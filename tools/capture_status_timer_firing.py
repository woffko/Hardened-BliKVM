#!/usr/bin/env python3
"""Observe automatic timer completion after the arming SSH command exits."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import capture_status_restore_timer as timer
from tools import capture_subscription_preflight as subscription
from tools import status_timer_firing as firing
from tools.capture_pikvm_baseline import write_json_0600_atomic


def program(operation: str, phase: str, state: object = None) -> tuple[str, dict[str, str]]:
    timer.policy.guard_name(operation)
    if phase not in ("arm", "observe"):
        raise ValueError("invalid automatic timer phase")
    modules = (timer.resources, timer.ipc, timer.load, timer.policy, timer.properties, timer.metadata,
               timer.probe, subscription.event, subscription.monitor, subscription.worker, firing)
    sources = {module.__name__: Path(cast(str, module.__file__)).read_text() for module in modules}
    native_source = Path(subscription.native.__file__).read_text()
    child_sources = {module.__name__: sources[module.__name__]
                     for module in (timer.policy, timer.properties, subscription.event)}
    child_sources[subscription.native.__name__] = native_source
    worker_source = subscription.bootstrap(child_sources) + "sys.modules['tools.systemd_subscription'].subscription_worker()\n"
    status_source = sources[timer.ipc.__name__] + "\nprint(json.dumps(read_status(), sort_keys=True))\n"
    source = subscription.bootstrap(sources)
    if phase == "arm":
        invocation = f"arm({operation!r},{status_source!r})"
    else:
        handoff = firing.validate_handoff(state, operation)
        invocation = f"observe({operation!r},{handoff!r},{status_source!r},{worker_source!r})"
    source += f"result=sys.modules['tools.status_timer_firing'].{invocation}\n"
    source += "import json\nprint(json.dumps(result,sort_keys=True))\nraise SystemExit(0 if result['result'] in ('armed','passed') else 1)\n"
    if len(source.encode()) > 100000:
        raise ValueError("automatic timer probe source exceeds bound")
    return source, {name: hashlib.sha256(value.encode()).hexdigest()
                    for name, value in {**sources, subscription.native.__name__: native_source}.items()}


def validate_completion(value: object, handoff: dict[str, object]) -> dict[str, object]:
    result = timer.ipc.object_value(value)
    checks = {"guardRemoved", "statusUnchanged", "servingIdentityUnchanged", "statusSocketNeverStopped"}
    timer.ipc.require(set(result) == checks | {"result", "operation", "completion", "guardMayRemain"}
                      and result["result"] == "passed" and result["operation"] == handoff["operation"]
                      and all(result[key] is True for key in checks) and result["guardMayRemain"] is False,
                      "automatic timer completion not accepted")
    event = timer.ipc.object_value(result["completion"])
    timer.ipc.require(set(event) == {"jobId", "unit", "result", "receivedMonotonicUsec", "timingSource"}
                      and event["unit"] == timer.policy.guard_name(cast(str, handoff["operation"])) + ".service"
                      and event["result"] == "done" and event["timingSource"] == "observer-receive-clock",
                      "invalid automatic timer event")
    job = firing.uint(event["jobId"], 32)
    received = firing.uint(event["receivedMonotonicUsec"], 64)
    deadline = firing.uint(handoff["deadlineUsec"], 64)
    timer.ipc.require(job > 0 and deadline <= received <= deadline + 25_000_000, "automatic timer event outside window")
    return result


def remember_failure(raw: bytes, operation: str, receipt: dict[str, object]) -> None:
    """Keep only closed failure categories, never raw remote error output."""
    if len(raw) > 8192:
        return
    try:
        value = timer.ipc.object_value(json.loads(raw))
        phase, error = value.get("phase"), value.get("errorType")
        if (set(value) == {"result", "operation", "phase", "errorType", "guardMayRemain"}
                and value["result"] == "failed" and value["operation"] == operation
                and type(value["guardMayRemain"]) is bool and isinstance(phase, str)
                and phase in {"arm", "handoff", "subscribe-monitor", "await-execution", "verify-after", "cleanup"}
                and isinstance(error, str) and error in {"OSError", "ValueError", "RuntimeError", "KeyError",
                    "CalledProcessError", "TimeoutExpired", "PermissionError", "FileNotFoundError", "JSONDecodeError"}):
            receipt.update({"failurePhase": phase, "remoteErrorType": error, "guardMayRemain": value["guardMayRemain"]})
    except (ValueError, TypeError):
        return


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    transport = timer.transport
    output = transport.validate_output_path(args.output)
    timer.ipc.require(not output.exists(), "new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    operation = secrets.token_hex(12)
    source, hashes = program(operation, "arm")
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "status-timer-automatic-completion", "result": "failed",
                                "operation": operation, "sourceSha256": hashes, "guardMayRemain": True}
    try:
        arm = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=60)
        receipt["armSshExitCode"] = arm.returncode
        if arm.returncode != 0:
            remember_failure(arm.stdout, operation, receipt)
        timer.ipc.require(arm.returncode == 0 and len(arm.stdout) <= 32768, "arming SSH command failed")
        handoff = firing.validate_handoff(json.loads(arm.stdout), operation)
        receipt.update({"handoff": handoff, "armingCommandExited": True})
        # Persist the known timer identity before opening the second connection.
        write_json_0600_atomic(output, receipt)
        source, next_hashes = program(operation, "observe", handoff)
        timer.ipc.require(next_hashes == hashes, "probe source changed between SSH commands")
        observed = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=180)
        receipt["observerSshExitCode"] = observed.returncode
        if observed.returncode != 0:
            remember_failure(observed.stdout, operation, receipt)
            raise ValueError("automatic timer observation failed")
        timer.ipc.require(len(observed.stdout) <= 8192, "oversize observer receipt")
        value = timer.ipc.object_value(json.loads(observed.stdout))
        receipt["observation"] = validate_completion(value, handoff)
        receipt["guardMayRemain"] = False
        receipt["result"] = "passed"
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
