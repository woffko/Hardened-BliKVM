from contextlib import nullcontext
from pathlib import Path
import unittest
from unittest.mock import patch

from tools import persistent_write_lifecycle as lifecycle


class Actions:
    def __init__(self, phase: lifecycle.contract.Phase):
        self.current = phase
        self.events: list[str] = []
    def fence_admission(self, *, boot: bool) -> None: self.events.append("fence-boot" if boot else "fence")
    def lock(self) -> nullcontext[None]:
        self.events.append("lock")
        return nullcontext()
    def phase(self) -> lifecycle.contract.Phase: return self.current
    def close_admission(self) -> None: self.events.append("close")
    def await_terminal_pipeline(self) -> None: self.events.append("settle")
    def remove_owned_configuration(self) -> None: self.events.append("remove")
    def reload_and_restart_confirmed(self) -> None: self.events.append("restart")
    def verify_closed_and_preserved(self) -> None: self.events.append("verify-runtime")
    def cancel_socket_jobs_nonblocking(self) -> None: self.events.append("cancel-jobs")
    def reload_manager(self) -> None: self.events.append("reload")
    def verify_owned_configuration_absent(self) -> None: self.events.append("verify-files")
    def record_restored(self, *, boot: bool) -> None: self.events.append("record-boot" if boot else "record-runtime")
    def release_admission_fence(self) -> None: self.events.append("release-fence")
    def disarm_recovery(self) -> None: self.events.append("disarm")
    def verify_wired_and_preserved(self) -> None: self.events.append("verify-wired")
    def record_committed(self) -> None: self.events.append("record-commit")
    def detach_guard_dependencies(self) -> None: self.events.append("detach")
    def verify_retained_and_detached(self) -> None: self.events.append("verify-retained")


class LifecycleTests(unittest.TestCase):
    def setUp(self) -> None:
        self.binding = lifecycle.contract.EvidenceBinding("a" * 24, "b" * 64, lifecycle.contract.CONFIG_SHA256,
            "app-00000000000000000008-" + "c" * 64, 8, 40)
    def revoked(self, _root: Path, proposal: lifecycle.decisions.Decision) -> lifecycle.decisions.Decision:
        return proposal
    def test_late_deadline_after_commit_performs_no_actions(self) -> None:
        actions = Actions(lifecycle.contract.Phase.WIRED)
        def committed(_root: Path, _proposal: lifecycle.decisions.Decision) -> lifecycle.decisions.Decision:
            return lifecycle.decisions.Decision(self.binding, "committed", dict.fromkeys(lifecycle.contract.REQUIRED_GATES, "d" * 64))
        lifecycle.recover(Path("/unused"), actions, binding=self.binding, boot=False, publish=committed)
        self.assertEqual(actions.events, [])
    def test_already_restored_does_not_restart_server(self) -> None:
        actions = Actions(lifecycle.contract.Phase.RESTORED)
        lifecycle.recover(Path("/unused"), actions, binding=self.binding, boot=False, publish=self.revoked)
        self.assertEqual(actions.events, ["fence", "lock", "verify-runtime", "release-fence", "disarm"])
    def test_runtime_and_boot_paths_keep_distinct_ordering(self) -> None:
        for boot in (False, True):
            actions = Actions(lifecycle.contract.Phase.WIRED)
            lifecycle.recover(Path("/unused"), actions, binding=self.binding, boot=boot, publish=self.revoked)
            middle = ["cancel-jobs", "remove", "reload", "cancel-jobs", "verify-files", "record-boot"] if boot else [
                "close", "settle", "remove", "restart", "verify-runtime", "record-runtime"]
            expected = ["fence-boot", "lock", *middle] if boot else ["fence", "lock", *middle, "release-fence", "disarm"]
            self.assertEqual(actions.events, expected)

    def test_commit_records_atomic_winner_before_detaching_guard(self) -> None:
        actions = Actions(lifecycle.contract.Phase.WIRED)
        evidence = tuple(lifecycle.contract.GateEvidence(name, self.binding, "d" * 64, True)
                         for name in sorted(lifecycle.contract.REQUIRED_GATES))
        def publish(_root: Path, proposal: lifecycle.decisions.Decision) -> lifecycle.decisions.Decision:
            actions.events.append("publish-commit")
            return proposal
        with patch.object(lifecycle.decisions, "publish", side_effect=publish):
            lifecycle.commit(Path("/unused"), actions, phase=actions.current, binding=self.binding,
                             evidence=evidence, retention_approved=True)
        self.assertEqual(actions.events, ["verify-wired", "publish-commit", "record-commit", "detach", "reload", "verify-retained", "disarm"])

    def test_revoked_winner_prevents_commit_marker_or_disarm(self) -> None:
        actions = Actions(lifecycle.contract.Phase.WIRED)
        evidence = tuple(lifecycle.contract.GateEvidence(name, self.binding, "d" * 64, True)
                         for name in sorted(lifecycle.contract.REQUIRED_GATES))
        with patch.object(lifecycle.decisions, "publish", return_value=lifecycle.decisions.Decision(self.binding, "revoked", {})):
            with self.assertRaises(ValueError):
                lifecycle.commit(Path("/unused"), actions, phase=actions.current, binding=self.binding,
                                 evidence=evidence, retention_approved=True)
        self.assertEqual(actions.events, ["verify-wired"])
