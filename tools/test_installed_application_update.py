import inspect
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import installed_application_update as gate
from tools import stage_installed_application_update as driver


class InstalledApplicationUpdateTests(unittest.TestCase):
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
