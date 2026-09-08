"""Fixed status-socket outage phases; no server or hardware-control actions.

Arming uses status_timer_firing.arm. The caller must persist that handoff before
requesting stop, and retain the timer until the browser has logged out and
restoration has been verified. A failed stop always attempts the fixed restore.
"""

from __future__ import annotations

import os
import subprocess
import time
from typing import cast

from tools import observe_service_resources as resources
from tools import probe_status_restore_timer as probe
from tools import status_outage_contract as policy
from tools import status_timer_firing as timer
from tools.systemd_property import decode_property

require = resources.require
WORKERS = probe.WORKERS


def stopped_and_drained() -> None:
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        active = decode_property(probe.property_value(policy.SOCKET, "Unit", "ActiveState", "s"), "s")
        substate = decode_property(probe.property_value(policy.SOCKET, "Unit", "SubState", "s"), "s")
        count = decode_property(probe.property_value(policy.SOCKET, "Socket", "NConnections", "u"), "u")
        # Accept=yes service instances can survive listener stop. Do not infer
        # their exit from listener state or NConnections alone. Require every
        # exact-template instance to have been collected; even inactive/failed
        # retained instances fail closed here.
        collected = probe.workers_collected()
        require(active in ("inactive", "deactivating") and substate in ("dead", "stop-pre", "stop-post", "stop-sigterm", "stop-sigkill"),
                "status socket did not stop")
        if active == "inactive" and substate == "dead" and count == 0 and collected:
            return
        time.sleep(0.25)
    raise ValueError("status workers did not drain")


def handoff(operation: str, state: object) -> dict[str, object]:
    require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected outage host")
    return timer.validate_handoff(state, operation)


def verified_serving(state: dict[str, object], status_source: str) -> None:
    require(probe.status_read(status_source) == state["status"], "application status changed")
    probe.same_identity(cast(resources.Sample, state["baseline"]), resources.snapshot())
    probe.quiet_socket()


def emergency_restore() -> None:
    # Kept available even if a stop reply or handoff was lost. Never disarm here.
    probe.command(policy.restore_command(), timeout=15)


def stop(operation: str, state: object, status_source: str) -> dict[str, object]:
    value = handoff(operation, state)
    verified_serving(value, status_source)
    deadline, remaining = timer.guard(operation)
    require(deadline == value["deadlineUsec"], "restore deadline changed")
    argv = policy.stop_command(policy.OutageObservation(True, True, 0, True, remaining))
    try:
        probe.command(argv, timeout=15)
        stopped_and_drained()
        probe.same_identity(cast(resources.Sample, value["baseline"]), resources.snapshot())
        return {"result": "stopped", "operation": operation, "workersDrained": True,
                "servingIdentityUnchanged": True, "guardMayRemain": True}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
        emergency_restore()
        raise


def restore(operation: str, state: object, status_source: str) -> dict[str, object]:
    value = handoff(operation, state)
    emergency_restore()
    verified_serving(value, status_source)
    return {"result": "restored", "operation": operation, "statusUnchanged": True,
            "servingIdentityUnchanged": True, "guardMayRemain": True}


def finish(operation: str, state: object, status_source: str) -> dict[str, object]:
    # Only the host's post-logout phase may call this. Timer removal follows
    # successful fixed restoration and verified status/serving observations.
    value = handoff(operation, state)
    emergency_restore()
    verified_serving(value, status_source)
    require(timer.cleanup(operation), "restore guard removal not confirmed")
    verified_serving(value, status_source)
    return {"result": "finished", "operation": operation, "statusUnchanged": True,
            "servingIdentityUnchanged": True, "guardMayRemain": False}
