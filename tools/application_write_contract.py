"""Closed rules for a temporary same-boot HTTP application-update gate.

No device operations or filesystem writes are performed here. A successful
activation receipt is not a confirmed application or permission to restart it.
"""

from dataclasses import dataclass
from pathlib import Path
import re
from typing import Protocol

if not __package__:
    from probe_installed_upload import PROFILES, SLOT
else:
    from tools.probe_installed_upload import PROFILES, SLOT

PREFIX = "hardened-blikvm-http-update-"
CURRENT = "hardened-blikvm-server-current.service"
CANDIDATE = "hardened-blikvm-server-candidate.service"
UPLOAD = "hardened-blikvm-update-upload.socket"
ACTIVATION = "hardened-blikvm-update-activation.socket"
WRITE_ENVIRONMENT = {
    "BLI_KVM_UPDATE_UPLOAD_SOCKET": "/run/hardened-blikvm-update-api/upload.sock",
    "BLI_KVM_UPDATE_ACTIVATION_SOCKET": "/run/hardened-blikvm-update-api/activation.sock",
    "BLI_KVM_UPDATE_WRITES_ENABLED": "1",
}
WRITE_DROPIN = "90-http-update-rehearsal.conf"
# No Wants/Requires: an apply-candidate/confirmed restart must not reopen
# admission after the restoration path has stopped these sockets.
WRITE_DROPIN_BYTES = ("[Service]\n" + "".join(
    f"Environment={name}={value}\n" for name, value in WRITE_ENVIRONMENT.items())).encode()
SOURCE_SLOT = SLOT
TARGET_SLOT = PROFILES["seq8-publish"].slot
BASE_GENERATION = 37


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


@dataclass(frozen=True)
class TerminalState:
    slot: str
    floor: int
    generation: int


def terminal_state(status: dict[str, object], *, observed_floor: int = 7,
                   require_target: bool = False) -> TerminalState:
    require(type(observed_floor) is int and observed_floor in (7, 8), "invalid observed floor")
    floor, generation = status.get("floorSequence"), status.get("generation")
    require(type(floor) is int and floor in (7, 8) and floor >= observed_floor, "confirmed floor regressed or escaped gate")
    require(type(generation) is int and generation >= BASE_GENERATION, "generation regressed")
    assert isinstance(floor, int) and isinstance(generation, int)
    require("pending" in status and status["pending"] is None, "activation has not settled")
    expected = SOURCE_SLOT if floor == 7 else TARGET_SLOT
    require(status.get("activeSlotId") == status.get("confirmedSlotId") == expected,
            "selected and confirmed pointers disagree")
    if require_target:
        require(floor == 8 and generation > BASE_GENERATION and status.get("lastRollback") is None
                and status.get("previousSlotId") == SOURCE_SLOT, "target confirmation not established")
    return TerminalState(expected, floor, generation)


def require_pipeline_idle(*, activation_workers: int, queued_jobs: int,
                          candidate_active: bool, health_token_present: bool,
                          confirmed_apply_present: bool) -> None:
    require(type(activation_workers) is int and activation_workers == 0
            and type(queued_jobs) is int and queued_jobs == 0
            and candidate_active is False and health_token_present is False
            and confirmed_apply_present is False, "activation or apply pipeline still outstanding")


class Restoration(Protocol):
    def close_admission(self) -> None: ...
    def drain_naturally(self) -> None: ...
    def await_terminal_pipeline(self) -> TerminalState: ...
    def remove_owned_overrides(self) -> None: ...
    def reload_and_restart_confirmed(self) -> None: ...
    def verify_closed(self, terminal: TerminalState) -> None: ...


def restore_runtime(actions: Restoration) -> TerminalState:
    """Called under one controller lock, also by deadline recovery.

The independent deadline can close admission before waiting for that lock.
Neither path may kill the activation worker or force journal rollback. A
failure leaves the owned guard for inspection; cleanup is a separate step
only after verify_closed succeeds. Do not disarm the deadline on entry.
"""
    actions.close_admission()
    actions.drain_naturally()
    terminal = actions.await_terminal_pipeline()
    actions.remove_owned_overrides()
    actions.reload_and_restart_confirmed()
    actions.verify_closed(terminal)
    return terminal


def guard_units(root: Path) -> dict[str, bytes]:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid HTTP update guard path")
    # Deadline action must close admission before attempting the common lock.
    # It must not terminate an unrelated/possibly PID-reused controller.
    service = ("[Unit]\nStartLimitIntervalSec=0\n[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=300\n"
               f"ExecStart=/usr/bin/python3 -B {root}/controller.py deadline --guard-root {root}\n")
    return {root.name + "-restore.service": service.encode(),
            root.name + "-deadline.timer": ("[Timer]\nOnActiveSec=600\nOnUnitInactiveSec=30\n"
                f"Unit={root.name}-restore.service\n").encode()}
