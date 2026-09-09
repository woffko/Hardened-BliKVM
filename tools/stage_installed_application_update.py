#!/usr/bin/env python3
"""Drive bound installed application-update steps with phase-bound browser gates."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import installed_application_update as native
from tools import stage_device_prototype as transport
from tools import stage_initialized_updater_upgrade as packages
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require


def validate_guard(value: object) -> str:
    require(isinstance(value, str) and re.fullmatch(r"/var/lib/" + native.PREFIX + r"[a-f0-9]{24}", value) is not None,
            "invalid application guard")
    return cast(str, value)


def checkpoint(path: str) -> str:
    receipt = cast(dict[str, object], json.loads(transport.validate_private_file(path).read_bytes()))
    require(receipt.get("probe") == "installed-application-update" and receipt.get("bundleSha256") == native.active_profile.bundle_sha
            and receipt.get("profile", "seq1-to-seq2") == native.active_profile.name,
            "application checkpoint binding mismatch")
    return validate_guard(receipt.get("guardRoot"))


def browser_gate(path: str | None, root: str, phase: str) -> None:
    require(path is not None, "phase-bound browser receipt required")
    receipt = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, path)).read_bytes()))
    require(receipt.get("probe") == "hardened-blikvm-existing-account-browser" and receipt.get("result") == "passed"
            and receipt.get("runTag") == Path(root).name + "-" + phase, "browser receipt belongs to another phase")
    for name in ("videoDecoded", "streamRevoked", "secureSessionCookie", "secureCsrfCookie", "sessionCleared", "legacyBrandingAbsent"):
        require(receipt.get(name) is True, "browser acceptance incomplete")
    for name in ("loginStatus", "inventoryStatus", "diagnosticsStatus", "videoStatus", "logoutStatus"):
        require(receipt.get(name) == 200, "browser request failed")
    require(receipt.get("controlRequests") == 0 and receipt.get("webSockets") == 0 and receipt.get("secretEmitted") is False,
            "browser safety boundary failed")


def commit_browser_phase(path: str) -> str:
    checkpoint(path)
    receipt = cast(dict[str, object], json.loads(transport.validate_private_file(path).read_bytes()))
    require(receipt.get("result") == "passed", "successful application checkpoint required")
    device = cast(dict[str, object], receipt.get("device", {}))
    phase = device.get("phase", device.get("result"))
    require(phase in ("rehearsed", "committed-rehearsal", "confirmed", "committed-update"), "checkpoint phase cannot be committed")
    return "rehearsal" if phase in ("rehearsed", "committed-rehearsal") else "confirmed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "inspect", "activate", "recover", "commit", "cleanup"))
    parser.add_argument("--profile", choices=tuple(native.PROFILES), default="seq1-to-seq2")
    for name in ("endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    for name in ("old-package", "new-package", "bundle", "guard-receipt", "browser-receipt"):
        parser.add_argument("--" + name)
    args = parser.parse_args()
    native.select_profile(args.profile)
    packages.select_profile(native.active_profile.updater_profile)
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new receipt required")
    guard: str | None = None
    accepted_browser_phase: str | None = None
    uploads: dict[str, bytes] = {}
    if args.action == "prepare":
        require(args.old_package is not None and args.new_package is not None and args.bundle is not None, "bound inputs required")
        uploads = packages.upload_payloads(cast(str, args.old_package), cast(str, args.new_package))
        archive = Path(cast(str, args.bundle)).read_bytes()
        require(len(archive) == native.active_profile.bundle_bytes and hashlib.sha256(archive).hexdigest() == native.active_profile.bundle_sha, "application archive binding mismatch")
        uploads["application.tar"] = archive
        for name in ("installed_application_update.py", *native.HELPERS):
            uploads[name] = Path(__file__).with_name(name).read_bytes()
    else:
        require(args.guard_receipt is not None, "application checkpoint required")
        guard = checkpoint(args.guard_receipt)
        if args.action in ("activate", "commit"):
            accepted_browser_phase = "rehearsal" if args.action == "activate" else commit_browser_phase(args.guard_receipt)
            browser_gate(args.browser_receipt, guard, accepted_browser_phase)
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-application-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    receipt: dict[str, object] = {"probe": "installed-application-update", "action": args.action,
                                  "bundleSha256": native.active_profile.bundle_sha, "profile": native.active_profile.name, "targetPowerControl": False}

    def invoke(action: str) -> tuple[int, dict[str, object]]:
        argv = ["python3", "-B", stage + "/installed_application_update.py", "prepare", "--assets", stage] if action == "prepare" else [
            "python3", "-B", cast(str, guard) + "/controller.py", action, "--guard-root", cast(str, guard)]
        argv += ["--profile", native.active_profile.name]
        if action == "commit":
            require(accepted_browser_phase is not None, "browser phase missing")
            argv += ["--browser-phase", cast(str, accepted_browser_phase)]
        result = transport.run_ssh(target, secret, shlex.join(argv), timeout=180)
        return result.returncode, cast(dict[str, object], json.loads(result.stdout)) if result.stdout else {}

    try:
        if uploads:
            require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0, "controller staging failed")
            staged = True
            for name, data in uploads.items():
                destination = q(stage + "/" + name)
                script = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
                with tempfile.TemporaryFile() as stream:
                    stream.write(data)
                    stream.seek(0)
                    require(transport.run_ssh(target, secret, script, stdin=stream, timeout=60).returncode == 0, "upload failed")
        code, result = invoke(args.action)
        receipt["device"] = result
        if "guardRoot" in result:
            guard = validate_guard(result["guardRoot"])
        if guard:
            receipt["guardRoot"] = guard
            if args.action in ("prepare", "activate"):
                receipt["browserRunTag"] = Path(guard).name + ("-rehearsal" if args.action == "prepare" else "-confirmed")
        expected_result = {"prepare": "rehearsed", "recover": "recovered"}.get(args.action, args.action)
        require(code == 0 and result.get("result") == expected_result, "device application step failed")
        receipt["result"] = "passed"
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "errorType": type(error).__name__})
        if guard is not None and args.action != "recover":
            try:
                code, result = invoke("recover")
                receipt.update({"guardRoot": guard, "recovery": result, "recoverySucceeded": code == 0})
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt.update({"guardRoot": guard, "recoverySucceeded": False})
        return 1
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            exact = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {exact}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, script).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    raise SystemExit(main())
