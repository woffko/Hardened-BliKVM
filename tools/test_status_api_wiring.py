from pathlib import Path
import json
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import status_api_wiring as gate
from tools import stage_status_api_wiring as driver


class StatusApiWiringTests(unittest.TestCase):
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
