#!/usr/bin/env python3
"""Read bounded, redacted service failure facts; never reopen HTTP admission."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_application_write_gate as runner
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import json, pathlib, subprocess
units = ("hardened-blikvm-nginx.service", "hardened-blikvm-server-current.service", "hardened-blikvm-update-upload@*.service")
patterns = ("Permission denied", "No space left on device", "Connection refused", "client intended to send too large body", "upstream prematurely closed", "Failed", "failed", "status=", "quota", "invalid_archive")
facts = {}
for unit in units:
    result = subprocess.run(["journalctl", "--no-pager", "--output=cat", "--since=-20min", "--lines=100", "--unit=" + unit], capture_output=True, timeout=10)
    if result.returncode != 0 or len(result.stdout) > 262144:
        raise ValueError("bounded journal read failed")
    text = result.stdout.decode(errors="replace")
    facts[unit] = {"lines": len(text.splitlines()), "counts": {pattern: text.count(pattern) for pattern in patterns}}
for unit in ("hardened-blikvm-update-upload.socket", "hardened-blikvm-update-activation.socket"):
    result = subprocess.run(["systemctl", "show", unit, "--property=ActiveState,SubState,Result,Job"], capture_output=True, check=True, timeout=5)
    if len(result.stdout) > 4096:
        raise ValueError("oversize unit result")
    facts[unit] = dict(line.split("=", 1) for line in result.stdout.decode().splitlines())
print(json.dumps(facts, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = runner.transport.validate_output_path(args.output)
    runner.require(not output.exists(), "new failure evidence required")
    host, port = runner.transport.parse_endpoint(args.endpoint)
    target = runner.transport.RemoteTarget(host, port, "root", runner.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = runner.transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "application-http-failure-facts", "result": "failed", "readOnly": True}
    try:
        result = runner.transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", REMOTE]), timeout=60)
        runner.require(result.returncode == 0 and len(result.stdout) <= 16384, "failure fact capture failed")
        receipt["facts"] = json.loads(result.stdout)
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
