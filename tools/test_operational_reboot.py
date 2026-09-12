import copy
import inspect
import unittest
from typing import cast
from pathlib import Path
import tempfile
from unittest.mock import patch
from types import SimpleNamespace

from tools import stage_operational_reboot as gate
from tools import capture_operational_boot as observer


class OperationalRebootTests(unittest.TestCase):
    def test_application_profile_is_independent_and_preserves_legacy_defaults(self) -> None:
        self.assertEqual(observer.resolve_application_profile("dev6-dev7", None), "seq2")
        self.assertEqual(observer.resolve_application_profile("dev7-dev9", None), "seq6")
        self.assertEqual(observer.resolve_application_profile("dev7-dev9", "seq7"), "seq7")
        self.assertEqual(observer.resolve_application_profile("dev7-dev9", "seq9"), "seq9")
        self.assertEqual(observer.resolve_application_profile("dev7-dev9", "seq8"), "seq8")
        for updater, application in (("dev6-dev7", "seq8"), ("dev6-dev7", "seq7"), ("dev6-dev7", "seq6"), ("dev7-dev9", "seq2"), ("unknown", None)):
            with self.assertRaises(ValueError):
                observer.resolve_application_profile(updater, application)
        self.assertIn('"--application-profile", application_profile', inspect.getsource(gate.main))

    def test_seq7_snapshot_binds_slot_generation_anchor_and_preserves_dropins(self) -> None:
        original = observer.u.selected_profile
        observer.u.select_profile("dev7-dev9")
        value: dict[str, object] = {"managerStatus": {"previousSlotId": observer.u.SEQ6_SLOT}, "serving": {}}
        try:
            with patch.object(observer.persistent, "validate_dropins") as dropins, \
                 patch.object(observer.u, "write_transports_inert") as inert, \
                 patch.object(observer.status_api, "snapshot", return_value=value) as snapshot, \
                 patch.object(observer.s, "property_value", return_value="123"), \
                 patch.object(observer.persistent, "dropins", return_value=(Path("/current"), Path("/candidate"))), \
                 patch.object(observer.u, "digest", return_value="hash"):
                result = observer.seq7_snapshot()
                snapshot.assert_called_once_with(True, expected_version="0.1.0~dev9",
                    expected_supervisor_sha=observer.u.SUPERVISOR_HASHES["new"],
                    expected_slot=observer.SEQ7_SLOT, expected_floor=7, expected_generation=37)
                dropins.assert_called_once_with(True)
                inert.assert_called_once_with()
                self.assertEqual(result["persistentDropinHashes"], {"/current": "hash", "/candidate": "hash"})
                cast(dict[str, object], value["managerStatus"])["previousSlotId"] = "foreign"
                with self.assertRaisesRegex(ValueError, "rollback anchor"):
                    observer.seq7_snapshot()
        finally:
            observer.u.select_profile(original)

    def test_dev9_observer_checks_retained_snapshot_and_status_startup_order(self) -> None:
        observer.u.select_profile("dev7-dev9")
        properties = {"Requires": observer.RECOVERY, "After": observer.RECOVERY,
                      "ActiveState": "active", "SubState": "exited", "Result": "success",
                      "ExecMainStatus": "0", "ExecMainExitTimestampMonotonic": "100",
                      "ExecMainStartTimestampMonotonic": "200", "ActiveEnterTimestampMonotonic": "150"}

        def property_value(_unit: str, field: str) -> str:
            return properties[field]

        try:
            with patch.object(observer.u, "installed") as installed, \
                 patch.object(observer.u, "snapshot", return_value={}) as snapshot, \
                 patch.object(observer.infrastructure, "enabled_snapshot") as legacy, \
                 patch.object(observer.infrastructure, "current_status", return_value={}), \
                 patch.object(observer.s, "property_value", side_effect=property_value), \
                 patch.object(Path, "read_bytes", return_value=b"boot"), \
                 patch.object(Path, "read_text", return_value="12.50 25.00"), \
                 patch.object(observer.os, "statvfs", return_value=SimpleNamespace(f_flag=0)):
                result = observer.capture(Path("/assets"), True)
                self.assertTrue(result["recoveryOrderVerified"])
                self.assertEqual(result["uptimeSeconds"], 12)
                installed.assert_called_once_with(Path("/assets"), "new")
                snapshot.assert_called_once_with("new")
                legacy.assert_not_called()
                with patch.object(observer, "seq7_snapshot", return_value={}) as seq7:
                    result = observer.capture(Path("/assets"), True, "seq7")
                    self.assertEqual(result["applicationProfile"], "seq7")
                    seq7.assert_called_once_with()
                with patch.object(observer, "seq8_snapshot", return_value={}) as seq8:
                    result = observer.capture(Path("/assets"), True, "seq8")
                    self.assertEqual(result["applicationProfile"], "seq8")
                    seq8.assert_called_once_with()
                for invalid in ("0", "201", "invalid"):
                    properties["ActiveEnterTimestampMonotonic"] = invalid
                    with self.assertRaises(ValueError):
                        observer.capture(Path("/assets"), True)
        finally:
            observer.u.select_profile("dev6-dev7")

    def test_seq8_observer_requires_confirmed_target_and_no_write_environment(self) -> None:
        original = observer.u.selected_profile
        observer.u.select_profile("dev7-dev9")
        value: dict[str, object] = {"managerStatus": {"previousSlotId": observer.SEQ7_SLOT, "lastRollback": None}, "serving": {}}
        def property_value(_unit: str, field: str) -> str:
            return "123" if field == "MainPID" else ""
        try:
            with patch.object(observer.persistent, "validate_dropins"), patch.object(observer.u, "write_transports_inert"), \
                 patch.object(observer.status_api, "snapshot", return_value=value) as snapshot, \
                 patch.object(observer.s, "property_value", side_effect=property_value), \
                 patch.object(observer.persistent, "dropins", return_value=()), \
                 patch.object(Path, "glob", return_value=iter(())), \
                 patch.object(Path, "read_bytes", return_value=b"SAFE=value"):
                observer.seq8_snapshot()
                snapshot.assert_called_once_with(True, expected_version="0.1.0~dev9",
                    expected_supervisor_sha=observer.u.SUPERVISOR_HASHES["new"], expected_slot=observer.SEQ8_SLOT,
                    expected_floor=8, expected_generation=40)
                with patch.object(Path, "read_bytes", return_value=b"BLI_KVM_UPDATE_WRITES_ENABLED=1"):
                    with self.assertRaisesRegex(ValueError, "running HTTP"):
                        observer.seq8_snapshot()
                cast(dict[str, object], value["managerStatus"])["lastRollback"] = "health_timeout"
                with self.assertRaisesRegex(ValueError, "confirmation or rollback"):
                    observer.seq8_snapshot()
        finally:
            observer.u.select_profile(original)

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

    def test_persistent_status_dropins_must_survive_reboot(self) -> None:
        before = self.fixture()
        cast(dict[str, object], before["snapshot"])["persistentDropinHashes"] = {"current": "a", "candidate": "b"}
        after = copy.deepcopy(before)
        after["bootIdSha256"] = "b" * 64
        gate.compare(before, after)
        for changed in (None, {}, {"current": "a", "candidate": "changed"}):
            cast(dict[str, object], after["snapshot"])["persistentDropinHashes"] = changed
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
