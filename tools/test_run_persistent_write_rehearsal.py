from dataclasses import asdict
import subprocess
import unittest
from unittest.mock import patch

from tools import run_persistent_write_rehearsal as runner


class RehearsalTests(unittest.TestCase):
    def setUp(self) -> None:
        self.receipt: dict[str, object] = {"result": "failed", "prepareSubmitted": False,
            "cleanupSubmitted": False, "guardRetained": True, "runtimeWritesClosed": False}
        self.calls: list[str] = []
        self.saved: list[dict[str, object]] = []
    def save(self) -> None:
        self.saved.append(dict(self.receipt))
    def invoke(self, action: str) -> dict[str, object]:
        if action == "prepare":
            self.assertTrue(self.saved[-1]["prepareSubmitted"])
            self.assertFalse(self.saved[-1]["runtimeWritesClosed"])
        if action == "cleanup":
            self.assertTrue(self.saved[-1]["cleanupSubmitted"])
        self.calls.append(action)
        return {}
    def observe(self) -> dict[str, object]:
        self.calls.append("observe")
        return {"bootIdSha256": "a" * 64}

    def test_preflight_never_submits_mutating_controller_phase(self) -> None:
        runner.run_phases("preflight", self.receipt, invoke=self.invoke, observe=self.observe, save=self.save)
        self.assertEqual(self.calls, ["observe"])
        self.assertEqual(self.receipt["result"], "passed")
        self.assertTrue(self.receipt["runtimeWritesClosed"])
        self.assertFalse(self.receipt["prepareSubmitted"])

    def test_rehearsal_is_one_prepare_restore_cleanup_and_two_observations(self) -> None:
        runner.run_phases("rehearse", self.receipt, invoke=self.invoke, observe=self.observe, save=self.save)
        self.assertEqual(self.calls, ["observe", "prepare", "restore", "cleanup", "observe"])
        self.assertEqual(self.receipt["result"], "passed")
        self.assertFalse(self.receipt["guardRetained"])

    def test_ambiguous_prepare_never_retries_prepare(self) -> None:
        def invoke(action: str) -> dict[str, object]:
            self.calls.append(action)
            if action == "prepare":
                raise subprocess.TimeoutExpired("ssh", 260)
            return {}
        runner.run_phases("rehearse", self.receipt, invoke=invoke, observe=self.observe, save=self.save)
        self.assertEqual(self.calls, ["observe", "prepare", "restore"])
        self.assertEqual(self.receipt["result"], "failed")
        self.assertTrue(self.receipt["runtimeWritesClosed"])
        self.assertTrue(self.receipt["guardRetained"])

    def test_ambiguous_cleanup_never_retries_or_restores_archived_root(self) -> None:
        def invoke(action: str) -> dict[str, object]:
            self.calls.append(action)
            if action == "cleanup":
                raise subprocess.TimeoutExpired("ssh", 180)
            return {}
        runner.run_phases("rehearse", self.receipt, invoke=invoke, observe=self.observe, save=self.save)
        self.assertEqual(self.calls, ["observe", "prepare", "restore", "cleanup"])
        self.assertTrue(self.receipt["independentInspectionRequired"])

    def test_archive_cleanup_counter_types_are_strict(self) -> None:
        root = "/var/lib/" + runner.native.contract.PREFIX + "a" * 24
        value = {"result": "cleaned", "guardRoot": root, "archiveRoot": "/var/lib/" + runner.native.ARCHIVE_PREFIX + "a" * 24,
            "runtimeWritesClosed": True, "floor": 8, "generation": 40}
        runner.validate_result(value, action="cleanup", root=root)
        for invalid in (True, 0, "8"):
            with self.assertRaises(ValueError):
                runner.validate_result({**value, "floor": invalid}, action="cleanup", root=root)

    def test_prepare_receipt_requires_exact_baseline_and_no_retention(self) -> None:
        root = "/var/lib/" + runner.native.contract.PREFIX + "a" * 24
        bound = runner.native.contract.EvidenceBinding("a" * 24, "b" * 64, runner.native.contract.CONFIG_SHA256,
            runner.native.temporary.upload.boot.SEQ8_SLOT, 8, 40)
        value = {"result": "prepared", "guardRoot": root, "phase": "wired", "deadlineArmed": True,
            "retentionApproved": False, "binding": asdict(bound)}
        runner.validate_result(value, action="prepare", root=root)
        with self.assertRaises(ValueError):
            runner.validate_result({**value, "retentionApproved": True}, action="prepare", root=root)

    def test_payload_contains_no_application_archive_or_old_package(self) -> None:
        with patch.object(runner.packages, "select_profile"), \
                patch.object(runner.packages, "upload_payloads", return_value={"bindings.json": b"{}", "new.deb": b"test", "old.deb": b"old"}):
            uploads = runner.build_uploads("old", "new")
        self.assertEqual(set(uploads), {*runner.native.ASSETS, "installed_persistent_write.py"})
        self.assertNotIn("old.deb", uploads)
        self.assertNotIn("application.tar", uploads)
