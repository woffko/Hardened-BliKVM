"""Bounded, exact-unit JobRemoved observation for automatic restore tests.

systemd v249 busctl adds SERVICE operands as additional broad monitor matches;
never append them to this command. JSON mode excludes credential dump fields.
This module does not create timers or start any systemd unit.
"""

import json
import re
from typing import cast

from tools.status_outage_contract import guard_name
from tools.systemd_property import uint

MAX_EVENT_BYTES = 8192
MANAGER_PATH = "/org/freedesktop/systemd1"
MANAGER_INTERFACE = "org.freedesktop.systemd1.Manager"
SYSTEM_BUS_ADDRESS = "unix:path=/run/dbus/system_bus_socket"


def monitor_command(operation: str) -> list[str]:
    unit = guard_name(operation) + ".service"
    rule = ("type='signal',sender='org.freedesktop.systemd1',"
            f"path='{MANAGER_PATH}',interface='{MANAGER_INTERFACE}',"
            f"member='JobRemoved',arg2='{unit}'")
    return ["/usr/bin/busctl", "--system", "--address=" + SYSTEM_BUS_ADDRESS,
            "--no-pager", "--json=short", "--match=" + rule, "monitor"]


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for name, value in pairs:
        if name in result:
            raise ValueError("duplicate event field")
        result[name] = value
    return result


def object_value(value: object) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError("invalid restore event object")
    return cast(dict[str, object], value)


def parse_completed_job(raw: bytes, operation: str, expected_sender: str) -> dict[str, object]:
    """Validate a signal from the separately resolved systemd bus owner.

    The caller additionally binds reception to the armed timer's monotonic
    deadline and boot, and verifies serving state after the observed job.
    Realtime header timestamps are not used to decide guard timing.
    """
    unit = guard_name(operation) + ".service"
    if (not raw or len(raw) > MAX_EVENT_BYTES
            or re.fullmatch(r":[0-9]{1,10}\.[0-9]{1,10}", expected_sender) is None):
        raise ValueError("invalid restore event input")
    try:
        value = object_value(json.loads(raw.decode("ascii"), object_pairs_hook=unique_object))
        required = {"type", "endian", "flags", "version", "cookie", "timestamp-realtime",
                    "sender", "path", "interface", "member", "payload"}
        optional = {"destination", "monotonic", "realtime", "seqnum"}
        if not required <= set(value) <= required | optional:
            raise ValueError("unexpected restore event fields")
        if (value["type"] != "signal" or value["sender"] != expected_sender
                or value["path"] != MANAGER_PATH or value["interface"] != MANAGER_INTERFACE
                or value["member"] != "JobRemoved" or value["endian"] not in ("l", "B")
                or uint(value["version"], 8) != 1):
            raise ValueError("restore event origin mismatch")
        uint(value["flags"], 8)
        for name in ("cookie", "timestamp-realtime", "monotonic", "realtime", "seqnum"):
            if name in value:
                uint(value[name], 64)
        if "destination" in value:
            destination = value["destination"]
            if not isinstance(destination, str) or re.fullmatch(r":[0-9]{1,10}\.[0-9]{1,10}", destination) is None:
                raise ValueError("invalid restore event destination")
        payload = object_value(value["payload"])
        if set(payload) != {"type", "data"} or payload["type"] != "uoss" or not isinstance(payload["data"], list):
            raise ValueError("restore job signature mismatch")
        data = cast(list[object], payload["data"])
        if len(data) != 4:
            raise ValueError("restore job argument count mismatch")
        job_id = uint(data[0], 32)
        if job_id == 0 or data[1] != MANAGER_PATH + "/job/" + str(job_id) or data[2] != unit or data[3] != "done":
            raise ValueError("restore job did not complete successfully")
        return {"jobId": job_id, "unit": unit, "result": "done"}
    except (ValueError, TypeError, KeyError, RecursionError):
        raise ValueError("invalid or unsuccessful restore completion event") from None
