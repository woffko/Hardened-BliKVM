"""Ordered controller operations with an atomic commit/revoke decision.

The installed backend owns privileged checks and actions. These operations
never interpret a failed or missing receipt as acceptance.
"""

from collections.abc import Callable
from contextlib import AbstractContextManager
from pathlib import Path
from typing import Protocol

if not __package__:
    import persistent_write_contract as contract
    import persistent_write_decision as decisions
else:
    from tools import persistent_write_contract as contract
    from tools import persistent_write_decision as decisions


class CommitBackend(Protocol):
    def verify_wired_and_preserved(self) -> None: ...
    def record_committed(self) -> None: ...
    def detach_guard_dependencies(self) -> None: ...
    def reload_manager(self) -> None: ...
    def verify_retained_and_detached(self) -> None: ...
    def disarm_recovery(self) -> None: ...


def commit(root: Path, actions: CommitBackend, *, phase: contract.Phase,
           binding: contract.EvidenceBinding, evidence: tuple[contract.GateEvidence, ...],
           retention_approved: bool) -> decisions.Decision:
    # A native caller MUST hold its common controller lock continuously from
    # evidence/runtime validation through publication and detachment. This
    # also excludes locked emergency closure when decision storage has failed.
    contract.require_commit(phase, evidence, current=binding, explicit_retention_approved=retention_approved)
    actions.verify_wired_and_preserved()
    proposal = decisions.Decision(binding, "committed", {item.gate: item.receipt_sha256 for item in evidence})
    winner = decisions.publish(root, proposal)
    contract.temporary.require(winner.outcome == "committed", "deadline already revoked write retention")
    # decision.json is authoritative if a crash precedes this secondary marker.
    actions.record_committed()
    actions.detach_guard_dependencies()
    actions.reload_manager()
    actions.verify_retained_and_detached()
    actions.disarm_recovery()
    return winner


class RecoveryBackend(contract.RuntimeDisable, contract.BootDisable, Protocol):
    def fence_admission(self, *, boot: bool) -> None: ...
    def lock(self) -> AbstractContextManager[object]: ...
    def phase(self) -> contract.Phase: ...
    def record_restored(self, *, boot: bool) -> None: ...
    def release_admission_fence(self) -> None: ...
    def disarm_recovery(self) -> None: ...


def recover(root: Path, actions: RecoveryBackend, *, binding: contract.EvidenceBinding,
            boot: bool, publish: Callable[[Path, decisions.Decision], decisions.Decision] = decisions.publish) -> None:
    winner = publish(root, decisions.Decision(binding, "revoked", {}))
    if winner.outcome == "committed":
        return  # Never close already-committed admission from a late guard.
    # Runtime masks plus stop/cancel prevent a delayed preparation process from
    # reopening sockets. The backend must leave this fence on every failure.
    actions.fence_admission(boot=boot)
    with actions.lock():
        if actions.phase() is contract.Phase.RESTORED:
            if boot:
                actions.verify_owned_configuration_absent()
            else:
                actions.verify_closed_and_preserved()
        elif boot:
            contract.disable_boot(actions)
            actions.record_restored(boot=True)
        else:
            contract.disable_runtime(actions)
            actions.record_restored(boot=False)
        if boot:
            # Keep masks/recovery metadata until a separate post-boot runtime
            # verification. Never stop this running boot guard from itself.
            return
        # Remove the fence only after the old owner has released this lock and
        # the backend has verified no start jobs/owned preparers remain.
        actions.release_admission_fence()
        actions.disarm_recovery()
