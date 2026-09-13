from contextlib import ExitStack, nullcontext
from collections.abc import Callable
from dataclasses import asdict
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from typing import cast
from unittest.mock import MagicMock, patch

from tools import installed_persistent_write as native


class ControllerTests(unittest.TestCase):
    def test_unknown_preparation_profile_rejected_before_filesystem_access(self) -> None:
        with self.assertRaisesRegex(ValueError, 'unreviewed'):
            native.prepare(Path('/not-an-asset'), Path('/not-a-guard'), application_profile='seq11')

    def setUp(self) -> None:
        self.root = Path("/var/lib") / (native.contract.PREFIX + "a" * 24)
        self.bound = native.contract.EvidenceBinding("a" * 24, "b" * 64, native.contract.CONFIG_SHA256,
            "app-00000000000000000008-" + "c" * 64, 8, 40)
        self.plan: dict[str, object] = {"phase": "prepared", "binding": asdict(self.bound),
            "backup": {}, "boot": "d" * 64, "observed": {"floor": 8, "generation": 40}, "terminal": None,
            "cleanupStarted": False, "disableStarted": False}

    def test_staged_controller_import_closure_and_retention_cli(self) -> None:
        source = Path(native.__file__).parent
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in native.HELPERS:
                (root / name).write_bytes((source / name).read_bytes())
            (root / "controller.py").write_bytes(Path(native.__file__).read_bytes())
            result = subprocess.run([sys.executable, "-B", str(root / "controller.py"), "--help"],
                capture_output=True, timeout=5, check=False)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertIn(b"{prepare,wire,restore,cleanup,disable,evidence,commit}", result.stdout)
            self.assertIn(b"--approve-retention", result.stdout)
            self.assertIn(b"--evidence-seal", result.stdout)

    def test_boot_initial_arm_requires_absent_policy_and_no_decision(self) -> None:
        cgroup = "0::/system.slice/" + self.root.name + "-boot.service\n"
        with patch.object(native, "load", return_value=self.plan), patch.object(Path, "read_text", return_value=cgroup), \
                patch.object(native, "boot_identity", return_value=self.plan["boot"]), \
                patch.object(native.decisions, "read", return_value=None), \
                patch.object(native.snapshot, "configuration") as configuration, \
                patch.object(native.lifecycle, "recover") as recover:
            result = native.restore(self.root, boot=True)
        self.assertEqual(result["result"], "armed")
        configuration.assert_called_once_with(self.root, {}, wired=False)
        recover.assert_not_called()

    def test_boot_arm_refuses_existing_policy(self) -> None:
        cgroup = "0::/system.slice/" + self.root.name + "-boot.service\n"
        with patch.object(native, "load", return_value=self.plan), patch.object(Path, "read_text", return_value=cgroup), \
                patch.object(native, "boot_identity", return_value=self.plan["boot"]), \
                patch.object(native.decisions, "read", return_value=None), \
                patch.object(native, "emergency_close") as close, \
                patch.object(native.snapshot, "configuration", side_effect=ValueError("policy exists")):
            with self.assertRaisesRegex(ValueError, "policy exists"):
                native.restore(self.root, boot=True)
        close.assert_called_once_with(self.root, boot=True)

    def test_real_boot_uses_recovery_not_same_boot_arm(self) -> None:
        cgroup = "0::/system.slice/" + self.root.name + "-boot.service\n"
        revoked = native.decisions.Decision(self.bound, "revoked", {})
        with patch.object(native, "load", return_value=self.plan), patch.object(Path, "read_text", return_value=cgroup), \
                patch.object(native, "boot_identity", return_value="e" * 64), \
                patch.object(native.decisions, "read", return_value=revoked), \
                patch.object(native.lifecycle, "recover") as recover:
            result = native.restore(self.root, boot=True)
        self.assertEqual(result["result"], "restored")
        self.assertFalse(result["runtimeVerified"])
        self.assertTrue(recover.call_args.kwargs["boot"])

    def test_wiring_failure_closes_even_if_revocation_storage_fails(self) -> None:
        cgroup = "0::/system.slice/" + self.root.name + "-prepare.service\n"
        with patch.object(Path, "read_text", return_value=cgroup), \
                patch.object(native.temporary.guard, "locked", return_value=nullcontext()), \
                patch.object(native, "load", return_value=self.plan), \
                patch.object(native, "require_lease", side_effect=ValueError("expired")), \
                patch.object(native.decisions, "publish", side_effect=OSError("disk failure")), \
                patch.object(native, "emergency_closed_locked") as close, \
                patch.object(native.files, "install") as install:
            with self.assertRaises(OSError):
                native.wire(self.root)
        close.assert_called_once_with(self.root, boot=False)
        install.assert_not_called()

    def test_counter_persistence_precedes_terminal_marker(self) -> None:
        actions = native.Restore(self.root, dict(self.plan))
        events: list[dict[str, object]] = []
        terminal = native.contract.temporary.TerminalState("app-00000000000000000009-" + "c" * 64, 9, 43)
        def settle(_read: object, remember: object, **_kwargs: object) -> native.contract.temporary.TerminalState:
            assert callable(remember)
            remember(native.pipeline.Counters(9, 43))
            return terminal
        def save(_root: Path, plan: dict[str, object]) -> None:
            events.append(dict(plan))
        with patch.object(native.pipeline, "await_terminal", side_effect=settle), \
                patch.object(native.temporary.upload, "empty_spool"), patch.object(native, "save", side_effect=save):
            actions.await_terminal_pipeline()
        self.assertIsNone(events[0]["terminal"])
        self.assertEqual(events[0]["observed"], {"floor": 9, "generation": 43})
        self.assertEqual(events[1]["terminal"], asdict(terminal))

    def test_plan_validation_pins_every_helper(self) -> None:
        raw = b"test asset"
        plan = {**self.plan, "schemaVersion": 1, "started": 1.0, "before": {},
            "files": {name: hashlib.sha256(raw).hexdigest() for name in (*native.ASSETS, "controller.py")}}
        with patch.object(native.decisions, "validate_root", return_value="a" * 24), \
                patch.object(native, "private_file", return_value=raw):
            native.validate(self.root, plan)
        with patch.object(native.decisions, "validate_root", return_value="a" * 24), \
                patch.object(native, "private_file", return_value=b"changed"):
            with self.assertRaisesRegex(ValueError, "code or assets changed"):
                native.validate(self.root, plan)

    def cleanup_context(self, stack: ExitStack, events: list[str]) -> None:
        self.plan.update({"phase": "restored", "terminal": {"slot": self.bound.baseline_slot, "floor": 8, "generation": 40}})
        names = {*native.ASSETS, "controller.py", "plan.json", "decision.json"}
        for target, name, value in ((native.temporary.guard, "locked", nullcontext()), (native, "load", self.plan),
                (native.decisions, "read", native.decisions.Decision(self.bound, "revoked", {})),
                (native.temporary.observer, "unit_busy", False), (native.fence, "exists", False),
                (native.temporary.upload, "properties", {"ActiveState": "inactive", "Job": "0"})):
            stack.enter_context(patch.object(target, name, return_value=value))
        def exists(path: Path) -> bool:
            return path in native.guard_files(self.root)
        def event(name: str) -> Callable[..., None]:
            def record(*_args: object, **_kwargs: object) -> None:
                events.append(name)
            return record
        def property_value(_unit: str, field: str) -> str:
            return "not-found" if field == "LoadState" else ""
        stack.enter_context(patch.object(native.files, "exists", side_effect=exists))
        stack.enter_context(patch.object(Path, "iterdir", return_value=iter(self.root / name for name in names)))
        stack.enter_context(patch.object(native, "validate_guards", side_effect=event("validate-guards")))
        stack.enter_context(patch.object(native, "verify_runtime", side_effect=event("verify-runtime")))
        stack.enter_context(patch.object(native, "save", side_effect=event("save-cleanup")))
        stack.enter_context(patch.object(native.u, "sync_directory"))
        stack.enter_context(patch.object(native.systemd.Admission, "reload_manager", side_effect=lambda: events.append("reload")))
        stack.enter_context(patch.object(native.s, "property_value", side_effect=property_value))
        stack.enter_context(patch.object(Path, "unlink", side_effect=lambda: events.append("unlink-unit")))
        stack.enter_context(patch.object(native.os, "rename", side_effect=event("archive")))

    def test_cleanup_is_durable_before_unit_removal_and_archives_last(self) -> None:
        events: list[str] = []
        with ExitStack() as stack:
            self.cleanup_context(stack, events)
            result = native.cleanup(self.root)
        self.assertEqual(result["result"], "cleaned")
        self.assertLess(events.index("save-cleanup"), events.index("unlink-unit"))
        self.assertEqual(events[-2:], ["verify-runtime", "archive"])
        self.assertTrue(self.plan["cleanupStarted"])

    def test_cleanup_failure_does_not_archive_recovery_metadata(self) -> None:
        events: list[str] = []
        with ExitStack() as stack:
            self.cleanup_context(stack, events)
            stack.enter_context(patch.object(Path, "unlink", side_effect=OSError("interrupted")))
            with self.assertRaises(OSError):
                native.cleanup(self.root)
        self.assertTrue(self.plan["cleanupStarted"])
        self.assertNotIn("archive", events)

    def test_cleanup_refuses_loaded_requires_edge_before_mutation(self) -> None:
        events: list[str] = []
        with ExitStack() as stack:
            self.cleanup_context(stack, events)
            stack.enter_context(patch.object(native.s, "property_value", return_value=self.root.name + "-boot.service"))
            with self.assertRaisesRegex(ValueError, "dependency remains"):
                native.cleanup(self.root)
        self.assertNotIn("save-cleanup", events)
        self.assertNotIn("unlink-unit", events)

    def test_emergency_corrupt_decision_masks_but_never_rewrites_decision(self) -> None:
        with patch.object(native.decisions, "validate_root"), \
                patch.object(native.decisions, "read", side_effect=ValueError("corrupt")), \
                patch.object(native.systemd.Admission, "install_owned_fence") as fence, \
                patch.object(native.decisions, "publish") as publish:
            native.emergency_closed_locked(self.root, boot=True)
        fence.assert_called_once_with(boot=True)
        publish.assert_not_called()

    def test_emergency_valid_commit_is_never_closed(self) -> None:
        winner = native.decisions.Decision(self.bound, "committed", dict.fromkeys(native.contract.REQUIRED_GATES, "d" * 64))
        with patch.object(native.decisions, "validate_root"), patch.object(native.decisions, "read", return_value=winner), \
                patch.object(native.systemd.Admission, "install_owned_fence") as fence:
            native.emergency_closed_locked(self.root, boot=False)
        fence.assert_not_called()

    def test_restore_publication_failure_takes_locked_emergency_path_and_still_fails(self) -> None:
        with patch.object(native, "load", return_value=self.plan), \
                patch.object(native.lifecycle, "recover", side_effect=OSError("publication unavailable")), \
                patch.object(native.temporary.guard, "locked", return_value=nullcontext()) as lock, \
                patch.object(native, "emergency_closed_locked") as close:
            with self.assertRaisesRegex(OSError, "publication unavailable"):
                native.restore(self.root, boot=False)
        lock.assert_called_once_with(self.root)
        close.assert_called_once_with(self.root, boot=False)

    def committed_decision(self) -> native.decisions.Decision:
        return native.decisions.Decision(self.bound, "committed",
            dict.fromkeys(native.contract.REQUIRED_GATES, "d" * 64))

    def retained_plan(self, phase: str, *, disable_started: bool) -> dict[str, object]:
        return {**self.plan, "phase": phase, "disableStarted": disable_started,
            "terminal": {"slot": self.bound.baseline_slot, "floor": 8, "generation": 40}}

    def test_disable_requires_committed_retained_policy(self) -> None:
        plan = self.retained_plan("wired", disable_started=False)
        with patch.object(native, "load", return_value=plan), \
                patch.object(native.decisions, "read", return_value=native.decisions.Decision(self.bound, "revoked", {})), \
                patch.object(native.temporary.guard, "locked", return_value=nullcontext()), \
                patch.object(native, "save") as save:
            with self.assertRaisesRegex(ValueError, "committed retained policy"):
                native.disable(self.root)
        save.assert_not_called()

    def test_disable_closes_drains_removes_unmasks_and_marks_disabled(self) -> None:
        plan = self.retained_plan("committed", disable_started=False)
        events: list[str] = []

        def event(name: str) -> Callable[..., None]:
            def record(*_args: object, **_kwargs: object) -> None:
                events.append(name)
            return record

        def save(_root: Path, value: dict[str, object]) -> None:
            events.append("save-" + str(value["phase"]))

        admission = MagicMock()
        admission.install_owned_fence.side_effect = event("fence")
        admission.reload_manager.side_effect = event("reload")
        with ExitStack() as stack:
            stack.enter_context(patch.object(native.temporary.guard, "locked", return_value=nullcontext()))
            stack.enter_context(patch.object(native, "load", return_value=plan))
            stack.enter_context(patch.object(native.decisions, "read", return_value=self.committed_decision()))
            stack.enter_context(patch.object(native.systemd, "Admission", return_value=admission))
            stack.enter_context(patch.object(native, "save", side_effect=save))
            stack.enter_context(patch.object(native.Restore, "await_terminal_pipeline", side_effect=event("drain")))
            stack.enter_context(patch.object(native.files, "remove", side_effect=event("remove")))
            stack.enter_context(patch.object(native.Restore, "reload_and_restart_confirmed", side_effect=event("restart")))
            stack.enter_context(patch.object(native.fence, "remove", side_effect=event("unmask")))
            stack.enter_context(patch.object(native.temporary.observer, "require_closed_admission", side_effect=event("closed")))
            stack.enter_context(patch.object(native, "verify_runtime", side_effect=event("verify")))
            result = native.disable(self.root)
        self.assertEqual(result["result"], "disabled")
        self.assertTrue(result["runtimeWritesClosed"])
        self.assertFalse(result["retentionApproved"])
        self.assertEqual(plan["phase"], "disabled")
        self.assertTrue(plan["disableStarted"])
        self.assertEqual(events, ["save-committed", "fence", "drain", "remove", "restart", "unmask", "reload",
                                  "save-disabled", "closed", "verify"])

    def test_disabled_policy_resume_verifies_without_repeating_removal(self) -> None:
        plan = self.retained_plan("disabled", disable_started=True)
        with ExitStack() as stack:
            stack.enter_context(patch.object(native.temporary.guard, "locked", return_value=nullcontext()))
            stack.enter_context(patch.object(native, "load", return_value=plan))
            stack.enter_context(patch.object(native.decisions, "read", return_value=self.committed_decision()))
            stack.enter_context(patch.object(native.systemd, "Admission"))
            stack.enter_context(patch.object(native.temporary.observer, "require_closed_admission"))
            fence_remove = stack.enter_context(patch.object(native.fence, "remove"))
            files_remove = stack.enter_context(patch.object(native.files, "remove"))
            save = stack.enter_context(patch.object(native, "save"))
            verify = stack.enter_context(patch.object(native, "verify_runtime"))
            result = native.disable(self.root)
        self.assertEqual(result["result"], "disabled")
        fence_remove.assert_not_called()
        files_remove.assert_not_called()
        save.assert_not_called()
        verify.assert_called_once()

    def test_evidence_staging_requires_wired_undecided_policy(self) -> None:
        plan = {**self.plan, "phase": "committed", "disableStarted": False}
        with patch.object(native, "load", return_value=plan), \
                patch.object(native.decisions, "read", return_value=None), \
                patch.object(native.temporary.guard, "locked", return_value=nullcontext()):
            with self.assertRaisesRegex(ValueError, "undecided wired policy"):
                native.evidence(self.root, envelope=b"{}")

    def test_evidence_staging_seals_exact_gate_receipts_idempotently(self) -> None:
        plan = {**self.plan, "phase": "wired", "disableStarted": False}
        receipts = {gate: ("receipt-" + gate).encode() for gate in native.contract.REQUIRED_GATES}
        gates = {gate: base64.b64encode(data).decode("ascii") for gate, data in receipts.items()}
        envelope = json.dumps({"schemaVersion": 1, "gates": gates}).encode()
        expected = {gate: hashlib.sha256(data).hexdigest() for gate, data in receipts.items()}
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            with patch.object(native.storage, "PARENT", parent), \
                    patch.object(native.storage, "OWNER_UID", os.getuid()), \
                    patch.object(native, "load", return_value=plan), \
                    patch.object(native.decisions, "read", return_value=None), \
                    patch.object(native.temporary.guard, "locked", return_value=nullcontext()):
                result = native.evidence(self.root, envelope=envelope)
                self.assertEqual(result["result"], "sealed")
                self.assertEqual(result["receipts"], expected)
                store = native.storage.Store(native.binding(plan))
                self.assertEqual(store.root.parent, parent)
                store.verify_seal(expected, str(result["sealSha256"]))
                repeat = native.evidence(self.root, envelope=envelope)
                self.assertEqual(repeat["sealSha256"], result["sealSha256"])
                changed = dict(receipts)
                changed["host"] = b"different receipt"
                changed_envelope = json.dumps({"schemaVersion": 1, "gates": {
                    gate: base64.b64encode(data).decode("ascii") for gate, data in changed.items()}}).encode()
                with self.assertRaises(OSError):
                    native.evidence(self.root, envelope=changed_envelope)

    def test_commit_requires_operator_approval_and_seal(self) -> None:
        with patch.object(native.commit_backend, "commit") as backend:
            with self.assertRaisesRegex(ValueError, "retention approval"):
                native.commit(self.root, evidence_seal="a" * 64, approve_retention=False)
            with self.assertRaisesRegex(ValueError, "evidence seal"):
                native.commit(self.root, evidence_seal="short", approve_retention=True)
        backend.assert_not_called()

    def test_commit_passes_sealed_provider_and_reports_winner(self) -> None:
        winner = native.decisions.Decision(self.bound, "committed",
            dict.fromkeys(native.contract.REQUIRED_GATES, "d" * 64))
        captured: dict[str, object] = {}

        def backend(root: Path, provider: object, *, retention_approved: bool) -> native.decisions.Decision:
            captured["root"] = root
            captured["provider"] = provider
            captured["approved"] = retention_approved
            return winner

        with patch.object(native.commit_backend, "commit", side_effect=backend), \
                patch.object(native.evidence_provider, "read_verified_evidence") as evidence:
            result = native.commit(self.root, evidence_seal="e" * 64, approve_retention=True)
            self.assertEqual(result["result"], "committed")
            self.assertEqual(result["gateReceiptHashes"], winner.receipt_hashes)
            self.assertIs(captured["approved"], True)
            provider = cast(Callable[[Path, native.contract.EvidenceBinding], object], captured["provider"])
            provider(self.root, self.bound)
        evidence.assert_called_once_with(self.root, self.bound, expected_seal="e" * 64)

    def test_unreadable_plan_attempts_emergency_closure_without_success_receipt(self) -> None:
        with patch.object(native, "load", side_effect=OSError("unreadable marker")), \
                patch.object(native, "emergency_close") as close:
            with self.assertRaisesRegex(OSError, "unreadable marker"):
                native.restore(self.root, boot=False)
        close.assert_called_once_with(self.root, boot=False)
