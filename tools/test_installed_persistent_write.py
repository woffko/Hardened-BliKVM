from contextlib import ExitStack, nullcontext
from collections.abc import Callable
from dataclasses import asdict
import hashlib
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import installed_persistent_write as native


class ControllerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path("/var/lib") / (native.contract.PREFIX + "a" * 24)
        self.bound = native.contract.EvidenceBinding("a" * 24, "b" * 64, native.contract.CONFIG_SHA256,
            "app-00000000000000000008-" + "c" * 64, 8, 40)
        self.plan: dict[str, object] = {"phase": "prepared", "binding": asdict(self.bound),
            "backup": {}, "boot": "d" * 64, "observed": {"floor": 8, "generation": 40}, "terminal": None, "cleanupStarted": False}

    def test_staged_controller_import_closure_and_no_retention_cli(self) -> None:
        source = Path(native.__file__).parent
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in native.HELPERS:
                (root / name).write_bytes((source / name).read_bytes())
            (root / "controller.py").write_bytes(Path(native.__file__).read_bytes())
            result = subprocess.run([sys.executable, "-B", str(root / "controller.py"), "--help"],
                capture_output=True, timeout=5, check=False)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertIn(b"{prepare,wire,restore,cleanup}", result.stdout)
            self.assertNotIn(b"--retention-approved", result.stdout)

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
                patch.object(native.snapshot, "configuration", side_effect=ValueError("policy exists")):
            with self.assertRaisesRegex(ValueError, "policy exists"):
                native.restore(self.root, boot=True)

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
                patch.object(native.temporary.observer, "close_admission") as close, \
                patch.object(native.files, "install") as install:
            with self.assertRaises(OSError):
                native.wire(self.root)
        close.assert_called_once()
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
