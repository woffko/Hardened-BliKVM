"""Closed policy for preparing persistent GUI update admission.

Pure decisions and unit rendering only. Importing this module never changes
device configuration, enables a socket, or approves retention.
"""

from dataclasses import dataclass
from enum import Enum
import hashlib
from pathlib import Path
import re
from typing import Protocol

if not __package__:
    import application_write_contract as temporary
else:
    from tools import application_write_contract as temporary

PREFIX = "hardened-blikvm-write-retain-"
DROPIN_NAME = "90-application-update-writes.conf"
TRANSACTION_DROPIN_NAME = "89-write-policy-transaction.conf"
SOCKETS = (temporary.UPLOAD, temporary.ACTIVATION)
SERVERS = (temporary.CURRENT, temporary.CANDIDATE)
# Boot admission is owned by explicit sockets.target enable links. A server
# restart must NEVER reopen admission after disable/recovery stopped sockets.
DROPIN_BYTES = ("[Unit]\nAfter=" + " ".join(SOCKETS) + "\n").encode() + temporary.WRITE_DROPIN_BYTES
CONFIG_SHA256 = hashlib.sha256(DROPIN_BYTES).hexdigest()
REQUIRED_GATES: frozenset[str] = frozenset(("host", "native_upload", "native_activation", "gui_happy_path",
                            "gui_failure_retry", "gui_cancel", "gui_logout", "boot_restore", "disable_drain"))


class Phase(str, Enum):
    PREPARED = "prepared"
    WIRED = "wired"
    RESTORED = "restored"
    COMMITTED = "committed"
    DISABLED = "disabled"


@dataclass(frozen=True)
class RecoveryDecision:
    close_admission: bool
    drain_workers: bool
    remove_owned_configuration: bool
    restart_confirmed: bool
    phase: Phase


def recovery_decision(phase: object, *, boot: bool) -> RecoveryDecision:
    if not isinstance(phase, Phase) or type(boot) is not bool:
        raise ValueError("invalid write recovery context")
    if phase in (Phase.COMMITTED, Phase.RESTORED, Phase.DISABLED):
        # Durable commit precedes disarm. A late deadline/boot invocation may
        # remove its own guard, but must not undo accepted persistent policy.
        # A permanently disabled policy is already closed; late recovery must
        # neither reopen admission nor treat disable as uncommitted preparation.
        return RecoveryDecision(False, False, False, False, phase)
    # Before serving at boot: cancel queued socket jobs non-blockingly. Never
    # synchronously start or wait for a service ordered after this boot guard.
    return RecoveryDecision(True, not boot, True, not boot, Phase.RESTORED)


@dataclass(frozen=True)
class EvidenceBinding:
    transaction_id: str
    device_fingerprint: str
    configuration_sha256: str
    baseline_slot: str
    baseline_floor: int
    baseline_generation: int

    def validate(self) -> None:
        temporary.require(re.fullmatch(r"[a-f0-9]{24}", self.transaction_id) is not None, "invalid evidence transaction")
        temporary.require(re.fullmatch(r"[a-f0-9]{64}", self.device_fingerprint) is not None, "invalid evidence device")
        temporary.require(self.configuration_sha256 == CONFIG_SHA256, "unreviewed write configuration")
        temporary.require(type(self.baseline_floor) is int and self.baseline_floor > 0
                          and type(self.baseline_generation) is int and self.baseline_generation > 0, "invalid evidence counters")
        temporary.require(re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", self.baseline_slot) is not None
                          and int(self.baseline_slot.split("-", 2)[1]) == self.baseline_floor, "invalid evidence slot")


@dataclass(frozen=True)
class GateEvidence:
    gate: str
    binding: EvidenceBinding
    receipt_sha256: str
    passed: bool


def require_commit(phase: object, gates: tuple[GateEvidence, ...], *, current: EvidenceBinding,
                   explicit_retention_approved: bool) -> Phase:
    temporary.require(isinstance(phase, Phase) and phase in (Phase.WIRED, Phase.COMMITTED), "write configuration not wired")
    current.validate()
    temporary.require(explicit_retention_approved is True, "permanent write retention is not approved")
    temporary.require(len(gates) == len(REQUIRED_GATES) and frozenset(value.gate for value in gates) == REQUIRED_GATES,
                      "persistent write gate set incomplete or duplicated")
    for value in gates:
        value.binding.validate()
        temporary.require(value.passed is True and value.binding == current
                          and re.fullmatch(r"[a-f0-9]{64}", value.receipt_sha256) is not None,
                          "persistent write evidence is stale, mismatched or incomplete")
    # The root adapter must read and validate each referenced receipt before
    # constructing GateEvidence; user-supplied hashes alone are not proof.
    return Phase.COMMITTED


