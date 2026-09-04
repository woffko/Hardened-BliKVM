from pathlib import Path
import subprocess
import sys
import unittest


ROOT = Path(__file__).resolve().parent.parent


class CaptureRuntimeProvisionTests(unittest.TestCase):
    def test_preflight_wrapper_has_only_read_only_remote_calls(self) -> None:
        source = (ROOT / "tools/capture_runtime_provision_preflight.py").read_text()
        self.assertIn("retain.build_final_script", source)
        self.assertIn("provision.build_preflight_script", source)
        self.assertIn('"readOnly": True', source)
        self.assertNotIn("provision.build_prepare_script", source)
        self.assertNotIn("provision.build_apply_script", source)
        self.assertNotIn("provision.build_commit_script", source)
        self.assertNotIn("provision.build_cleanup_script", source)

    def test_inspection_wrapper_has_only_final_read(self) -> None:
        source = (ROOT / "tools/capture_runtime_provisioned.py").read_text()
        self.assertIn("provision.build_final_script", source)
        self.assertIn('"readOnly": True', source)
        self.assertIn('"secretEmitted": False', source)
        self.assertNotIn("provision.build_preflight_script", source)
        self.assertNotIn("provision.build_prepare_script", source)
        self.assertNotIn("provision.build_apply_script", source)
        self.assertNotIn("provision.build_commit_script", source)
        self.assertNotIn("provision.build_cleanup_script", source)

    def test_prepare_gate_never_calls_live_apply_or_commit(self) -> None:
        source = (
            ROOT / "tools/stage_device_runtime_provision_prepare.py"
        ).read_text()
        self.assertIn("retain.build_final_script", source)
        self.assertIn("provision.build_preflight_script", source)
        self.assertIn("provision.build_prepare_script", source)
        self.assertIn("provision.build_cleanup_script", source)
        self.assertNotIn("provision.build_apply_script", source)
        self.assertNotIn("provision.build_commit_script", source)
        self.assertIn('"productionMutation": False', source)
        self.assertIn('"secretEmitted": False', source)

    def test_direct_entrypoints_load_project_package(self) -> None:
        for path in (
            "tools/capture_runtime_provision_preflight.py",
            "tools/capture_runtime_provisioned.py",
            "tools/stage_device_runtime_provision_prepare.py",
        ):
            result = subprocess.run(
                [sys.executable, path, "--help"],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
