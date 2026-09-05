from pathlib import Path
import json
import hashlib
import copy
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import Mock, patch

from tools import status_api_wiring as gate
from tools import stage_status_api_wiring as driver


class StatusApiWiringTests(unittest.TestCase):
    def test_retained_reboot_compares_state_and_never_reissues_request(self) -> None:
        snapshot: dict[str, object] = {"state": {"file": "hash"}, "managerStatus": {"floorSequence": 5}, "nginxUnit": "unit",
                    "runtime": {"account": "a", "nginxConfig": "n", "certificate": "c", "tlsKey": "k", "trust": "t",
                                "brokerRestarts": "0", "videoRestarts": "0"}}
        before: dict[str, object] = {"result": "inspect-retained", "guardRoot": "guard", "bootIdSha256": "a" * 64,
                  "dropinHashes": {"current": "hash"}, "snapshot": snapshot,
                  "persistentWiringVerified": True, "startupOrderVerified": True, "guardRemoved": True}
        after: dict[str, object] = {**copy.deepcopy(before), "bootIdSha256": "b" * 64}
        driver.compare_retained_boot(before, after)
        changes: list[dict[str, object]] = [{"bootIdSha256": "a" * 64}, {"bootIdSha256": None}, {"dropinHashes": {}}, {"startupOrderVerified": False}]
        for changed in changes:
            with self.assertRaises(ValueError):
                driver.compare_retained_boot(before, {**after, **changed})
        bad: dict[str, object] = {**after, "snapshot": {**snapshot, "managerStatus": {"floorSequence": 4}}}
        with self.assertRaises(ValueError):
            driver.compare_retained_boot(before, bad)
        invoke = Mock(side_effect=[(0, before), (1, {}), (0, after)])
        reboot = Mock(return_value=255)
        with patch.object(driver.time, "sleep"):
            receipt: dict[str, object] = {}
            self.assertEqual(driver.retained_reboot_test(invoke, reboot, receipt), after)
            self.assertTrue(receipt["persistentRebootVerified"])
        reboot.assert_called_once_with()

    def test_retention_requires_current_controller_boot_browser_and_cleanup(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = "/var/lib/" + driver.persistent_native.PREFIX + "a" * 24
            boot_path = Path(temporary) / "boot.json"
            browser_path = Path(temporary) / "browser.json"
            cleanup_path = Path(temporary) / "cleanup.json"
            base = {"probe": "persistent-status-api-wiring", "slotId": gate.SLOT, "guardRoot": root, "result": "passed"}
            proof = {"result": "verify-boot", "guardRoot": root, "bootChanged": True,
                     "restoredBeforeServer": True, "runtimeVerified": True,
                     "controllerSha256": hashlib.sha256(Path(driver.persistent_native.__file__).read_bytes()).hexdigest(),
                     "status": {"floorSequence": 5, "generation": 22, "pending": None,
                                "activeSlotId": gate.SLOT, "confirmedSlotId": gate.SLOT}}
            boot = {**base, "action": "reboot-test", "softwareRebootAttempted": True, "device": proof}
            browser = {"statusAcceptance": {"enabled": False, "desktop": True, "mobile": True,
                                             "repeatedReads": 0, "refreshChecks": 0}}
            cleanup = {**base, "action": "cleanup", "controllerStagingRemoved": True,
                       "device": {"result": "cleanup", "phase": "restored", "runtimeVerified": True}}
            boot_path.write_text(json.dumps(boot))
            browser_path.write_text(json.dumps(browser))
            cleanup_path.write_text(json.dumps(cleanup))
            def private_file(value: str) -> Path:
                return Path(value)
            with patch.object(driver.transport, "validate_private_file", side_effect=private_file), patch.object(driver, "browser_gate"):
                self.assertEqual(driver.boot_acceptance(str(boot_path), str(browser_path), str(cleanup_path)), root)
                for change in ({"controllerSha256": "f" * 64}, {"bootChanged": False}, {"restoredBeforeServer": False}):
                    boot_path.write_text(json.dumps({**boot, "device": {**proof, **change}}))
                    with self.assertRaises(ValueError):
                        driver.boot_acceptance(str(boot_path), str(browser_path), str(cleanup_path))
                boot_path.write_text(json.dumps(boot))
                cleanup_path.write_text(json.dumps({**cleanup, "controllerStagingRemoved": False}))
                with self.assertRaises(ValueError):
                    driver.boot_acceptance(str(boot_path), str(browser_path), str(cleanup_path))
                with self.assertRaises(ValueError):
                    driver.boot_acceptance(None, None, None)

    def test_reboot_is_sent_once_even_if_observation_times_out(self) -> None:
        root = "/var/lib/" + driver.persistent_native.PREFIX + "a" * 24
        before = {"result": "preflight-reboot", "guardRoot": root, "bootIdSha256": "a" * 64, "controllerSha256": "c" * 64}
        after = {"result": "verify-boot", "guardRoot": root, "bootChanged": True, "restoredBeforeServer": True,
                 "runtimeVerified": True, "beforeBootIdSha256": "a" * 64, "afterBootIdSha256": "b" * 64,
                 "controllerSha256": "c" * 64}
        for request_result in (0, 255, subprocess.TimeoutExpired("reboot", 20)):
            invoke = Mock(side_effect=[(0, before), subprocess.TimeoutExpired("observe", 30), (0, after)])
            reboot = Mock(side_effect=request_result) if isinstance(request_result, Exception) else Mock(return_value=request_result)
            with patch.object(driver.time, "sleep"):
                self.assertEqual(driver.reboot_test(invoke, reboot, {}), after)
            reboot.assert_called_once_with()
            self.assertEqual([call.args[0] for call in invoke.call_args_list], ["preflight-reboot", "verify-boot", "verify-boot"])
        reboot = Mock()
        with self.assertRaises(ValueError):
            driver.reboot_test(Mock(return_value=(1, {})), reboot, {})
        reboot.assert_not_called()
        reboot = Mock(return_value=0)
        with patch.object(driver.time, "monotonic", side_effect=[0, 0, 300]), patch.object(driver.time, "sleep"), self.assertRaises(ValueError):
            driver.reboot_test(Mock(side_effect=[(0, before), (1, {})]), reboot, {})
        reboot.assert_called_once_with()

    def test_persistent_checkpoint_cannot_reuse_temporary_guard(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            root = "/var/lib/" + driver.persistent_native.PREFIX + "b" * 24
            value = {"probe": "persistent-status-api-wiring", "slotId": gate.SLOT, "guardRoot": root}
            path.write_text(json.dumps(value))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                self.assertEqual(driver.checkpoint(str(path), persistent=True), root)
                with self.assertRaises(ValueError):
                    driver.checkpoint(str(path))
                path.write_text(json.dumps({**value, "guardRoot": "/var/lib/" + gate.PREFIX + "b" * 24}))
                with self.assertRaises(ValueError):
                    driver.checkpoint(str(path), persistent=True)

    def test_persistent_browser_proof_requires_complete_status_checks(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "browser.json"
            acceptance = {"enabled": True, "desktop": True, "mobile": True, "repeatedReads": 3, "refreshChecks": 2}
            path.write_text(json.dumps({"statusAcceptance": acceptance}))
            with patch.object(driver, "browser_gate") as basic, patch.object(driver.transport, "validate_private_file", return_value=path):
                driver.persistent_browser_gate(str(path), "guard")
                basic.assert_called_once_with(str(path), "guard", "enabled")
                path.write_text(json.dumps({"statusAcceptance": {**acceptance, "mobile": False}}))
                with self.assertRaises(ValueError):
                    driver.persistent_browser_gate(str(path), "guard")

    def test_clean_verifier_never_mutates_and_rejects_leftovers(self) -> None:
        root = "/var/lib/" + gate.PREFIX + "a" * 24
        with patch.object(gate.infrastructure, "exists", return_value=False), \
             patch.object(gate, "snapshot", return_value={"managerStatus": {"floorSequence": 5}}) as snapshot, \
             patch.object(gate, "checked") as command:
            self.assertTrue(gate.verify_clean(root)["guardRemoved"])
            snapshot.assert_called_once_with(False)
            command.assert_not_called()
            with patch.object(gate.infrastructure, "exists", return_value=True), self.assertRaises(ValueError):
                gate.verify_clean(root)
            with self.assertRaises(ValueError):
                gate.verify_clean("/var/lib")

    def test_driver_entrypoint_and_checkpoint_binding(self) -> None:
        result = subprocess.run([sys.executable, "tools/stage_status_api_wiring.py", "--help"], capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            root = "/var/lib/" + gate.PREFIX + "a" * 24
            value = {"probe": "installed-status-api-wiring", "slotId": gate.SLOT, "guardRoot": root}
            path.write_text(json.dumps(value))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                self.assertEqual(driver.checkpoint(str(path)), root)
                path.write_text(json.dumps({**value, "slotId": "wrong"}))
                with self.assertRaises(ValueError):
                    driver.checkpoint(str(path))
            for invalid in ("/var/lib", root + "/../other", root + ";command"):
                with self.assertRaises(ValueError):
                    driver.validate_guard(invalid)

    def test_runtime_dropins_only_add_read_status(self) -> None:
        self.assertEqual(len(gate.dropins()), 2)
        for path in gate.dropins():
            self.assertTrue(path.is_relative_to("/run/systemd/system"))
        content = gate.DROPIN_BYTES.decode()
        self.assertIn("Wants=" + gate.SOCKET, content)
        self.assertIn("Environment=" + gate.ENVIRONMENT, content)
        for forbidden in ("Requires=", "ReadWritePaths", "LoadCredential", "ExecStart", "apply", "recover"):
            self.assertNotIn(forbidden, content)

    def test_guard_names_cannot_select_broad_cleanup_roots(self) -> None:
        for path in ("/", "/var/lib", "/tmp/" + gate.PREFIX + "a" * 24, "/var/lib/" + gate.PREFIX + "invalid"):
            with self.assertRaises(ValueError):
                gate.guard_root(path)

    def test_deadline_calls_only_fixed_restore_controller(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        units = gate.units(root)
        self.assertIn("OnActiveSec=600", units[root.name + "-deadline.timer"])
        self.assertIn(f"{root}/controller.py restore --guard-root {root}", units[root.name + "-restore.service"])
        self.assertNotIn("[Install]", "\n".join(units.values()))

    def test_restore_restarts_server_after_dropin_removal_then_stops_socket(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = tuple(root / (name + ".d") / gate.DROPIN_NAME for name in ("current", "candidate"))
            for path in paths:
                path.parent.mkdir()
                path.parent.chmod(0o755)
                path.write_bytes(gate.DROPIN_BYTES)
                path.chmod(0o644)
            with patch.object(gate, "dropins", return_value=paths), patch.object(gate, "checked", return_value=b"") as checked, \
                 patch.object(gate, "await_snapshot") as verify, patch.object(gate, "save"):
                plan: dict[str, object] = {"before": {}, "phase": "wired"}
                gate.restore(root, plan)
                self.assertTrue(all(not path.parent.exists() for path in paths))
                self.assertEqual([call.args[0] for call in checked.call_args_list], [
                    ["systemctl", "daemon-reload"], ["systemctl", "restart", gate.s.CURRENT_SERVER],
                    ["systemctl", "stop", gate.SOCKET],
                ])
                verify.assert_called_once_with(False, {})
                self.assertEqual(plan["phase"], "restored")

    def test_restore_preserves_foreign_override(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            path = root / "override.conf"
            path.write_bytes(b"foreign")
            with patch.object(gate, "dropins", return_value=(path,)), patch.object(gate, "checked") as checked:
                with self.assertRaises(ValueError):
                    gate.restore(root, {"before": {}})
                self.assertEqual(path.read_bytes(), b"foreign")
                checked.assert_not_called()


if __name__ == "__main__":
    unittest.main()
