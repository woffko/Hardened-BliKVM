import inspect
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import enable_update_services as gate
from tools import stage_initialized_updater_upgrade as driver


class EnableUpdateServicesTests(unittest.TestCase):
    def test_operational_snapshot_rejects_invalid_slot_floor_binding_before_io(self) -> None:
        for slot, floor in (("../../untrusted", 2), (gate.s.bootstrap.SLOT, 2)):
            with patch.object(gate.s.bootstrap, "trusted_code_and_config") as load, self.assertRaises(ValueError):
                gate.enabled_snapshot(expected_slot=slot, expected_floor=floor, check_initial=False)
            load.assert_not_called()

    def test_status_query_does_not_own_or_remove_watchdog_runtime_directory(self) -> None:
        source = inspect.getsource(gate.current_status)
        self.assertNotIn("--property=RuntimeDirectory=", source)
        self.assertNotIn("rmdir", source)
        self.assertIn('"status"', source)
        self.assertNotIn('"activate"', source)

    def test_changed_tmpfiles_rule_is_preserved(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            rule = Path(temporary) / "tmpfiles.conf"
            rule.write_text("unrelated configuration")
            with patch.object(gate, "RULE", rule), patch.object(gate, "checked", return_value=b""), self.assertRaises(ValueError):
                gate.disable(False)
            self.assertEqual(rule.read_text(), "unrelated configuration")

    def test_committed_rollback_cannot_disable_retained_infrastructure(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(gate.upgrade, "load_plan", return_value={"phase": "committed"}), \
                 patch.object(gate, "disable") as disable, patch.object(gate, "verify") as verify, \
                 patch.object(gate.upgrade, "disarm"):
                result = gate.transition("rollback", Path(temporary), False)
                disable.assert_not_called()
                self.assertTrue(verify.call_args.args[2])
                self.assertEqual(result["phase"], "committed")

    def test_package_rehearsal_is_not_infrastructure_rehearsal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            receipt = {"probe": "initialized-updater-upgrade", "action": "rehearse", "result": "passed",
                       "packageHashes": gate.upgrade.PACKAGE_HASHES, "retainedVersion": gate.upgrade.VERSIONS["new"],
                       "guardRemoved": True, "controllerStagingRemoved": True}
            path.write_text(json.dumps(receipt))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                with self.assertRaises(ValueError):
                    driver.validate_rehearsal(str(path), enable_services=True)
                receipt["probe"] = "updater-infrastructure"
                path.write_text(json.dumps(receipt))
                driver.validate_rehearsal(str(path), enable_services=True)

    def test_provisioning_is_guarded_and_does_not_install_or_activate(self) -> None:
        source = inspect.getsource(gate.prepare)
        self.assertLess(source.index("upgrade.arm(root)"), source.index("upgrade.new_file(RULE"))
        self.assertLess(source.index('"systemd-tmpfiles"'), source.index('"enable", "--now"'))
        self.assertNotIn('"activate"', source)
        self.assertNotIn("upgrade.install(", source)
        self.assertNotIn('"restart"', source)


if __name__ == "__main__":
    unittest.main()
