#!/usr/bin/env python3
"""Drive one bounded reversible installed status API acceptance transaction."""

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
from tools import status_api_wiring as native
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require


def validate_guard(value: object) -> str:
    require(isinstance(value, str) and re.fullmatch(r"/var/lib/" + native.PREFIX + "[a-f0-9]{24}", value) is not None,
            "invalid returned status guard")
    return cast(str, value)


def checkpoint(path: str) -> str:
    value = cast(dict[str, object], json.loads(transport.validate_private_file(path).read_bytes()))
    require(value.get("probe") == "installed-status-api-wiring" and value.get("slotId") == native.SLOT,
            "status checkpoint mismatch")
    return validate_guard(value.get("guardRoot"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "inspect", "restore", "cleanup", "verify-clean"))
    for name in ("endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--guard-receipt")
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new status receipt required")
    guard = None if args.action == "prepare" else checkpoint(args.guard_receipt)
    uploads = {name: Path(__file__).with_name(name).read_bytes() for name in native.HELPERS} if args.action in ("prepare", "verify-clean") else {}
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-status-wiring-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    receipt: dict[str, object] = {"probe": "installed-status-api-wiring", "action": args.action,
                                  "slotId": native.SLOT, "targetPowerControl": False}

    def invoke(action: str) -> tuple[int, dict[str, object]]:
        argv = ["python3", "-B", stage + "/status_api_wiring.py", "prepare", "--assets", stage] if action == "prepare" else [
            "python3", "-B", cast(str, guard) + "/controller.py", action, "--guard-root", cast(str, guard)]
        if action == "verify-clean":
            argv = ["python3", "-B", stage + "/status_api_wiring.py", action, "--guard-root", cast(str, guard)]
        result = transport.run_ssh(target, secret, shlex.join(argv), timeout=180)
        return result.returncode, cast(dict[str, object], json.loads(result.stdout)) if result.stdout else {}

    try:
        if uploads:
            require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                    "status controller staging failed")
            staged = True
            for name, data in uploads.items():
                destination = q(stage + "/" + name)
                command = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
                with tempfile.TemporaryFile() as stream:
                    stream.write(data)
                    stream.seek(0)
                    require(transport.run_ssh(target, secret, command, stdin=stream, timeout=60).returncode == 0, "status controller upload failed")
        code, result = invoke(args.action)
        receipt["device"] = result
        if "guardRoot" in result:
            guard = validate_guard(result["guardRoot"])
        if guard:
            receipt["guardRoot"] = guard
        require(code == 0 and result.get("result") == ("prepared" if args.action == "prepare" else args.action), "status device step failed")
        if args.action in ("prepare", "restore"):
            receipt["browserRunTag"] = Path(cast(str, guard)).name + ("-enabled" if args.action == "prepare" else "-restored")
        receipt["result"] = "passed"
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "errorType": type(error).__name__})
        if guard is not None and args.action == "prepare":
            try:
                code, restored = invoke("restore")
                receipt.update({"recovery": restored, "recoverySucceeded": code == 0})
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["recoverySucceeded"] = False
        return 1
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            exact = " ".join(q(stage + "/" + name) for name in uploads)
            command = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {exact}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, command).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    raise SystemExit(main())
