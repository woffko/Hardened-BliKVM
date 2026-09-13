from pathlib import Path
import unittest

from tools import stage_device_atx_live_gate as gate


ROOT = Path(__file__).resolve().parent.parent


class AtxLiveGateTests(unittest.TestCase):
    def paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0" * 16)

    def test_preflight_checks_kvmd_socket_credentials_and_unit(self) -> None:
        script = gate.build_preflight_script(self.paths())
        for key in ("kvmd_socket", "credentials", "systemd_run", "unit_free", "test_root_absent"):
            self.assertIn(key, script)
        self.assertIn("systemd-run", script)

    def test_apply_delivers_the_credential_through_loadcredential(self) -> None:
        script = gate.build_apply_script(self.paths())
        self.assertIn("systemd-run --quiet --wait --pipe --collect", script)
        self.assertIn("--unit=hardened-blikvm-atx-live-gate", script)
        self.assertIn(
            "--property=LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user", script
        )
        self.assertIn(
            "--property=LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password",
            script,
        )
        self.assertIn('install -m 0755 "$gate"', script)
        self.assertIn('cat "$root/gate.out"', script)
        self.assertNotIn("atx.sock", script)

    def test_rollback_clears_the_transient_unit_and_stage(self) -> None:
        script = gate.build_rollback_script(self.paths())
        self.assertIn('systemctl stop "$unit"', script)
        self.assertIn('systemctl reset-failed "$unit"', script)
        self.assertIn('rm -rf -- "$stage_root"', script)
        self.assertIn("unit_absent", script)

    def test_facts_and_receipt_contract(self) -> None:
        for key in (
            "power_short",
            "power_short_elapsed_ms",
            "duplicate_replay",
            "conflict_rejected",
            "cooldown_active",
            "power_long",
            "reset",
            "restart_pending",
            "reconciliation",
        ):
            self.assertIn(key, gate.GATE_KEYS)
        keys = gate.Receipt.__annotations__
        for required in (
            "bundleSha256",
            "gateSha256",
            "facts",
            "cleanupConfirmed",
            "failureStage",
            "failureType",
            "powerResetPhysicallyDisconnected",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_atx_live_gate.py").read_text()
        self.assertIn('"persistentChangeScope": "none"', source)
        self.assertIn('"powerResetPhysicallyDisconnected": True', source)


if __name__ == "__main__":
    unittest.main()
