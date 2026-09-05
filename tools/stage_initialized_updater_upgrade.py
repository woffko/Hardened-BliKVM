#!/usr/bin/env python3
"""Hash-bound SSH driver for preserving initialized state during updater upgrade."""

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

from tools import stage_device_prototype as transport
from tools import stage_device_update_runtime_package as package
from tools import upgrade_initialized_updater as native
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require
SOURCE_BINDINGS = {
    "old": ("e1e80628226cb3cccf02d8a99279d56c793166e8",
            "7345a9aec28ae0655dfc59ffd374c0a60e465b82f2b1bd289d7615d6f0d95bca"),
    "new": ("e371528fbb8ee2255dfc0822011aac6932457ad2",
            "76032fd77ae32ccb9732db5d22addfb28bf238fc56408be24e9f68d09a90a90f"),
}


def validate_guard(value: object) -> str:
    require(isinstance(value, str), "missing guard root")
    root = cast(str, value)
    require(re.fullmatch(r"/var/lib/" + native.PREFIX + r"[a-f0-9]{24}", root) is not None,
            "invalid returned guard namespace")
    return root


def validate_rehearsal(path: str | None) -> None:
    require(path is not None, "retention requires successful rollback rehearsal")
    receipt = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, path)).read_bytes()))
    require(receipt.get("probe") == "initialized-updater-upgrade" and receipt.get("action") == "rehearse"
            and receipt.get("result") == "passed" and receipt.get("packageHashes") == native.PACKAGE_HASHES
            and receipt.get("retainedVersion") == native.VERSIONS["old"]
            and receipt.get("guardRemoved") is True and receipt.get("controllerStagingRemoved") is True,
            "rollback rehearsal receipt mismatch")


def upload_payloads(old_path: str, new_path: str) -> dict[str, bytes]:
    bindings: dict[str, object] = {}
    uploads: dict[str, bytes] = {}
    for variant, path in (("old", old_path), ("new", new_path)):
        commit, bundle = SOURCE_BINDINGS[variant]
        info = package.validate_package(path, native.PACKAGE_HASHES[variant], commit, bundle, commit,
                                        expected_version=native.VERSIONS[variant])
        bindings[variant] = {"sha256": info.sha256, "version": native.VERSIONS[variant], "files": info.installed_files}
        uploads[variant + ".deb"] = info.path.read_bytes()
    uploads["bindings.json"] = json.dumps(bindings, sort_keys=True).encode()
    for name in (*native.HELPERS, "upgrade_initialized_updater.py"):
        uploads[name] = Path(__file__).with_name(name).read_bytes()
    return uploads


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "rehearse", "retain"))
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--rehearsal-receipt")
    args = parser.parse_args()
    if args.action == "retain":
        validate_rehearsal(args.rehearsal_receipt)
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new private receipt required")
    uploads = upload_payloads(args.old_package, args.new_package)
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-initialized-upgrade-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    guard: str | None = None
    phase = "stage"
    receipt: dict[str, object] = {"probe": "initialized-updater-upgrade", "action": args.action,
                                  "packageHashes": native.PACKAGE_HASHES, "targetPowerControl": False}

    def invoke(action: str, *, variant: str = "old") -> tuple[int, dict[str, object]]:
        script = stage + "/upgrade_initialized_updater.py" if action in ("preflight", "prepare") else cast(str, guard) + "/controller.py"
        argv = ["python3", "-B", script, action]
        argv += ["--assets", stage, "--variant", variant] if action in ("preflight", "prepare") else ["--guard-root", cast(str, guard)]
        result = transport.run_ssh(target, secret, shlex.join(argv), timeout=240)
        decoded = cast(dict[str, object], json.loads(result.stdout)) if result.stdout else {}
        return result.returncode, decoded

    try:
        require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                "controller stage failed")
        staged = True
        for name, data in uploads.items():
            destination = q(stage + "/" + name)
            script = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream, timeout=60).returncode == 0, "upload failed")
        phase = "preflight"
        code, before = invoke("preflight")
        receipt["before"] = before
        require(code == 0 and before.get("result") == "preflight", "preflight failed")
        if args.action == "preflight":
            receipt.update({"result": "passed", "retainedVersion": native.VERSIONS["old"], "guardRemoved": True})
            return 0
        phase = "prepare"
        code, prepared = invoke("prepare")
        receipt["prepare"] = prepared
        if "guardRoot" in prepared:
            guard = validate_guard(prepared["guardRoot"])
        require(code == 0 and prepared.get("result") == "prepared" and guard is not None, "guarded preparation failed")
        phase = "independent-inspect"
        code, inspected = invoke("inspect")
        receipt["inspection"] = inspected
        require(code == 0 and inspected.get("stateAndServingPreserved") is True, "independent verification failed")
        phase = "rollback" if args.action == "rehearse" else "commit"
        code, settled = invoke(phase)
        receipt["settlement"] = settled
        require(code == 0 and settled.get("stateAndServingPreserved") is True, "settlement failed")
        phase = "cleanup"
        code, cleaned = invoke("cleanup")
        receipt["cleanup"] = cleaned
        require(code == 0 and cleaned.get("result") == "cleanup", "guard cleanup failed")
        receipt["guardRemoved"] = True
        guard = None
        phase = "independent-final"
        variant = "old" if args.action == "rehearse" else "new"
        code, final = invoke("preflight", variant=variant)
        require(code == 0 and final == before, "final runtime or state differs")
        receipt.update({"result": "passed", "retainedVersion": native.VERSIONS[variant], "stateAndServingPreserved": True})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "phase": phase, "errorType": type(error).__name__})
        if guard is not None:
            try:
                code, recovery = invoke("rollback")
                receipt["recovery"] = recovery
                receipt["recoverySucceeded"] = code == 0
                receipt["guardRoot"] = guard
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["recoverySucceeded"] = False
                receipt["guardRoot"] = guard
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
