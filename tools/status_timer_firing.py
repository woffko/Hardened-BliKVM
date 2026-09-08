"""Two-phase automatic timer test; observation never starts the restorer."""

from __future__ import annotations

import json
import os
import re
import subprocess
import time
from typing import cast

from tools import probe_status_restore_timer as probe
from tools import status_outage_contract as policy
from tools import status_restore_properties as metadata
from tools import observe_service_resources as resources
from tools.observe_retained_backend_load import retained_status
from tools.status_restore_event import SYSTEM_BUS_ADDRESS
from tools.status_restore_monitor import RestoreMonitor
from tools.subscription_process import SubscriptionProcess
from tools.systemd_property import uint

require = resources.require


def cleanup(operation: str) -> bool:
    name = policy.guard_name(operation)
    try:
        if not probe.unit_absent(name + ".timer"):
            require(probe.own_timer_identity(name + ".timer"), "timer identity unavailable")
            try:
                probe.command(["systemctl", "stop", name + ".timer"])
            except subprocess.CalledProcessError:
                require(probe.unit_absent(name + ".timer"), "timer stop failed")
        # The owned restoration service has TimeoutStartSec=20s. Its job may
        # already be running when the timer is stopped; never cancel it.
        deadline = time.monotonic() + 25
        while time.monotonic() < deadline:
            if probe.unit_absent(name + ".timer") and probe.unit_absent(name + ".service"):
                return True
            time.sleep(0.25)
    except (OSError, ValueError, subprocess.SubprocessError):
        pass
    return False


