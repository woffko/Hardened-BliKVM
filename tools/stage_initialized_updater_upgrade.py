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
from tools import enable_update_services as infrastructure
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require
SOURCE_BINDINGS = {
    "old": ("f22615d98ec69ff61fb49def0847df36c0ebb690",
            "21436994ad8f57db2ddf97e37fd241288cf56bc700efb8c7339ae7c5dd8e9d1c"),
    "new": ("4d47974a13eea1e563107c6611a52d520d90ccf0",
            "a8b1d9ec9150baf8f6f67d57a3d268fa5854c8415b13ccce58a53091077d91cc"),
}
LEGACY_SOURCE_BINDINGS = dict(SOURCE_BINDINGS)


def select_profile(name: str) -> None:
    native.select_profile(name)
    SOURCE_BINDINGS.update(LEGACY_SOURCE_BINDINGS if name == "dev6-dev7" else {
        "old": LEGACY_SOURCE_BINDINGS["new"],
        "new": ("77192d35a137e8d2c9658cb24a5b637d8915736d",
                "4097793ec08d946071dfd871e343fd3ab66d67877017e1bf1f3e143892c467d3"),
    })


def validate_guard(value: object, *, prefix: str = native.PREFIX) -> str:
    require(isinstance(value, str), "missing guard root")
    root = cast(str, value)
    require(re.fullmatch(r"/var/lib/" + prefix + r"[a-f0-9]{24}", root) is not None,
            "invalid returned guard namespace")
    return root


def validate_rehearsal(path: str | None, *, enable_services: bool = False) -> None:
    require(path is not None, "retention requires successful rollback rehearsal")
    receipt = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, path)).read_bytes()))
    expected_probe = "updater-infrastructure" if enable_services else "initialized-updater-upgrade"
    require(receipt.get("probe") == expected_probe and receipt.get("action") == "rehearse"
            and receipt.get("result") == "passed" and receipt.get("packageHashes") == native.PACKAGE_HASHES
            and receipt.get("retainedVersion") == native.VERSIONS["new" if enable_services else "old"]
            and receipt.get("guardRemoved") is True and receipt.get("controllerStagingRemoved") is True,
            "rollback rehearsal receipt mismatch")
    require(receipt.get("profile", "dev6-dev7") == native.selected_profile, "rehearsal profile mismatch")


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
    for name in dict.fromkeys((*native.HELPERS, "upgrade_initialized_updater.py")):
        uploads[name] = Path(__file__).with_name(name).read_bytes()
    return uploads


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "rehearse", "retain"))
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--rehearsal-receipt")
    parser.add_argument("--infrastructure", action="store_true", help="Enable idle updater services instead of changing the package")
    parser.add_argument("--profile", choices=native.PROFILES, default="dev6-dev7")
    args = parser.parse_args()
    require(not args.infrastructure or args.profile == "dev6-dev7", "infrastructure only supports legacy profile")
    select_profile(args.profile)
    if args.action == "retain":
        validate_rehearsal(args.rehearsal_receipt, enable_services=args.infrastructure)
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new private receipt required")
    uploads = upload_payloads(args.old_package, args.new_package)
    controller_name = "enable_update_services.py" if args.infrastructure else "upgrade_initialized_updater.py"
    guard_prefix = infrastructure.PREFIX if args.infrastructure else native.PREFIX
    if args.infrastructure:
        uploads[controller_name] = Path(__file__).with_name(controller_name).read_bytes()
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-initialized-upgrade-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    guard: str | None = None
    phase = "stage"
    receipt: dict[str, object] = {"probe": "updater-infrastructure" if args.infrastructure else "initialized-updater-upgrade", "action": args.action,
                                  "profile": native.selected_profile, "packageHashes": native.PACKAGE_HASHES, "targetPowerControl": False}

    def invoke(action: str, *, variant: str = "old") -> tuple[int, dict[str, object]]:
        script = stage + "/" + controller_name if action in ("preflight", "prepare") else cast(str, guard) + "/controller.py"
        argv = ["python3", "-B", script, action]
        if not args.infrastructure:
            argv += ["--profile", native.selected_profile]
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
            receipt.update({"result": "passed", "retainedVersion": native.VERSIONS["new" if args.infrastructure else "old"], "guardRemoved": True})
            return 0
        phase = "prepare"
        code, prepared = invoke("prepare")
        receipt["prepare"] = prepared
        if "guardRoot" in prepared:
            guard = validate_guard(prepared["guardRoot"], prefix=guard_prefix)
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
        receipt.update({"result": "passed", "retainedVersion": native.VERSIONS["new" if args.infrastructure else variant],
                        "stateAndServingPreserved": True, "infrastructureEnabled": bool(args.infrastructure and args.action == "retain")})
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
