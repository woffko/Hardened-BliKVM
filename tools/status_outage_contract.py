"""Closed command policy for a status-only outage acceptance guard.

Pure policy: no subprocesses, filesystem writes or device operations. The
coordinator must obtain fresh observations before requesting each command.
"""

from dataclasses import dataclass
import re

SOCKET = "hardened-blikvm-update-status.socket"
PREFIX = "hardened-blikvm-status-outage-"
GUARD_SECONDS = 120
MIN_REMAINING_SECONDS = 60


def guard_name(operation: str) -> str:
    if re.fullmatch(r"[a-f0-9]{24}", operation) is None:
        raise ValueError("invalid status outage operation")
    return PREFIX + operation


@dataclass(frozen=True)
class OutageObservation:
    baseline_matches: bool
    socket_active: bool
    active_workers: int
    guard_verified: bool
    guard_remaining_seconds: int


def validate_observation(value: object) -> None:
    if (not isinstance(value, OutageObservation)
            or any(type(flag) is not bool for flag in (
                value.baseline_matches, value.socket_active, value.guard_verified))
            or type(value.active_workers) is not int or not 0 <= value.active_workers <= 4
            or type(value.guard_remaining_seconds) is not int
            or not 0 <= value.guard_remaining_seconds <= GUARD_SECONDS):
        raise ValueError("invalid status outage observation")


def arm_command(operation: str, *, namespace_unused: bool,
                observation: OutageObservation) -> list[str]:
    name = guard_name(operation)
    validate_observation(observation)
    if (namespace_unused is not True or not observation.baseline_matches
            or not observation.socket_active or observation.active_workers != 0
            or observation.guard_verified or observation.guard_remaining_seconds != 0):
        raise ValueError("status outage baseline is not ready")
    # No shell, staging script, server/video restart, enable/disable or reboot.
    # The transient timer belongs to PID 1 and survives controller/SSH loss.
    return ["systemd-run", "--quiet", "--collect", "--unit=" + name,
            "--on-active=" + str(GUARD_SECONDS) + "s", "--timer-property=AccuracySec=1s",
            "--property=Type=oneshot", "--property=TimeoutStartSec=20s",
            "/usr/bin/systemctl", "start", SOCKET]


def stop_command(observation: OutageObservation) -> list[str]:
    validate_observation(observation)
    if (not observation.baseline_matches or not observation.socket_active
            or observation.active_workers != 0 or not observation.guard_verified
            or observation.guard_remaining_seconds < MIN_REMAINING_SECONDS):
        raise ValueError("verified restore guard required before status outage")
    return ["systemctl", "stop", SOCKET]


def restore_command() -> list[str]:
    # Restoration must remain available even after a failed observation.
    return ["systemctl", "start", SOCKET]


def disarm_command(operation: str, *, observation: OutageObservation,
                   restored_status_verified: bool) -> list[str]:
    name = guard_name(operation)
    validate_observation(observation)
    if (not observation.baseline_matches or not observation.socket_active
            or observation.active_workers != 0 or restored_status_verified is not True
            or not observation.guard_verified):
        raise ValueError("verified status restoration required before disarm")
    # Stop only the verified timer. Never cancel a restoration service already
    # running: allow it to complete its idempotent start of the fixed socket.
    return ["systemctl", "stop", name + ".timer"]
