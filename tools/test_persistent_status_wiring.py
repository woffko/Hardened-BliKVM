from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_status_wiring as gate


class PersistentStatusWiringTests(unittest.TestCase):
    def test_dropins_are_fixed_persistent_paths(self) -> None:
        self.assertEqual(len(gate.dropins()), 2)
        self.assertTrue(all(path.is_relative_to("/etc/systemd/system") for path in gate.dropins()))
        self.assertEqual(len(gate.HELPERS), len(set(gate.HELPERS)))

    def test_boot_restore_does_not_wait_for_servers_or_socket(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for phase in ("prepared", "wired", "restored"):
                with patch.object(gate, "load", return_value={"phase": phase, "before": {}}), \
                     patch.object(gate.common, "remove_matching_dropins") as remove, \
                     patch.object(gate.u, "sync_directory"), patch.object(gate, "checked", return_value=b"") as checked, \
                     patch.object(gate, "verify") as verify, patch.object(gate.common, "save"), \
                     patch.object(gate, "disarm"):
                    result = gate.transition("restore", root, boot=True)
                    remove.assert_called_once()
                    self.assertEqual([call.args[0] for call in checked.call_args_list], [["systemctl", "daemon-reload"]])
                    self.assertTrue(verify.call_args.kwargs["boot"])
                    self.assertFalse(result["runtimeVerified"])
                    self.assertTrue(result["bootRecoveryPendingVerification"])

    def test_committed_recovery_preserves_dropins_and_services(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for boot in (False, True):
                with patch.object(gate, "load", return_value={"phase": "committed", "before": {}}), \
                     patch.object(gate.common, "remove_matching_dropins") as remove, \
                     patch.object(gate, "checked") as checked, patch.object(gate, "verify") as verify, \
                     patch.object(gate.common, "save"), patch.object(gate, "disarm"):
                    result = gate.transition("restore", root, boot=boot)
                    self.assertEqual(result["phase"], "committed")
                    remove.assert_not_called()
                    checked.assert_not_called()
                    self.assertTrue(verify.call_args.args[1])

    def test_commit_is_saved_before_disarm_and_requires_browser(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            events: list[object] = []

            def load(_root: Path) -> dict[str, object]:
                return {"phase": "wired", "before": {}}

            def save(_root: Path, plan: dict[str, object]) -> None:
                events.append(plan["phase"])

            with patch.object(gate, "load", side_effect=load), \
                 patch.object(gate, "verify"), \
                 patch.object(gate.common, "save", side_effect=save), \
                 patch.object(gate, "disarm", side_effect=OSError("interrupted disarm")):
                with self.assertRaises(ValueError):
                    gate.transition("commit", root)
                self.assertEqual(events, [])
                with self.assertRaises(OSError):
                    gate.transition("commit", root, browser_verified=True)
                self.assertEqual(events, ["committed"])

    def test_failed_restore_verification_keeps_guard_armed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(gate, "load", return_value={"phase": "wired", "before": {}}), \
                 patch.object(gate.common, "remove_matching_dropins"), patch.object(gate.u, "sync_directory"), \
                 patch.object(gate, "checked", return_value=b""), patch.object(gate, "verify", side_effect=ValueError("state drift")), \
                 patch.object(gate.common, "save") as save, patch.object(gate, "disarm") as disarm:
                with self.assertRaises(ValueError):
                    gate.transition("restore", root)
                save.assert_not_called()
                disarm.assert_not_called()


if __name__ == "__main__":
    unittest.main()
