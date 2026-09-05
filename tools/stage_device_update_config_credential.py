#!/usr/bin/env python3
"""Verify AArch64 update config delivery through transient device services."""

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
from tools import stage_device_unused_update_trust_removal as boundary
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("supervisor", "expected-sha256", "enrollment-receipt", "expected-config-sha256", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--known-hosts", default="evidence/private/ssh-known-hosts")
    parser.add_argument("--baseline", default="evidence/private/device-web-cutover-preflight.json")
    args = cast(dict[str, str], vars(parser.parse_args()))
    output = transport.validate_output_path(args["output"])
    require(not output.exists(), "new receipt path required")
    expected = transport.validate_sha256(args["expected_sha256"])
    binary_path = Path(args["supervisor"])
    require(not binary_path.is_symlink() and 0 < binary_path.stat().st_size < 8 * 1024 * 1024, "invalid binary source")
    binary = binary_path.read_bytes()
    require(hashlib.sha256(binary).hexdigest() == expected, "binary hash mismatch")
    require(binary[:5] == b"\x7fELF\x02" and binary[18:20] == b"\xb7\x00", "AArch64 ELF required")
    runner = Path(__file__).with_name("verify_update_config_credential.py").read_bytes()
    enrollment = boundary.load_enrollment_receipt(args["enrollment_receipt"], args["expected_config_sha256"])
    baseline = cutover.load_baseline(args["baseline"])
    host, port = transport.parse_endpoint(args["endpoint"])
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file(args["known_hosts"]))
    token = secrets.token_hex(12)
    stage = "/tmp/hardened-blikvm-config-credential-" + token
    remote_receipt = stage + ".json"
    q = shlex.quote
    prepare = f"""set -eu
test ! -e {q(stage)} && test ! -L {q(stage)}
test ! -e {q(remote_receipt)} && test ! -L {q(remote_receipt)}
test -z "$(find /tmp -maxdepth 1 -name 'blikvm-config-credential-*' -print -quit)"
mkdir -m 700 {q(stage)}
printf '%s' {q(token)} > {q(stage + '/owner')}
"""
    cleanup = f"""set -eu
if [ -e {q(stage)} ]; then
  test ! -L {q(stage)}
  test "$(stat -c '%a:%u:%g' {q(stage)})" = 700:0:0
  test "$(cat {q(stage + '/owner')})" = {q(token)}
  test -z "$(find {q(stage)} -mindepth 1 -maxdepth 1 ! -name owner ! -name supervisor ! -name runner.py -print -quit)"
  rm -f -- {q(stage + '/supervisor')} {q(stage + '/runner.py')} {q(stage + '/owner')}
  rmdir {q(stage)}
fi
if [ -e {q(remote_receipt)} ]; then
  test ! -L {q(remote_receipt)}
  test "$(stat -c '%a:%u:%g:%h' {q(remote_receipt)})" = 600:0:0:1
  rm -f -- {q(remote_receipt)}
fi
test -z "$(find /tmp -maxdepth 1 -name 'blikvm-config-credential-*' -print -quit)"
"""
    password = transport.read_secret_stdin()
    created = False
    phase = "preflight"
    receipt: dict[str, object] = {"probe": "device-update-config-credential", "supervisorSha256": expected}
    try:
        before = transport.run_ssh(target, password, boundary.build_preflight_script(enrollment, baseline), timeout=90)
        require(before.returncode == 0, "baseline rejected")
        before_facts = boundary.parse_facts(before.stdout, boundary.PRE_KEYS)
        phase = "prepare"
        created = True
        require(transport.run_ssh(target, password, prepare).returncode == 0, "stage creation failed")
        for name, data, mode in (("supervisor", binary, "755"), ("runner.py", runner, "600")):
            phase = "upload-" + name
            script = f"set -eu\numask 077\ntest \"$(cat {q(stage + '/owner')})\" = {q(token)}\ncat > {q(stage + '/' + name)}\nchmod {mode} {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as source:
                source.write(data)
                source.seek(0)
                require(transport.run_ssh(target, password, script, stdin=source).returncode == 0, "upload failed")
        phase = "credential-probe"
        command = f"python3 {q(stage + '/runner.py')} --supervisor {q(stage + '/supervisor')} --output {q(remote_receipt)} --service-user hardened-blikvm-update"
        result = transport.run_ssh(target, password, command, timeout=90)
        if result.returncode != 0:
            receipt["probeExitCode"] = result.returncode
            receipt["probeError"] = result.stderr[-4096:].decode(errors="replace")
        require(result.returncode == 0, "credential probe failed")
        observed = cast(dict[str, object], json.loads(result.stdout))
        require(observed.get("result") == "passed" and observed.get("supervisorSha256") == expected, "invalid probe receipt")
        receipt["credentialProbe"] = observed
        phase = "cleanup"
        require(transport.run_ssh(target, password, cleanup).returncode == 0, "cleanup failed")
        created = False
        phase = "independent-final"
        after = transport.run_ssh(target, password, boundary.build_preflight_script(enrollment, baseline), timeout=90)
        require(after.returncode == 0, "final baseline rejected")
        require(boundary.parse_facts(after.stdout, boundary.PRE_KEYS) == before_facts, "runtime changed")
        receipt.update({"result": "passed", "runtimeUnchanged": True, "trustConfigUnchanged": True,
                        "stateInitialized": False, "unitsEnabled": False, "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "stage": phase, "errorType": type(error).__name__})
        return 1
    finally:
        if created:
            try:
                created = transport.run_ssh(target, password, cleanup, timeout=45).returncode != 0
            except (OSError, subprocess.SubprocessError):
                created = True
        receipt["cleanupConfirmed"] = not created
        write_json_0600_atomic(output, receipt)
        for index in range(len(password)):
            password[index] = 0


if __name__ == "__main__":
    raise SystemExit(main())
