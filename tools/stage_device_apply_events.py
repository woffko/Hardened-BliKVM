#!/usr/bin/env python3
"""Run isolated apply-event systemd tests and verify the serving GUI is unchanged."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("supervisor", "expected-sha256", "fixture", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = cast(dict[str, str], vars(parser.parse_args()))
    output = transport.validate_output_path(args["output"])
    require(not output.exists(), "new receipt required")
    supervisor = Path(args["supervisor"]).read_bytes()
    require(len(supervisor) < 8 * 1024 * 1024 and hashlib.sha256(supervisor).hexdigest() == transport.validate_sha256(args["expected_sha256"]),
            "supervisor binding failed")
    fixture_root = Path(args["fixture"])
    fixture_bytes = (fixture_root / "fixture.json").read_bytes()
    fixture = cast(dict[str, object], json.loads(fixture_bytes))
    artifacts = cast(list[dict[str, object]], fixture["artifacts"])
    require({item["path"] for item in artifacts} == {f"application-{i}.tar" for i in (1, 2, 3)}, "unexpected fixture members")
    uploads: dict[str, bytes] = {"supervisor": supervisor, "fixture.json": fixture_bytes}
    for item in artifacts:
        name = cast(str, item["path"])
        data = (fixture_root / name).read_bytes()
        require(len(data) <= 1024 * 1024 and hashlib.sha256(data).hexdigest() == item["sha256"], "fixture hash mismatch")
        uploads[name] = data
    tool_root = Path(__file__).resolve().parent
    for name in ("verify_apply_events.py", "switch_signed_slot_server.py", "initialize_application_state.py", "application_bootstrap_roots.py"):
        uploads[name] = (tool_root / name).read_bytes()
    for name, source in (("apply.service", "hardened-blikvm-update-apply-confirmed.service"),
                         ("watchdog.service", "hardened-blikvm-update-watchdog.service"),
                         ("watchdog.timer", "hardened-blikvm-update-watchdog.timer")):
        uploads[name] = (tool_root.parent / "packaging/systemd" / source).read_bytes()
    host, port = transport.parse_endpoint(args["endpoint"])
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    token = secrets.token_hex(12)
    stage = "/tmp/blikvm-apply-event-controller-" + token
    remote_output = stage + ".json"
    q = shlex.quote
    secret = transport.read_secret_stdin()
    staged = False
    result: dict[str, object] = {"probe": "device-isolated-apply-events", "supervisorSha256": args["expected_sha256"]}
    phase = "stage"
    observer: str | None = None
    try:
        require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)}\ntest ! -e {q(remote_output)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                "staging failed")
        staged = True
        for name, data in uploads.items():
            script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream).returncode == 0, "upload failed")
        observer_code = "import json,sys;sys.path.insert(0," + repr(stage) + ");import switch_signed_slot_server as s;v=s.snapshot();r=s.verify(v,True);print(json.dumps({'snapshot':v,'verification':r},sort_keys=True))"
        observer = shlex.join(["python3", "-B", "-c", observer_code])
        phase = "before"
        before = transport.run_ssh(target, secret, observer, timeout=60)
        require(before.returncode == 0, "serving preflight failed")
        result["before"] = json.loads(before.stdout)
        phase = "isolated-systemd-test"
        test = transport.run_ssh(target, secret, shlex.join(["python3", "-B", stage + "/verify_apply_events.py",
                                                           "--assets", stage, "--output", remote_output]), timeout=150)
        if test.stdout:
            result["test"] = json.loads(test.stdout)
        require(test.returncode == 0, "isolated apply-event test failed")
        phase = "independent-after"
        after = transport.run_ssh(target, secret, observer, timeout=60)
        require(after.returncode == 0 and json.loads(after.stdout) == result["before"], "serving runtime changed")
        result.update({"result": "passed", "servingRuntimeUnchanged": True, "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        result.update({"result": "failed", "phase": phase, "errorType": type(error).__name__})
        return 1
    finally:
        if result.get("result") != "passed" and observer is not None and "before" in result:
            try:
                after = transport.run_ssh(target, secret, observer, timeout=60)
                result["failureRuntimeUnchanged"] = after.returncode == 0 and json.loads(after.stdout) == result["before"]
            except (OSError, ValueError, subprocess.SubprocessError):
                result["failureRuntimeUnchanged"] = False
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            paths = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"""set -eu
test ! -L {q(stage)}
test "$(stat -c '%a:%u:%g' {q(stage)})" = 700:0:0
test -z "$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)"
rm -f -- {paths}
rmdir {q(stage)}
if [ -e {q(remote_output)} ]; then
  test ! -L {q(remote_output)}
  test "$(stat -c '%a:%u:%g:%h' {q(remote_output)})" = 600:0:0:1
  rm -f -- {q(remote_output)}
fi
"""
            try:
                staged = transport.run_ssh(target, secret, script).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        result["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, result)
        for index in range(len(secret)):
            secret[index] = 0


if __name__ == "__main__":
    raise SystemExit(main())
