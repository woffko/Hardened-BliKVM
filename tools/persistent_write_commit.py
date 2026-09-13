"""Native commit backend, with no CLI or production evidence-reader yet.

Only a privileged provider that validates original bytes, archive custody and
current qualification may supply evidence. Compatibility objects and arbitrary
client hashes are not a provider. No recovery implementation is modified here.
"""

from __future__ import annotations

from collections.abc import Callable
import os
from pathlib import Path

if not __package__:
    import installed_persistent_write as native
else:
    from tools import installed_persistent_write as native


class _Backend:
    def __init__(self, root: Path, plan: dict[str, object], binding: native.contract.EvidenceBinding,
                 evidence: tuple[native.contract.GateEvidence, ...]):
        self.root, self.plan, self.binding = root, dict(plan), binding
        self.receipts = {item.gate: item.receipt_sha256 for item in evidence}
        self.terminal = native.contract.temporary.TerminalState(binding.baseline_slot,
            binding.baseline_floor, binding.baseline_generation)
        native.u.select_profile('dev7-dev9')

    def require_winner(self) -> native.decisions.Decision:
        winner = native.decisions.read(self.root)
        native.require(winner is not None and winner.outcome == 'committed'
            and winner.binding == self.binding and winner.receipt_hashes == self.receipts,
            'durable winner differs from verified qualification')
        assert winner is not None
        return winner

    def verify_wired_and_preserved(self) -> None:
        native.require(self.plan['phase'] == 'wired' and native.decisions.read(self.root) is None,
                       'new commit requires undecided wired policy')
        native.validate_guards(self.root, enabled=True)
        native.require(native.s.property_value(self.root.name + '-deadline.timer', 'ActiveState') == 'active'
            and native.s.property_value(self.root.name + '-boot.service', 'ActiveState') == 'active',
            'recovery is not armed')
        native.verify_runtime(self.root, self.plan, self.terminal, wired=True, masked=False)

    def record_committed(self) -> None:
        self.require_winner()
        self.plan['phase'] = native.contract.Phase.COMMITTED.value
        native.save(self.root, self.plan)

    def detach_guard_dependencies(self) -> None:
        self.require_winner()
        native.files.detach_guard(self.root, native.object_value(self.plan['backup']))

    def reload_manager(self) -> None:
        self.require_winner()
        native.systemd.Admission(self.root).reload_manager()

    def verify_retained_and_detached(self) -> None:
        self.require_winner()
        if not __package__:
            import persistent_write_retained_snapshot as retained
        else:
            from tools import persistent_write_retained_snapshot as retained
        after = retained.capture(self.root, native.object_value(self.plan['backup']), self.terminal)
        native.snapshot.preserve(native.object_value(self.plan['before']), after,
            new_boot=self.plan['boot'] != native.boot_identity(), allow_transition=False)

    def disarm_recovery(self) -> None:
        self.require_winner()
        native.Restore(self.root, self.plan).disarm_recovery()


def commit(root: Path, read_verified_evidence: Callable[[Path, native.contract.EvidenceBinding],
           tuple[native.contract.GateEvidence, ...]], *, retention_approved: bool) -> native.decisions.Decision:
    """Keep the common lock across trusted evidence reading and all commit work.

    A failure after decision publication is NOT permission to revoke/overwrite
    the committed winner. Re-enter only with the same persisted qualification;
    no branch calls restore or signals an update worker.
    """
    native.require(retention_approved is True, 'explicit retention approval required')
    native.require(os.geteuid() == 0, 'privileged commit provider required')
    native.contract.guard_units(root)
    with native.temporary.guard.locked(root):
        plan = native.load(root)
        phase = native.contract.Phase(plan['phase'])
        native.require(phase in (native.contract.Phase.WIRED, native.contract.Phase.COMMITTED)
            and plan['cleanupStarted'] is False, 'policy not eligible for commit')
        binding = native.binding(plan)
        prior = native.decisions.read(root)
        native.require(prior is None or (prior.outcome == 'committed' and prior.binding == binding),
                       'policy was revoked or decision binding differs')
        native.require(prior is not None or phase is native.contract.Phase.WIRED,
                       'committed marker without durable decision')
        evidence = read_verified_evidence(root, binding)
        native.contract.require_commit(phase, evidence, current=binding, explicit_retention_approved=True)
        actions = _Backend(root, plan, binding, evidence)
        winner = native.decisions.read(root)
        if winner is not None:
            # A committed decision can precede a stale phase marker or partial
            # detachment. Exact owned removal is resumable; fresh wiring checks
            # would incorrectly require already-detached transaction drop-ins.
            actions.require_winner()
            actions.record_committed()
            actions.detach_guard_dependencies()
            actions.reload_manager()
            actions.verify_retained_and_detached()
            actions.disarm_recovery()
        else:
            native.require(phase is native.contract.Phase.WIRED, 'committed marker without durable decision')
            winner = native.lifecycle.commit(root, actions, phase=phase, binding=binding,
                evidence=evidence, retention_approved=True)
        # Disarm removes enable links and reloads systemd. This is a separate
        # post-mutation boundary, not a redundant copy of the pre-disarm check.
        actions.verify_retained_and_detached()
        return winner
