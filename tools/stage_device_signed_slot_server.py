#!/usr/bin/env python3
"""Transport the serving cutover controller and correlate browser acceptance."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import stat
import subprocess
import sys
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as transport
from tools import switch_signed_slot_server as native
from tools.capture_pikvm_baseline import write_json_0600_atomic


def browser_accepted(path: Path, tag: str) -> None:
    private = (Path(__file__).resolve().parent.parent / "evidence/private").resolve()
    require = native.require
    require(not path.is_symlink() and path.resolve().parent == private, "private browser receipt required")
    metadata = path.stat()
    require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.geteuid() and metadata.st_nlink == 1
            and stat.S_IMODE(metadata.st_mode) == 0o600 and 0 < metadata.st_size <= 16384, "unsafe browser receipt")
    decoded = cast(object, json.loads(path.read_bytes()))
    require(isinstance(decoded, dict), "invalid browser receipt")
    data = cast(dict[str, object], decoded)
    require(data.get("probe") == "hardened-blikvm-existing-account-browser" and data.get("result") == "passed"
            and data.get("runTag") == tag and data.get("videoDecoded") is True
            and data.get("sessionCleared") is True and data.get("streamRevoked") is True
            and data.get("controlRequests") == 0 and data.get("webSockets") == 0,
            "browser did not accept this serving transition")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "commit", "rollback", "cleanup"))
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--guard-root")
    parser.add_argument("--browser-receipt")
    args = parser.parse_args()
    action = cast(str, args.action)
    output = transport.validate_output_path(cast(str, args.output))
    native.require(not output.exists(), "new receipt required")
    guard = cast(str | None, args.guard_root)
    if action not in ("preflight", "prepare"):
        native.require(isinstance(guard, str) and re.fullmatch(r"/var/lib/hardened-blikvm-slot-serving-[a-f0-9]{24}", guard) is not None,
                       "exact serving guard required")
    if action == "commit":
        native.require(args.browser_receipt is not None and guard is not None, "browser acceptance required")
        assert guard is not None
        browser_accepted(Path(cast(str, args.browser_receipt)), Path(guard).name)
    host, port = transport.parse_endpoint(cast(str, args.endpoint))
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    stage = "/tmp/blikvm-serving-controller-" + secrets.token_hex(12)
    q = shlex.quote
    password = transport.read_secret_stdin()
    staged = False
    receipt: dict[str, object] = {"probe": "signed-slot-serving", "action": action}
    try:
        created = transport.run_ssh(target, password, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}")
        native.require(created.returncode == 0, "controller staging failed")
        staged = True
        for name, source in (("controller.py", Path(native.__file__)),
                             ("initialize_application_state.py", Path(native.bootstrap.__file__)),
                             ("application_bootstrap_roots.py", Path(__file__).with_name("application_bootstrap_roots.py"))):
            data = source.read_bytes()
            script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                result = transport.run_ssh(target, password, script, stdin=stream)
                native.require(result.returncode == 0, "controller upload failed")
        command = ["python3", "-B", stage + "/controller.py", action]
        if guard:
            command += ["--guard-root", guard]
        result = transport.run_ssh(target, password, shlex.join(command), timeout=180)
        native.require(len(result.stdout) <= 16384, "oversized serving response")
        observed = cast(dict[str, object], json.loads(result.stdout))
        receipt["device"] = observed
        native.require(result.returncode == 0, "serving transition failed")
        receipt.update({"result": "passed", "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "errorType": type(error).__name__})
        return 1
    finally:
        if staged:
            script = f"""set -eu
test ! -L {q(stage)}
test "$(stat -c '%a:%u:%g' {q(stage)})" = 700:0:0
test -z "$(find {q(stage)} -mindepth 1 -maxdepth 1 ! -name controller.py ! -name initialize_application_state.py ! -name application_bootstrap_roots.py -print -quit)"
rm -f -- {q(stage + '/controller.py')} {q(stage + '/initialize_application_state.py')} {q(stage + '/application_bootstrap_roots.py')}
rmdir {q(stage)}
"""
            try:
                staged = transport.run_ssh(target, password, script).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        for index in range(len(password)):
            password[index] = 0


if __name__ == "__main__":
    raise SystemExit(main())