def validate_handoff(value: object, operation: str) -> dict[str, object]:
    policy.guard_name(operation)
    require(isinstance(value, dict), "invalid timer handoff")
    handoff = cast(dict[str, object], value)
    require(set(handoff) == {"result", "operation", "baseline", "status", "deadlineUsec"}
            and handoff["result"] == "armed" and handoff["operation"] == operation, "timer handoff identity mismatch")
    retained_status(handoff["status"])
    require(isinstance(handoff["baseline"], dict), "invalid baseline")
    baseline = cast(dict[str, object], handoff["baseline"])
    require(set(baseline) == {"monotonicNs", "bootIdSha256", "units"}, "invalid baseline fields")
    clock = uint(baseline["monotonicNs"], 64)
    boot = baseline["bootIdSha256"]
    require(isinstance(boot, str) and re.fullmatch(r"[a-f0-9]{64}", boot) is not None, "invalid baseline boot")
    require(isinstance(baseline["units"], dict), "invalid baseline units")
    units = cast(dict[str, object], baseline["units"])
    require(set(units) == set(resources.UNITS), "baseline service set changed")
    for item in units.values():
        require(isinstance(item, dict), "invalid baseline service")
        unit = cast(dict[str, object], item)
        require(set(unit) == set(resources.UnitSample.__annotations__), "invalid baseline service fields")
        for number in unit.values():
            uint(number, 64)
        require(all(cast(int, unit[key]) > 0 for key in ("mainPid", "mainStartTicks", "processes")),
                "invalid baseline service identity")
    require(clock > 0 and uint(handoff["deadlineUsec"], 64) > clock // 1000, "invalid handoff deadline")
    return handoff


def guard(operation: str) -> tuple[int, int]:
    name = policy.guard_name(operation)
    timer = probe.properties(name + ".timer", "Timer", metadata.TIMER_PROPERTIES)
    service = probe.properties(name + ".service", "Service", metadata.SERVICE_PROPERTIES)
    remaining = metadata.verify_guard_properties(operation, timer, service, now_usec=time.monotonic_ns() // 1000)
    deadline = cast(dict[str, object], timer["NextElapseUSecMonotonic"])["data"]
    require(remaining >= policy.MIN_REMAINING_SECONDS, "guard margin too short")
    return uint(deadline, 64), remaining


def manager_owner() -> str:
    base = ["/usr/bin/busctl", "--system", "--address=" + SYSTEM_BUS_ADDRESS, "--json=short", "--timeout=5s",
            "call", "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus"]
    reply = json.loads(probe.command(base + ["GetNameOwner", "s", "org.freedesktop.systemd1"], timeout=6))
    require(isinstance(reply, dict), "invalid manager owner reply")
    value = cast(dict[str, object], reply)
    require(set(value) == {"type", "data"} and value["type"] == "s" and isinstance(value["data"], list),
            "invalid manager owner signature")
    names = cast(list[object], value["data"])
    require(len(names) == 1 and isinstance(names[0], str), "invalid manager owner data")
    owner = cast(str, names[0])
    require(re.fullmatch(r":[0-9]{1,10}\.[0-9]{1,10}", owner) is not None, "invalid manager owner")
    pid = json.loads(probe.command(base + ["GetConnectionUnixProcessID", "s", owner], timeout=6))
    require(pid == {"type": "u", "data": [1]} and type(pid["data"][0]) is int, "manager is not PID 1")
    return owner


def arm(operation: str, status_source: str) -> dict[str, object]:
    name = policy.guard_name(operation)
    attempted = False
    try:
        require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected host")
        baseline = resources.snapshot()
        status = probe.status_read(status_source)
        probe.quiet_socket()
        probe.namespace_pristine(operation)
        unused = probe.unit_absent(name + ".timer") and probe.unit_absent(name + ".service")
        argv = policy.arm_command(operation, namespace_unused=unused,
                                  observation=policy.OutageObservation(True, True, 0, False, 0))
        attempted = True
        probe.command(argv, timeout=15)
        deadline, _ = guard(operation)
        probe.same_identity(baseline, resources.snapshot())
        return {"result": "armed", "operation": operation, "baseline": baseline,
                "status": status, "deadlineUsec": deadline}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        removed = cleanup(operation) if attempted else True
        return {"result": "failed", "operation": operation, "phase": "arm",
                "errorType": type(error).__name__, "guardMayRemain": not removed}


def observe(operation: str, state: object, status_source: str, worker_source: str) -> dict[str, object]:
    phase = "handoff"
    result: dict[str, object]
    removed = False
    authorized = False
    try:
        require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected host")
        handoff = validate_handoff(state, operation)
        authorized = True
        before = cast(resources.Sample, handoff["baseline"])
        probe.same_identity(before, resources.snapshot())
        require(probe.status_read(status_source) == handoff["status"], "status changed before observation")
        phase = "subscribe-monitor"
        with SubscriptionProcess(worker_source) as subscription:
            owner = manager_owner()
            with RestoreMonitor(operation) as monitor:
                deadline, _ = guard(operation)
                require(deadline == handoff["deadlineUsec"], "guard deadline changed across SSH sessions")
                subscription.ensure_alive()
                phase = "await-execution"
                event = monitor.completed(owner, deadline)
                subscription.ensure_alive()
        require(subscription.process.returncode == 0, "subscription did not close cleanly")
        phase = "verify-after"
        require(probe.status_read(status_source) == handoff["status"], "status changed after automatic start")
        probe.same_identity(before, resources.snapshot())
        probe.quiet_socket()
        phase = "cleanup"
        removed = cleanup(operation)
        require(removed, "timer cleanup not confirmed")
        require(probe.status_read(status_source) == handoff["status"], "final status changed")
        probe.same_identity(before, resources.snapshot())
        probe.quiet_socket()
        result = {"result": "passed", "operation": operation, "completion": event,
                  "guardRemoved": True, "statusUnchanged": True, "servingIdentityUnchanged": True,
                  "statusSocketNeverStopped": True}
    except (OSError, ValueError, RuntimeError, KeyError, subprocess.SubprocessError) as error:
        result = {"result": "failed", "operation": operation, "phase": phase, "errorType": type(error).__name__}
    finally:
        # No outage was created by either phase. Cancel only the owned timer;
        # never stop a restoration service that might already be executing.
        if authorized and not removed:
            removed = cleanup(operation)
    result["guardMayRemain"] = not removed
    return result
