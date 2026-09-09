"""Arm/verify/disarm a restore timer without ever stopping the status socket."""

from __future__ import annotations

import grp
import json
import os
from pathlib import Path
import pwd
import subprocess
import sys
import time
from typing import cast

from tools import observe_service_resources as resources
from tools import status_outage_contract as policy
from tools import status_restore_properties as metadata
from tools.observe_retained_backend_load import retained_status
from tools.systemd_property import decode_property

require = resources.require
WORKERS = "hardened-blikvm-update-status@*.service"


def command(argv: list[str], timeout: int = 5) -> bytes:
    result = subprocess.run(argv, capture_output=True, timeout=timeout, check=True)
    require(len(result.stdout) <= 32768, "oversize systemd reply")
    return result.stdout


def unit_path(unit: str) -> str:
    require(unit.isascii(), "invalid unit name")
    return "/org/freedesktop/systemd1/unit/" + "".join(
        char if char.isalnum() else "_" + format(ord(char), "02x") for char in unit)


def property_value(unit: str, interface: str, name: str, signature: str) -> dict[str, object]:
    raw = command(["busctl", "--system", "--json=short", "get-property", "org.freedesktop.systemd1",
                   unit_path(unit), "org.freedesktop.systemd1." + interface, name])
    value: object = json.loads(raw)
    decode_property(value, signature)
    return cast(dict[str, object], value)


def properties(unit: str, interface: str, selected: dict[str, str]) -> dict[str, object]:
    return {**{name: property_value(unit, "Unit", name, sig) for name, sig in metadata.UNIT_PROPERTIES.items()},
            **{name: property_value(unit, interface, name, sig) for name, sig in selected.items()}}


def own_timer_identity(unit: str) -> bool:
    return (decode_property(property_value(unit, "Unit", "Id", "s"), "s") == unit
            and decode_property(property_value(unit, "Unit", "Transient", "b"), "b") is True)


def unit_absent(unit: str) -> bool:
    result = subprocess.run(["systemctl", "show", unit, "--property=LoadState", "--value"],
                            capture_output=True, timeout=5, check=False)
    require(result.returncode in (0, 1) and len(result.stdout) <= 64, "unit presence query failed")
    value = result.stdout.strip()
    require(value in (b"loaded", b"not-found"), "unexpected unit load state")
    return value == b"not-found"


def namespace_pristine(operation: str) -> None:
    name = policy.guard_name(operation)
    raw = command(["busctl", "--system", "--json=short", "get-property", "org.freedesktop.systemd1",
                   "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager", "UnitPath"])
    roots = cast(list[str], decode_property(json.loads(raw), "as"))
    require(bool(roots), "systemd unit search paths missing")
    candidates: set[str] = set()
    for kind in ("timer", "service"):
        unit = name + "." + kind
        candidates.update({unit, unit + ".d", unit + ".wants", unit + ".requires", unit + ".upholds", kind + ".d"})
        candidates.update(name[:index + 1] + "." + kind + ".d"
                          for index, char in enumerate(name) if char == "-")
    for root in roots:
        path = Path(root)
        require(path.is_absolute() and path != Path("/") and ".." not in path.parts,
                "unsafe systemd unit search path")
        require(not any(os.path.lexists(path / candidate) for candidate in candidates),
                "guard namespace has inherited configuration")


def workers_collected() -> bool:
    # Preserve failed-unit evidence. Historical retained instances deliberately
    # fail this gate before arming, rather than being reset or ignored.
    return not command(["systemctl", "list-units", "--all", "--plain",
                        "--no-legend", "--no-pager", WORKERS]).strip()


def quiet_socket() -> None:
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        for interface, name, sig, expected in (("Unit", "ActiveState", "s", "active"),
                                               ("Unit", "SubState", "s", "listening"),
                                               ("Socket", "Accept", "b", True),
                                               ("Socket", "MaxConnections", "u", 4)):
            require(decode_property(property_value(policy.SOCKET, interface, name, sig), sig) == expected,
                    "status socket baseline mismatch")
        count = decode_property(property_value(policy.SOCKET, "Socket", "NConnections", "u"), "u")
        if count == 0 and workers_collected():
            return
        time.sleep(0.25)
    raise ValueError("status workers did not become idle")


