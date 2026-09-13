from pathlib import Path
import unittest

from tools import stage_device_hid_profile_gate as gate


ROOT = Path(__file__).resolve().parent.parent


class HidProfileGateTests(unittest.TestCase):
    def paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0" * 16)

    def test_preflight_checks_arch_and_clean_root(self) -> None:
        script = gate.build_preflight_script(self.paths())
        self.assertIn("emit architecture", script)
        self.assertIn("test_root_absent", script)

    def test_apply_proves_credential_requirement_and_startup(self) -> None:
        script = gate.build_apply_script(self.paths())
        self.assertIn("tar -xzf", script)
        self.assertIn("env -u CREDENTIALS_DIRECTORY", script)
        self.assertIn("BLI_KVM_HID_PROFILE=1", script)
        self.assertIn("refused_without_credentials", script)
        self.assertIn("CREDENTIALS_DIRECTORY=\"$root/cred\"", script)
        self.assertIn("started_with_credentials", script)
        self.assertIn("kill \"$pid\"", script)
        self.assertNotIn("/run/kvmd/kvmd.sock", script)
        self.assertNotIn("systemd-run", script)

    def test_rollback_kills_the_process_and_removes_only_its_stage(self) -> None:
        script = gate.build_rollback_script(self.paths())
        self.assertIn("broker.pid", script)
        self.assertIn('rm -rf -- "$stage_root"', script)
        self.assertIn("process_stopped", script)

    def test_receipt_contract(self) -> None:
        keys = gate.Receipt.__annotations__
        for required in (
            "bundleSha256",
            "gateSha256",
            "facts",
            "cleanupConfirmed",
            "failureStage",
            "failureType",
            "physicalActionsInvoked",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_hid_profile_gate.py").read_text()
        self.assertIn('"physicalActionsInvoked": False', source)
        self.assertIn('"persistentChangeScope": "none"', source)
        self.assertIn('"refused_without_credentials"', source)


if __name__ == "__main__":
    unittest.main()
