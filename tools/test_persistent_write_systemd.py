from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

from tools import persistent_write_systemd as native


class AdmissionTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path("/var/lib") / (native.contract.PREFIX + "a" * 24)
        self.actions = native.Admission(self.root)

    def test_boot_closure_never_waits_for_downstream_units(self) -> None:
        with patch.object(self.actions, "require_revoked"), patch.object(native.fence, "install"), \
                patch.object(self.actions, "require_masked_and_closed") as verify, \
                patch.object(native.observer.upload, "checked", return_value=b"") as run:
            self.actions.fence_admission(boot=True)
        self.assertEqual([call.args[0] for call in run.call_args_list], [
            ["systemctl", "stop", "--no-block", *native.contract.SOCKETS],
            ["systemctl", "daemon-reload"],
            ["systemctl", "stop", "--no-block", *native.contract.SOCKETS]])
        verify.assert_not_called()

    def test_failed_first_stop_still_masks_but_is_not_success(self) -> None:
        with patch.object(self.actions, "require_revoked"), patch.object(native.fence, "install") as install, \
                patch.object(self.actions, "require_masked_and_closed"), \
                patch.object(native.observer.upload, "checked", side_effect=[
                    subprocess.TimeoutExpired("systemctl", 20), b"", b""]):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.actions.fence_admission(boot=False)
        install.assert_called_once_with(self.root)

    def test_partial_mask_failure_still_attempts_socket_stop(self) -> None:
        with patch.object(self.actions, "require_revoked"), \
                patch.object(native.fence, "install", side_effect=OSError("full")), \
                patch.object(native.observer.upload, "checked", return_value=b"") as run:
            with self.assertRaises(OSError):
                self.actions.fence_admission(boot=False)
        self.assertEqual(len(run.call_args_list), 2)
        self.assertTrue(all(call.args[0] == ["systemctl", "stop", *native.contract.SOCKETS]
                            for call in run.call_args_list))

    def test_no_revocation_means_no_service_or_file_mutation(self) -> None:
        with patch.object(native.decisions, "read", return_value=None), \
                patch.object(native.fence, "install") as install, \
                patch.object(native.observer.upload, "checked") as run:
            with self.assertRaises(ValueError):
                self.actions.fence_admission(boot=False)
        install.assert_not_called()
        run.assert_not_called()

    def test_preparer_descendants_or_jobs_prevent_unmask(self) -> None:
        name, _ = native.contract.preparer_unit(self.root)
        clean = {"LoadState": "loaded", "FragmentPath": str(native.files.UNIT_ROOT / name),
            "DropInPaths": "", "ActiveState": "inactive", "MainPID": "0", "ControlPID": "0", "Job": "0", "ControlGroup": ""}
        for field, value in (("ControlGroup", "/system.slice/owned.service"), ("Job", "44"),
                             ("ControlPID", "10"), ("ActiveState", "activating"), ("DropInPaths", "/foreign.conf")):
            with self.subTest(field=field), patch.object(native.files, "safe_directory"), \
                    patch.object(native.files, "safe_file"), \
                    patch.object(native.observer.upload, "properties", return_value={**clean, field: value}):
                with self.assertRaises(ValueError):
                    self.actions.require_preparer_quiescent()

    def test_failed_post_unmask_verification_reinstalls_fence(self) -> None:
        with patch.object(self.actions, "require_revoked"), patch.object(self.actions, "require_preparer_quiescent"), \
                patch.object(native.files, "validate_backup"), patch.object(native.files, "exists", return_value=False), \
                patch.object(self.actions, "require_masked_and_closed"), patch.object(native.fence, "remove"), \
                patch.object(self.actions, "reload_manager"), \
                patch.object(native.observer, "require_closed_admission", side_effect=ValueError("socket reopened")), \
                patch.object(self.actions, "fence_admission") as reinstall:
            with self.assertRaisesRegex(ValueError, "socket reopened"):
                self.actions.release_admission_fence({})
        reinstall.assert_called_once_with(boot=False)