def enable_links() -> dict[Path, Path]:
    return {Path("/etc/systemd/system/sockets.target.wants") / unit: Path("/lib/systemd/system") / unit
            for unit in SOCKETS}


def dropins() -> tuple[Path, ...]:
    return tuple(Path("/etc/systemd/system") / (unit + ".d") / DROPIN_NAME for unit in SERVERS)


def transaction_dropins(root: Path) -> dict[Path, bytes]:
    guard_units(root)  # Validate the exact guard namespace.
    unit = root.name + "-boot.service"
    data = f"[Unit]\nRequires={unit}\nAfter={unit}\n".encode()
    return {Path("/etc/systemd/system") / (name + ".d") / TRANSACTION_DROPIN_NAME: data
            for name in (*SERVERS, *SOCKETS)}


class RuntimeDisable(Protocol):
    def close_admission(self) -> None: ...
    def await_terminal_pipeline(self) -> None: ...
    def remove_owned_configuration(self) -> None: ...
    def reload_and_restart_confirmed(self) -> None: ...
    def verify_closed_and_preserved(self) -> None: ...


def disable_runtime(actions: RuntimeDisable) -> None:
    # Never restore an old journal/current pointer or delete a signed slot.
    # In-flight activation belongs to the manager until its natural settlement.
    actions.close_admission()
    actions.await_terminal_pipeline()
    actions.remove_owned_configuration()
    actions.reload_and_restart_confirmed()
    actions.verify_closed_and_preserved()


class BootDisable(Protocol):
    def cancel_socket_jobs_nonblocking(self) -> None: ...
    def remove_owned_configuration(self) -> None: ...
    def reload_manager(self) -> None: ...
    def verify_owned_configuration_absent(self) -> None: ...


def disable_boot(actions: BootDisable) -> None:
    actions.cancel_socket_jobs_nonblocking()
    actions.remove_owned_configuration()
    actions.reload_manager()
    actions.cancel_socket_jobs_nonblocking()
    actions.verify_owned_configuration_absent()


def guard_units(root: Path) -> dict[str, bytes]:
    temporary.require(root.parent == Path("/var/lib")
                      and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None, "invalid persistent write guard")
    unit = "[Unit]\nStartLimitIntervalSec=0\n"
    service = "[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=300\nNoNewPrivileges=yes\n"
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py restore --guard-root {root}"
    # The controller is deliberately not provided here: these artifacts cannot
    # be installed until a reviewed implementation and all gates exist.
    return {
        root.name + "-boot.service": (
            unit + "Description=Close uncommitted GUI update admission before serving\nDefaultDependencies=no\n"
            "After=local-fs.target\n" + f"RequiresMountsFor={root}\n"
            + "Before=sysinit.target sockets.target " + " ".join((*SOCKETS, *SERVERS)) + " shutdown.target\n"
            + "Conflicts=shutdown.target\n" + service + "RemainAfterExit=yes\n"
            + command + " --boot\n[Install]\nWantedBy=sysinit.target\n").encode(),
        root.name + "-restore.service": (unit + service + command + "\n").encode(),
        root.name + "-deadline.timer": ("[Timer]\nOnActiveSec=600\nOnUnitInactiveSec=30\n"
            + f"Unit={root.name}-restore.service\n[Install]\nWantedBy=timers.target\n").encode(),
    }


def preparer_unit(root: Path) -> tuple[str, bytes]:
    guard_units(root)
    # Only the owned configuration preparer may be bounded by this cgroup.
    # Upload/activation workers are separate installed services, never children
    # of this unit. Recovery waits for this entire unit to become quiescent.
    data = ("[Unit]\nStartLimitIntervalSec=0\n[Service]\nType=oneshot\nUser=root\nUMask=0077\n"
            "KillMode=control-group\nTimeoutStartSec=180\nTimeoutStopSec=15\nSendSIGKILL=yes\n"
            "NoNewPrivileges=yes\n"
            f"ExecStart=/usr/bin/python3 -B {root}/controller.py wire --guard-root {root}\n")
    return root.name + "-prepare.service", data.encode()
