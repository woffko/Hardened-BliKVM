import copy
import inspect
import unittest
from typing import cast
from pathlib import Path
import tempfile
from unittest.mock import patch

from tools import stage_operational_reboot as gate


class OperationalRebootTests(unittest.TestCase):
    def test_interruption_cleanup_without_ownership_preserves_user_override(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "probe"
            root.mkdir()
            override = Path(temporary) / "overrides"
            override.mkdir()
            user_file = override / "user.conf"
            user_file.write_text("unrelated")
            status = {"pending": None, "floorSequence": 2, "activeSlotId": gate.interruption.CONFIRMED,
                      "confirmedSlotId": gate.interruption.CONFIRMED}
            with patch.object(gate.interruption, "validate_assets"), \
                 patch.object(gate.interruption, "DROPIN_ROOT", override), \
                 patch.object(gate.interruption, "DROPIN", override / "ours.conf"), \
                 patch.object(gate.interruption, "source_root", return_value=root / "absent-input"), \
                 patch.object(gate.interruption.infrastructure, "current_status", return_value=status), \
                 patch.object(gate.interruption, "checked") as checked:
                gate.interruption.cleanup(root)
                checked.assert_not_called()
            self.assertEqual(user_file.read_text(), "unrelated")

    def test_pending_reboot_requires_boot_changed_rollback_without_floor_raise(self) -> None:
        before = self.fixture()
        after = copy.deepcopy(before)
        after["bootIdSha256"] = "b" * 64
        status: dict[str, object] = {"floorSequence": 2, "generation": 10, "pending": None,
                                    "activeSlotId": gate.interruption.CONFIRMED, "confirmedSlotId": gate.interruption.CONFIRMED,
                                    "previousSlotId": gate.interruption.CANDIDATE, "lastRollback": "boot_changed"}
        after["status"] = status
        final_tree = cast(dict[str, object], cast(dict[str, object], after["snapshot"])["state"])
        final_tree["candidate-file"] = "hash2"
        pending: dict[str, object] = {"result": "pending", "bootIdSha256": "a" * 64, "status": {"generation": 9},
                   "confirmedTree": {"signed-file": "hash"}, "candidateTree": {"candidate-file": "hash2"}}
        gate.compare(before, after, pending)
        for field, value in (("lastRollback", "health_timeout"), ("floorSequence", 3)):
            changed = copy.deepcopy(after)
            cast(dict[str, object], changed["status"])[field] = value
            with self.assertRaises(ValueError):
                gate.compare(before, changed, pending)

    def fixture(self) -> dict[str, object]:
        return {"result": "ready", "bootIdSha256": "a" * 64, "status": {"floorSequence": 2, "generation": 7},
                "snapshot": {"state": {"signed-file": "hash"}, "nginxUnit": "hash",
                             "runtime": {"account": "hash", "nginxConfig": "hash", "certificate": "hash", "tlsKey": "hash",
                                         "trust": "hash", "brokerPid": "100", "brokerRestarts": "0", "videoRestarts": "0"}},
                "recoveryOrderVerified": True, "rootfsMode": "rw"}

    def test_comparison_accepts_new_boot_with_preserved_state(self) -> None:
        before = self.fixture()
        after = copy.deepcopy(before)
        after["bootIdSha256"] = "b" * 64
        gate.compare(before, after)
        after["status"] = {"floorSequence": 1, "generation": 1}
        with self.assertRaises(ValueError):
            gate.compare(before, after)

    def test_unchanged_or_missing_boot_identity_is_not_reboot_proof(self) -> None:
        before = self.fixture()
        for value in (None, "", "a" * 64):
            after = copy.deepcopy(before)
            after["bootIdSha256"] = value
            with self.assertRaises(ValueError):
                gate.compare(before, after)

    def test_verified_order_and_requested_rw_mode_are_required(self) -> None:
        before = self.fixture()
        for field, value in (("rootfsMode", "ro"), ("recoveryOrderVerified", False)):
            after = copy.deepcopy(before)
            after["bootIdSha256"] = "b" * 64
            after[field] = value
            with self.assertRaises(ValueError):
                gate.compare(before, after)

    def test_single_nonforced_software_reboot_request(self) -> None:
        self.assertEqual(gate.REBOOT_COMMAND, "systemctl reboot --no-block")
        source = inspect.getsource(gate.main)
        self.assertEqual(source.count("transport.run_ssh(target, secret, REBOOT_COMMAND"), 1)
        self.assertIn("except subprocess.TimeoutExpired:", source)
        self.assertIn("if not args.reboot:", source)
        self.assertNotIn("--force", source)


if __name__ == "__main__":
    unittest.main()
