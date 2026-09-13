from pathlib import Path
import unittest

from tools import stage_device_hid_reboot_gate as gate


ROOT = Path(__file__).resolve().parent.parent


class HidRebootGateTests(unittest.TestCase):
    def test_preflight_and_postflight_share_the_same_safe_facts(self) -> None:
        for script in (gate.build_preflight_script(), gate.build_postflight_script()):
            for key in (
                "boot_token",
                "kvmd_active",
                "retained_state",
                "staging_absent",
                "account_absent",
            ):
                self.assertIn(key, script)
            self.assertIn("sha256sum /proc/sys/kernel/random/boot_id", script)

    def test_reboot_script_requests_exactly_one_reboot(self) -> None:
        script = gate.build_reboot_script()
        self.assertEqual(script.count("systemctl reboot"), 1)
        self.assertIn("reboot_requested", script)

    def test_receipt_stores_no_raw_boot_identifier(self) -> None:
        keys = gate.Receipt.__annotations__
        self.assertNotIn("boot_token", keys)
        for required in (
            "bootIdentityChanged",
            "kvmdActive",
            "retainedStateIntact",
            "stagingAbsent",
            "accountAbsent",
            "cleanupConfirmed",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_hid_reboot_gate.py").read_text()
        self.assertNotIn("boot_token\":", source)
        self.assertIn('"persistentChangeScope": "reboot"', source)

    def test_validation_fails_closed(self) -> None:
        with self.assertRaises(gate.GateError):
            gate.validate_preflight({"architecture": "ok"})
        with self.assertRaises(gate.GateError):
            gate.validate_preflight(
                {
                    "architecture": "no",
                    "boot_token": "x",
                    "kvmd_active": "yes",
                    "retained_state": "yes",
                    "staging_absent": "yes",
                    "account_absent": "yes",
                }
            )


if __name__ == "__main__":
    unittest.main()
