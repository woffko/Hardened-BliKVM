import inspect
import hashlib
import io
import json
from pathlib import Path
import tempfile
import tarfile
import unittest
from unittest.mock import patch

from tools import installed_application_update as gate
from tools import stage_installed_application_update as driver


class InstalledApplicationUpdateTests(unittest.TestCase):
    def test_notification_profile_is_bound_to_verified_sequence_five(self) -> None:
        profile = gate.PROFILES["seq4-to-seq5-notification"]
        self.assertEqual(profile.source_slot, gate.PROFILES["seq2-to-seq4-status"].target_slot)
        self.assertEqual((profile.source_sequence, profile.target_sequence), (4, 5))
        self.assertEqual(profile.bundle_bytes, 8591360)
        self.assertEqual(profile.bundle_sha, "618b1068086a04aa75f44804b58bcf0dac05d62c11553af74ebfe5cfdf893093")
        with patch.object(gate, "active_profile", profile):
            self.assertEqual(gate.minimum_recovery_floor({"phase": "confirmed"}), 5)
            self.assertEqual(gate.minimum_recovery_floor({"phase": "recovered", "observedFloor": 5}), 5)

    def test_named_profile_preserves_legacy_and_binds_new_recovery(self) -> None:
        original = gate.active_profile
        try:
            gate.select_profile("seq2-to-seq4-status")
            self.assertEqual(gate.profile_slots(), {2: gate.SLOT_TWO, 4: gate.active_profile.target_slot})
            self.assertEqual(gate.active_profile.bundle_bytes, 8591360)
            root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
            for suffix in ("-boot.service", "-recovery.service"):
                self.assertIn("--profile seq2-to-seq4-status", gate.guard_units(root)[root.name + suffix])
            with self.assertRaises(ValueError):
                gate.select_profile("unreviewed")
            self.assertEqual(gate.active_profile.name, "seq2-to-seq4-status")
            gate.select_profile("seq1-to-seq2")
            self.assertEqual(gate.profile_slots(), gate.SLOTS)
        finally:
            gate.select_profile(original.name)

    def test_staged_payload_matches_new_archive_not_previous_application(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "application.tar"
            payload = b"new server bytes"
            with tarfile.open(archive, "w") as output:
                member = tarfile.TarInfo("payload/bin/bli-kvm-server")
                member.size = len(payload)
                member.mode = 0o755
                output.addfile(member, io.BytesIO(payload))
            slot = root / "slot"
            (slot / "bin").mkdir(parents=True)
            target = slot / "bin/bli-kvm-server"
            target.write_bytes(payload)
            target.chmod(0o555)
            profile = gate.UpdateProfile("fixture", hashlib.sha256(archive.read_bytes()).hexdigest(),
                                         archive.stat().st_size, 2, "source", 4, "target")
            with patch.object(gate, "active_profile", profile):
                gate.verify_staged_payload(archive, slot)
                target.chmod(0o755)
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)
                target.write_bytes(b"wrong server bytes")
                target.chmod(0o555)
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)
                target.chmod(0o755)
                target.write_bytes(payload)
                target.chmod(0o555)
                (slot / "bin/extra").write_bytes(b"extra")
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)

    def test_recovery_rejects_floor_regression_but_accepts_confirmation_before_marker(self) -> None:
        original = gate.active_profile
        try:
            gate.select_profile("seq2-to-seq4-status")
            root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
            for boot in (False, True):
                for phase, observed, floor, accepted in (
                    ("confirmed", 2, 2, False), ("confirmed", 2, 4, True),
                    ("rehearsed", 2, 4, True), ("recovered", 4, 2, False),
                ):
                    slot = gate.profile_slots()[floor]
                    status = {"floorSequence": floor, "pending": None, "activeSlotId": slot, "confirmedSlotId": slot}
                    plan: dict[str, object] = {"phase": phase, "observedFloor": observed}
                    with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
                         patch.object(gate.infrastructure, "current_status", return_value=status), \
                         patch.object(gate, "await_stable", return_value={"status": status}), \
                         patch.object(gate, "save") as save, patch.object(gate, "disarm") as disarm, \
                         patch.object(gate, "checked", return_value=b""):
                        if accepted:
                            gate.recover(root, plan, boot)
                            self.assertEqual(plan["observedFloor"], 4)
                        else:
                            with self.assertRaises(ValueError):
                                gate.recover(root, plan, boot)
                            save.assert_not_called()
                            disarm.assert_not_called()
        finally:
            gate.select_profile(original.name)

    def test_commit_selects_browser_phase_from_bound_checkpoint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            base = {"probe": "installed-application-update", "result": "passed",
                    "bundleSha256": gate.active_profile.bundle_sha, "profile": gate.active_profile.name,
                    "guardRoot": "/var/lib/" + gate.PREFIX + "a" * 24}
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                for phase, expected in (("rehearsed", "rehearsal"), ("confirmed", "confirmed")):
                    path.write_text(json.dumps({**base, "device": {"phase": phase}}))
                    self.assertEqual(driver.commit_browser_phase(str(path)), expected)
                path.write_text(json.dumps({**base, "profile": "other", "device": {"phase": "confirmed"}}))
                with self.assertRaises(ValueError):
                    driver.commit_browser_phase(str(path))

    def test_device_commit_rejects_stale_browser_phase(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(gate, "load", return_value={"phase": "confirmed"}), \
                 patch.object(gate, "stable", return_value={"status": {"floorSequence": 2}}), \
                 patch.object(gate, "save") as save, patch.object(gate, "disarm") as disarm:
                with self.assertRaises(ValueError):
                    gate.transition("commit", root, False, "rehearsal")
                save.assert_not_called()
                disarm.assert_not_called()
                gate.transition("commit", root, False, "confirmed")
                self.assertEqual(save.call_args.args[1]["phase"], "committed-update")

    def test_browser_acceptance_is_bound_to_guard_and_transaction_phase(self) -> None:
        root = "/var/lib/" + gate.PREFIX + "b" * 24
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "browser.json"
            receipt: dict[str, object] = {"probe": "hardened-blikvm-existing-account-browser", "result": "passed",
                                          "runTag": Path(root).name + "-rehearsal", "controlRequests": 0,
                                          "webSockets": 0, "secretEmitted": False}
            receipt.update(dict.fromkeys(("videoDecoded", "streamRevoked", "secureSessionCookie", "secureCsrfCookie", "sessionCleared", "legacyBrandingAbsent"), True))
            receipt.update(dict.fromkeys(("loginStatus", "inventoryStatus", "diagnosticsStatus", "videoStatus", "logoutStatus"), 200))
            path.write_text(json.dumps(receipt))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                driver.browser_gate(str(path), root, "rehearsal")
                with self.assertRaises(ValueError):
                    driver.browser_gate(str(path), root, "confirmed")
                with self.assertRaises(ValueError):
                    driver.browser_gate(str(path), root.replace("b" * 24, "c" * 24), "rehearsal")

    def test_boot_handoff_orders_after_watchdog_without_waiting_for_gui(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        unit = gate.guard_units(root)[root.name + "-boot.service"]
        self.assertIn("After=basic.target " + gate.infrastructure.WATCHDOG, unit)
        self.assertIn("Before=" + gate.s.CURRENT_SERVER + " " + gate.s.CANDIDATE_SERVER, unit)
        self.assertIn("WantedBy=multi-user.target", unit)
        self.assertNotIn("sysinit.target", unit)
        status = {"floorSequence": 2, "pending": None, "activeSlotId": gate.SLOT_TWO, "confirmedSlotId": gate.SLOT_TWO}
        with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
             patch.object(gate.infrastructure, "current_status", return_value=status), \
             patch.object(gate, "await_stable") as wait, patch.object(gate, "save"), patch.object(gate, "disarm"):
            result = gate.recover(root, {"phase": "confirmed"}, True)
            wait.assert_not_called()
            self.assertFalse(result["journalRestoredFromBackup"])

    def test_recovery_after_confirmation_preserves_floor_two(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        verified = {"status": {"floorSequence": 2, "pending": None}}
        with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
             patch.object(gate.infrastructure, "current_status", return_value=verified["status"]), \
             patch.object(gate, "await_stable", return_value=verified), patch.object(gate, "save"), \
             patch.object(gate, "disarm"), patch.object(gate, "checked", return_value=b"") as checked:
            result = gate.recover(root, {"phase": "confirmed"}, False)
            self.assertEqual(result["verification"], verified)
            self.assertFalse(result["journalRestoredFromBackup"])
            self.assertEqual([call.args[0] for call in checked.call_args_list], [
                ["systemctl", "start", gate.infrastructure.WATCHDOG],
                ["systemctl", "start", gate.s.CURRENT_SERVER],
            ])

    def test_changed_health_override_is_not_removed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "override.conf"
            path.write_text("unrelated override")
            with patch.object(gate, "DROPIN", path), patch.object(gate, "checked", return_value=b""), self.assertRaises(ValueError):
                gate.remove_override()
            self.assertEqual(path.read_text(), "unrelated override")

    def test_no_journal_or_pointer_rewrite_in_recovery_controller(self) -> None:
        source = inspect.getsource(gate)
        self.assertNotIn("application-state.json", source)
        self.assertNotIn("os.symlink(", source)
        self.assertNotIn("shutil.rmtree", source)
        for forbidden in ('"reboot"', '"poweroff"', '"reset"', '"confirm-stdin"'):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