def status_read(source: str, profile_name: str = "seq5") -> dict[str, object]:
    resources.status_profile(profile_name)
    uid, gid = pwd.getpwnam("hardened-blikvm-server").pw_uid, grp.getgrnam("hardened-blikvm").gr_gid
    require(uid > 0 and gid > 0, "invalid status peer identity")
    result = subprocess.run([sys.executable, "-B", "-c", source], user=uid, group=gid, extra_groups=[],
                            env={}, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, timeout=8, check=True)
    require(len(result.stdout) <= 20000, "oversize status reply")
    return retained_status(json.loads(result.stdout), profile_name)


def same_identity(before: resources.Sample, after: resources.Sample) -> None:
    require(before["bootIdSha256"] == after["bootIdSha256"], "boot identity changed")
    for unit in resources.UNITS:
        require(all(before["units"][unit][key] == after["units"][unit][key]
                    for key in ("mainPid", "mainStartTicks", "restarts")), "serving identity changed")


def probe(operation: str, status_source: str) -> dict[str, object]:
    name = policy.guard_name(operation)
    phase = "baseline"
    attempted = removed = False
    result: dict[str, object] = {"result": "failed", "operation": operation}
    try:
        require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected probe host")
        before = resources.snapshot()
        before_status = status_read(status_source)
        quiet_socket()
        phase = "namespace"
        namespace_pristine(operation)
        unused = unit_absent(name + ".timer") and unit_absent(name + ".service")
        argv = policy.arm_command(operation, namespace_unused=unused,
                                  observation=policy.OutageObservation(True, True, 0, False, 0))
        phase = "arm"
        attempted = True
        command(argv, timeout=15)
        phase = "verify"
        timer = properties(name + ".timer", "Timer", metadata.TIMER_PROPERTIES)
        service = properties(name + ".service", "Service", metadata.SERVICE_PROPERTIES)
        remaining = metadata.verify_guard_properties(operation, timer, service,
                                                       now_usec=time.monotonic_ns() // 1000)
        require(remaining >= policy.MIN_REMAINING_SECONDS, "guard margin too short")
        phase = "restore-noop"
        command(policy.restore_command(), timeout=15)
        require(status_read(status_source) == before_status, "application status changed")
        after = resources.snapshot()
        same_identity(before, after)
        quiet_socket()
        phase = "disarm"
        # This probe never calls stop_command. Removing the owned timer cannot
        # strand an outage created by this probe; no restorer service is stopped.
        if not unit_absent(name + ".timer"):
            try:
                command(policy.disarm_command(operation, observation=policy.OutageObservation(True, True, 0, True, remaining),
                                              restored_status_verified=True))
            except subprocess.CalledProcessError:
                # Collection can race the stop call. Only verified absence of
                # this exact timer makes that particular failure harmless.
                require(unit_absent(name + ".timer"), "timer stop failed")
        phase = "cleanup"
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if unit_absent(name + ".timer") and unit_absent(name + ".service"):
                removed = True
                break
            time.sleep(0.25)
        require(removed, "guard units were not collected")
        same_identity(before, resources.snapshot())
        require(status_read(status_source) == before_status, "final application status changed")
        quiet_socket()
        result = {"result": "passed", "operation": operation, "guardVerified": True,
                  "guardRemoved": True, "statusUnchanged": True, "servingIdentityUnchanged": True,
                  "statusSocketNeverStopped": True, "initialGuardMarginSeconds": remaining}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        result = {"result": "failed", "operation": operation, "phase": phase, "errorType": type(error).__name__}
    finally:
        if attempted and not removed:
            # This arm/disarm-only probe never created a status outage. Cancel
            # its own transient timer even after metadata validation failed,
            # but never stop a foreign unit or any restoration service.
            try:
                if not unit_absent(name + ".timer"):
                    require(own_timer_identity(name + ".timer"), "timer ownership could not be verified")
                    command(["systemctl", "stop", name + ".timer"])
                removed = unit_absent(name + ".timer") and unit_absent(name + ".service")
            except (OSError, ValueError, subprocess.SubprocessError):
                removed = False
        result["guardMayRemain"] = attempted and not removed
    return result
