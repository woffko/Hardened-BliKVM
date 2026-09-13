from pathlib import Path
import unittest

from tools import stage_device_hid_isolated_gate as gate


ROOT = Path(__file__).resolve().parent.parent


class HidIsolatedGateTests(unittest.TestCase):
    def paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0" * 16)

    def test_preflight_checks_arch_and_clean_root(self) -> None:
        script = gate.build_preflight_script(self.paths())
        self.assertIn("emit architecture", script)
        self.assertIn("test_root_absent", script)

    def test_apply_extracts_and_runs_the_argument_free_gate(self) -> None:
        script = gate.build_apply_script(self.paths())
        self.assertIn("tar -xzf", script)
        self.assertIn('emit gate_present', script)
        self.assertIn('"$gate" > "$root/gate.out"', script)
        self.assertIn('emit run_rc "$run_rc"', script)
        self.assertIn('cat "$root/gate.out"', script)
        self.assertNotIn("systemd-run", script)
        self.assertNotIn("/dev/", script)

    def test_rollback_removes_only_the_owned_stage_root(self) -> None:
        script = gate.build_rollback_script(self.paths())
        self.assertIn('rm -rf -- "$stage_root"', script)
        self.assertIn("stage_absent", script)

    def test_facts_contract_is_fixed(self) -> None:
        for key in (
            "isolated_backend",
            "startup_replay",
            "lease_apply",
            "expiry_release",
            "identity_mismatch",
            "failure_release_all",
            "mouse_modes",
            "rollover",
            "soak",
            "physical_actions_invoked",
        ):
            self.assertIn(key, gate.APPLY_KEYS)
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
        source = (ROOT / "tools/stage_device_hid_isolated_gate.py").read_text()
        self.assertIn('"physicalActionsInvoked": False', source)
        self.assertIn('"persistentChangeScope": "none"', source)


if __name__ == "__main__":
    unittest.main()
