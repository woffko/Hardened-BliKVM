"""Strict decoding of the limited busctl JSON types used by outage probes.

This module does not issue D-Bus requests. It rejects unknown fields/types
without including property values in errors.
"""

from typing import cast

from tools.status_outage_contract import GUARD_SECONDS

MONOTONIC_BASES = {"OnActiveUSec", "OnBootUSec", "OnStartupUSec", "OnUnitActiveUSec", "OnUnitInactiveUSec"}


def uint(value: object, bits: int) -> int:
    if type(value) is not int or not 0 <= value < 2**bits:
        raise ValueError("invalid D-Bus unsigned integer")
    return value


def decode_property(value: object, signature: str) -> object:
    if not isinstance(value, dict):
        raise ValueError("invalid D-Bus property envelope")
    envelope = cast(dict[str, object], value)
    if set(envelope) != {"type", "data"} or envelope["type"] != signature:
        raise ValueError("D-Bus property signature mismatch")
    data = envelope["data"]
    if signature in ("u", "t"):
        return uint(data, 32 if signature == "u" else 64)
    if signature == "b":
        if type(data) is not bool:
            raise ValueError("invalid D-Bus boolean")
        return data
    if signature == "s":
        if not isinstance(data, str) or len(data) > 512 or any(ord(char) < 32 or ord(char) > 126 for char in data):
            raise ValueError("invalid D-Bus public string")
        return data
    if signature == "as":
        if not isinstance(data, list):
            raise ValueError("invalid D-Bus string array")
        strings = cast(list[object], data)
        if len(strings) > 32:
            raise ValueError("D-Bus string array too large")
        for item in strings:
            decode_property({"type": "s", "data": item}, "s")
        return strings
    if signature == "a(sst)":
        # The outage guard must not contain any calendar expressions.
        if data != []:
            raise ValueError("calendar expressions are not permitted for the guard")
        return []
    if signature == "a(sasbttttuii)":
        if not isinstance(data, list):
            raise ValueError("invalid D-Bus command array")
        commands = cast(list[object], data)
        if len(commands) > 16:
            raise ValueError("D-Bus command array too large")
        for command in commands:
            if not isinstance(command, list):
                raise ValueError("invalid D-Bus command record")
            fields = cast(list[object], command)
            if len(fields) != 10:
                raise ValueError("invalid D-Bus command record length")
            decode_property({"type": "s", "data": fields[0]}, "s")
            decode_property({"type": "as", "data": fields[1]}, "as")
            decode_property({"type": "b", "data": fields[2]}, "b")
            for item in fields[3:7]:
                uint(item, 64)
            uint(fields[7], 32)
            for item in fields[8:]:
                if type(item) is not int or not -(2**31) <= item < 2**31:
                    raise ValueError("invalid D-Bus command exit counter")
        return commands
    if signature == "a(stt)":
        if not isinstance(data, list):
            raise ValueError("invalid monotonic timer entries")
        entries = cast(list[object], data)
        if len(entries) > 16:
            raise ValueError("too many monotonic timer entries")
        for entry in entries:
            if not isinstance(entry, list):
                raise ValueError("invalid monotonic timer entry")
            fields = cast(list[object], entry)
            if len(fields) != 3:
                raise ValueError("invalid monotonic timer entry fields")
            if not isinstance(fields[0], str) or fields[0] not in MONOTONIC_BASES:
                raise ValueError("invalid monotonic timer base")
            uint(fields[1], 64)
            uint(fields[2], 64)
        return entries
    raise ValueError("unsupported D-Bus property signature")


def guard_remaining_seconds(*, specification: object, deadline: object,
                            wake_system: object, now_usec: int) -> int:
    """Validate one 120s OnActive timer using the device CLOCK_MONOTONIC.

    Unit identity, activation state, target service and command verification
    are separate required checks. Expired/uninitialized deadlines fail closed.
    """
    entries = cast(list[list[object]], decode_property(specification, "a(stt)"))
    next_usec = cast(int, decode_property(deadline, "t"))
    wake = decode_property(wake_system, "b")
    now = uint(now_usec, 64)
    if (wake is not False or now == 0 or len(entries) != 1
            or entries[0] != ["OnActiveUSec", GUARD_SECONDS * 1_000_000, next_usec]
            or not now < next_usec <= now + GUARD_SECONDS * 1_000_000):
        raise ValueError("invalid guard monotonic deadline")
    # Rounding down is conservative for the policy's minimum remaining margin.
    return (next_usec - now) // 1_000_000
