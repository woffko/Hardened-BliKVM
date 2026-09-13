from pathlib import Path
import unittest

from tools import stage_device_hid_credential_gate as gate


ROOT = Path(__file__).resolve().parent.parent


class HidCredentialGateTests(unittest.TestCase):
    def paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0" * 16)

    def test_preflight_requires_a_free_unit_dedicated_account_and_payload(self) -> None:
        script = gate.build_preflight_script(self.paths())
        self.assertIn(f'unit={gate.UNIT}', script.splitlines()[0] + "\n" + script)
        self.assertIn("hardened-blikvm-hid", script)
        self.assertIn("systemd-run", script)
        self.assertIn("unit_free", script)
        self.assertIn("test_root_absent", script)

    def test_apply_delivers_credentials_without_host_secrets(self) -> None:
        paths = self.paths()
        script = gate.build_apply_script(paths)
        self.assertIn("systemd-run --quiet --wait --pipe --collect", script)
        self.assertIn("--unit=hardened-blikvm-hid-credential", script)
        self.assertIn(f'--property=LoadCredential="kvmd-user:$cred/kvmd-user"', script)
        self.assertIn(
            f'--property=LoadCredential="kvmd-password:$cred/kvmd-password"', script
        )
        self.assertIn('chmod 400 "$cred/kvmd-user" "$cred/kvmd-password"', script)
        self.assertIn("od -An -N8 -tx1 /dev/urandom", script)
        self.assertIn("setpriv", script)
        self.assertIn("source_read_denied", script)
        self.assertIn("hid-credential-delivery=ok", script)
        self.assertNotIn("10.0.", script)
        self.assertNotIn("/home/", script)

    def test_rollback_removes_only_the_test_root_and_transient_unit(self) -> None:
        script = gate.build_rollback_script(self.paths())
        self.assertIn(f'rm -rf -- "$root"', script)
        self.assertIn('systemctl stop "$unit"', script)
        self.assertIn('systemctl reset-failed "$unit"', script)
        self.assertIn("credentials_removed", script)
        self.assertIn("unit_absent", script)

    def test_receipt_contract_excludes_secrets_and_physical_actions(self) -> None:
        keys = gate.Receipt.__annotations__
        for required in (
            "bundleSha256",
            "gateSha256",
            "unit",
            "failureStage",
            "deliveryToken",
            "sourceReadDenied",
            "cleanupConfirmed",
            "physicalActionsInvoked",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_hid_credential_gate.py").read_text()
        self.assertIn('"physicalActionsInvoked": False', source)
        self.assertIn('"persistentChangeScope": "none"', source)
        self.assertLess(source.index("stage = \"apply\""), source.index("stage = \"rollback\""))


if __name__ == "__main__":
    unittest.main()
