"""Closed transaction decisions and unit rendering for persistent status wiring.

This module performs no filesystem writes, systemd calls or device operations.
"""

from dataclasses import dataclass
from enum import Enum
from pathlib import Path
import re


PREFIX = "hardened-blikvm-status-retain-"
CURRENT = "hardened-blikvm-server-current.service"
CANDIDATE = "hardened-blikvm-server-candidate.service"
SOCKET = "hardened-blikvm-update-status.socket"


class Phase(str, Enum):
    PREPARED = "prepared"
    WIRED = "wired"
    RESTORED = "restored"
    COMMITTED = "committed"


@dataclass(frozen=True)
class RecoveryDecision:
    remove_dropins: bool
    restart_server: bool
    stop_socket: bool
    verify_runtime_now: bool
    phase: Phase


def recovery_decision(phase: object, *, boot: bool) -> RecoveryDecision:
    if not isinstance(phase, Phase):
        raise ValueError("invalid persistent status phase")
    if phase is Phase.COMMITTED:
        # A committed marker is durable before disarm. Never undo retention
        # merely because a deadline or boot guard survived interrupted disarm.
        return RecoveryDecision(False, False, False, not boot, Phase.COMMITTED)
    # Early boot must not synchronously start a unit ordered after this guard.
    # Stop is non-blocking during boot: cancel a socket start already queued
    # from the old Wants dependency before daemon-reload removed the drop-in.
    return RecoveryDecision(True, not boot, True, not boot, Phase.RESTORED)


def require_commit(phase: Phase, *, native_verified: bool, browser_verified: bool) -> Phase:
    if phase not in (Phase.WIRED, Phase.COMMITTED) or not native_verified or not browser_verified:
        raise ValueError("persistent status retention is not accepted")
    return Phase.COMMITTED


def guard_units(root: Path) -> dict[str, str]:
    if root.parent != Path("/var/lib") or re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is None:
        raise ValueError("invalid persistent status guard namespace")
    common = "[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=120\nNoNewPrivileges=yes\n"
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py restore --guard-root {root}"
    return {
        root.name + "-boot.service": (
            "[Unit]\nDescription=Restore uncommitted status wiring before serving\n"
            "DefaultDependencies=no\nAfter=local-fs.target\n"
            f"RequiresMountsFor={root}\n"
            f"Before=sysinit.target {SOCKET} {CURRENT} {CANDIDATE} shutdown.target\n"
            "Conflicts=shutdown.target\n" + common + command + " --boot\n"
            "[Install]\nWantedBy=sysinit.target\n"
        ),
        root.name + "-restore.service": common + command + "\n",
        root.name + "-deadline.timer": (
            "[Timer]\nOnActiveSec=600\n"
            f"Unit={root.name}-restore.service\n[Install]\nWantedBy=timers.target\n"
        ),
    }
