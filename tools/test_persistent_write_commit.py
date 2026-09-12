# White-box tests intentionally exercise the private backend's mutation guards.
# pyright: reportPrivateUsage=false
from contextlib import contextmanager
from dataclasses import asdict
from pathlib import Path
import unittest
from unittest.mock import MagicMock, patch

from tools import persistent_write_commit as commit


class CommitTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path('/var/lib') / (commit.native.contract.PREFIX + 'a' * 24)
        self.bound = commit.native.contract.EvidenceBinding('a' * 24, 'b' * 64, commit.native.contract.CONFIG_SHA256,
            'app-00000000000000000009-' + 'c' * 64, 9, 43)
        self.gates = tuple(commit.native.contract.GateEvidence(name, self.bound, 'd' * 64, True)
                           for name in sorted(commit.native.contract.REQUIRED_GATES))
        self.winner = commit.native.decisions.Decision(self.bound, 'committed', {g.gate: g.receipt_sha256 for g in self.gates})
        self.plan: dict[str, object] = {'phase': 'wired', 'binding': asdict(self.bound), 'cleanupStarted': False}

    def evidence(self, _root: Path, _binding: commit.native.contract.EvidenceBinding) -> tuple[commit.native.contract.GateEvidence, ...]:
        return self.gates

    def test_approval_failure_precedes_any_lock_or_device_access(self) -> None:
        with patch.object(commit.native.temporary.guard, 'locked') as lock, self.assertRaises(ValueError):
            commit.commit(self.root, self.evidence, retention_approved=False)
        lock.assert_not_called()

    def test_fresh_commit_keeps_evidence_and_lifecycle_inside_same_lock(self) -> None:
        held = [False]
        @contextmanager
        def lock(_root: Path):
            held[0] = True
            try: yield
            finally: held[0] = False
        def reader(_root: Path, binding: commit.native.contract.EvidenceBinding):
            self.assertTrue(held[0]); self.assertEqual(binding, self.bound)
            return self.gates
        actions = MagicMock(spec=commit._Backend)
        actions.verify_retained_and_detached.side_effect = lambda: self.assertTrue(held[0])
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked', side_effect=lock), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.decisions, 'read', return_value=None), \
             patch.object(commit.native.decisions, 'publish', return_value=self.winner), \
             patch.object(commit, '_Backend', return_value=actions):
            self.assertEqual(commit.commit(self.root, reader, retention_approved=True), self.winner)
        self.assertFalse(held[0])
        self.assertEqual([c[0] for c in actions.method_calls], ['verify_wired_and_preserved', 'record_committed',
            'detach_guard_dependencies', 'reload_manager', 'verify_retained_and_detached', 'disarm_recovery', 'verify_retained_and_detached'])

    def test_resume_does_not_republish_or_require_removed_guard_overrides(self) -> None:
        actions = MagicMock(spec=commit._Backend)
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked'), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.decisions, 'read', return_value=self.winner), \
             patch.object(commit.native.decisions, 'publish') as publish, \
             patch.object(commit, '_Backend', return_value=actions):
            commit.commit(self.root, self.evidence, retention_approved=True)
        publish.assert_not_called()
        actions.verify_wired_and_preserved.assert_not_called()
        actions.require_winner.assert_called_once()
        actions.detach_guard_dependencies.assert_called_once()

    def test_winner_mismatch_prevents_record_or_detach(self) -> None:
        with patch.object(commit.native.u, 'select_profile'):
            actions = commit._Backend(self.root, self.plan, self.bound, self.gates)
        for winner in (None, commit.native.decisions.Decision(self.bound, 'revoked', {}),
                       commit.native.decisions.Decision(self.bound, 'committed', {g.gate: 'e' * 64 for g in self.gates})):
            with patch.object(commit.native.decisions, 'read', return_value=winner), \
                 patch.object(commit.native, 'save') as save, \
                 patch.object(commit.native.files, 'detach_guard') as detach:
                with self.assertRaises(ValueError): actions.record_committed()
                with self.assertRaises(ValueError): actions.detach_guard_dependencies()
                save.assert_not_called(); detach.assert_not_called()

    def test_postpublication_failure_does_not_disarm_or_attempt_restore(self) -> None:
        actions = MagicMock(spec=commit._Backend)
        actions.verify_retained_and_detached.side_effect = ValueError('retained state differs')
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked'), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.decisions, 'read', return_value=None), \
             patch.object(commit.native.decisions, 'publish', return_value=self.winner) as publish, \
             patch.object(commit.native, 'restore') as restore, \
             patch.object(commit, '_Backend', return_value=actions), self.assertRaises(ValueError):
            commit.commit(self.root, self.evidence, retention_approved=True)
        publish.assert_called_once()
        actions.disarm_recovery.assert_not_called()
        restore.assert_not_called()

    def test_revoked_decision_rejected_before_evidence_reader(self) -> None:
        def reader(_root: Path, _binding: commit.native.contract.EvidenceBinding) -> tuple[commit.native.contract.GateEvidence, ...]:
            self.fail('revoked decision must not invoke evidence provider')
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked'), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.decisions, 'read', return_value=commit.native.decisions.Decision(self.bound, 'revoked', {})), \
             self.assertRaises(ValueError):
            commit.commit(self.root, reader, retention_approved=True)

    def test_revocation_during_evidence_read_prevents_publication(self) -> None:
        revoked = commit.native.decisions.Decision(self.bound, 'revoked', {})
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked'), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.u, 'select_profile'), \
             patch.object(commit.native.decisions, 'read', side_effect=[None, revoked, revoked]), \
             patch.object(commit.native.decisions, 'publish') as publish, \
             patch.object(commit.native, 'save') as save, self.assertRaises(ValueError):
            commit.commit(self.root, self.evidence, retention_approved=True)
        publish.assert_not_called()
        save.assert_not_called()

    def test_postdisarm_snapshot_is_required(self) -> None:
        actions = MagicMock(spec=commit._Backend)
        actions.verify_retained_and_detached.side_effect = [None, ValueError('post-disarm state differs')]
        with patch.object(commit.os, 'geteuid', return_value=0), \
             patch.object(commit.native.temporary.guard, 'locked'), \
             patch.object(commit.native, 'load', return_value=self.plan), \
             patch.object(commit.native.decisions, 'read', return_value=None), \
             patch.object(commit.native.decisions, 'publish', return_value=self.winner), \
             patch.object(commit.native, 'restore') as restore, \
             patch.object(commit, '_Backend', return_value=actions), self.assertRaises(ValueError):
            commit.commit(self.root, self.evidence, retention_approved=True)
        actions.disarm_recovery.assert_called_once()
        self.assertEqual(actions.verify_retained_and_detached.call_count, 2)
        restore.assert_not_called()


if __name__ == '__main__':
    unittest.main()
