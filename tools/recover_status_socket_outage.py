#!/usr/bin/env python3
"""Verify fixed restoration and remove the guard of one recorded outage."""

from __future__ import annotations

import argparse
import json
import shlex
import subprocess
import sys
from pathlib import Path
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import capture_status_socket_outage as capture
from tools.capture_pikvm_baseline import write_json_0600_atomic


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--receipt", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    transport = capture.timer.transport
    previous = transport.validate_private_file(args.receipt)
    output = transport.validate_output_path(args.output)
    capture.require(not output.exists() and previous.stat().st_size <= 65536, "invalid recovery receipt paths")
    recorded = capture.timer.ipc.object_value(json.loads(previous.read_bytes()))
    capture.require(recorded.get("probe") == "status-socket-browser-outage" and recorded.get("guardMayRemain") is True,
                    "recorded outage recovery is not pending")
    operation = recorded.get("operation")
    capture.require(isinstance(operation, str), "missing recorded operation")
    operation = cast(str, operation)
    state = capture.capture.firing.validate_handoff(recorded.get("handoff"), operation)
    source, hashes = capture.program(operation, "finish", state)
    capture.require(hashes == recorded.get("sourceSha256"), "outage recovery sources changed")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    result: dict[str, object] = {"probe": "status-outage-recovery", "operation": operation,
                                "result": "failed", "guardMayRemain": True, "sourceSha256": hashes}
    try:
        response = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=80)
        capture.require(response.returncode == 0 and len(response.stdout) <= 8192, "outage recovery SSH failed")
        result["finish"] = capture.validate_phase(json.loads(response.stdout), operation, "finish")
        result["guardMayRemain"] = False
        result["result"] = "passed"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        result["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, result)
    if result["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
