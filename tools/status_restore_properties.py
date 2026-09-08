"""Verify complete selected metadata of the fixed, unrun restore guard.

No device operations. Callers must query these properties on the system bus
for the exact validated operation names, and obtain a fresh device clock.
"""

from tools import status_outage_contract as policy
from tools.systemd_property import decode_property, guard_remaining_seconds

UNIT_PROPERTIES = {"Id": "s", "LoadState": "s", "ActiveState": "s", "SubState": "s",
                   "Transient": "b", "DropInPaths": "as"}
TIMER_PROPERTIES = {"Unit": "s", "WakeSystem": "b", "AccuracyUSec": "t", "RandomizedDelayUSec": "t",
                    "TimersMonotonic": "a(stt)", "TimersCalendar": "a(sst)",
                    "NextElapseUSecMonotonic": "t", "NextElapseUSecRealtime": "t"}
EXEC_SIGNATURE = "a(sasbttttuii)"
EXTRA_COMMANDS = ("ExecCondition", "ExecStartPre", "ExecStartPost", "ExecReload", "ExecStop", "ExecStopPost")
SERVICE_PROPERTIES = {"Type": "s", "User": "s", "Group": "s", "Restart": "s", "RemainAfterExit": "b",
                      "TimeoutStartUSec": "t", "ExecStart": EXEC_SIGNATURE,
                      **{name: EXEC_SIGNATURE for name in EXTRA_COMMANDS}}


def decoded(properties: dict[str, object], signatures: dict[str, str]) -> dict[str, object]:
    if set(properties) != set(signatures):
        raise ValueError("guard property coverage mismatch")
    return {name: decode_property(properties[name], signature) for name, signature in signatures.items()}


def verify_guard_properties(operation: str, timer: dict[str, object], service: dict[str, object],
                            *, now_usec: int) -> int:
    name = policy.guard_name(operation)
    timer_values = decoded(timer, {**UNIT_PROPERTIES, **TIMER_PROPERTIES})
    service_values = decoded(service, {**UNIT_PROPERTIES, **SERVICE_PROPERTIES})
    for values, suffix, active, sub in ((timer_values, ".timer", "active", "waiting"),
                                        (service_values, ".service", "inactive", "dead")):
        if (values["Id"] != name + suffix or values["LoadState"] != "loaded"
                or values["Transient"] is not True or values["DropInPaths"] != []
                or values["ActiveState"] != active or values["SubState"] != sub):
            raise ValueError("guard identity or activation state mismatch")
    if (timer_values["Unit"] != name + ".service" or timer_values["AccuracyUSec"] != 1_000_000
            or timer_values["RandomizedDelayUSec"] != 0 or timer_values["NextElapseUSecRealtime"] != 0):
        raise ValueError("guard timer policy mismatch")
    if (service_values["Type"] != "oneshot" or service_values["Restart"] != "no"
            or service_values["User"] not in ("", "root") or service_values["Group"] not in ("", "root")
            or service_values["RemainAfterExit"] is not False or service_values["TimeoutStartUSec"] != 20_000_000
            or any(service_values[key] != [] for key in EXTRA_COMMANDS)):
        raise ValueError("guard service policy mismatch")
    expected = [["/usr/bin/systemctl", ["/usr/bin/systemctl", "start", policy.SOCKET], False,
                 0, 0, 0, 0, 0, 0, 0]]
    if service_values["ExecStart"] != expected:
        raise ValueError("guard command changed or already executed")
    return guard_remaining_seconds(specification=timer["TimersMonotonic"],
                                   deadline=timer["NextElapseUSecMonotonic"],
                                   wake_system=timer["WakeSystem"], now_usec=now_usec)
