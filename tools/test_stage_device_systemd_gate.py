from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_systemd_gate as gate
import tools.stage_device_prototype as prototype


class StageDeviceSystemdGateTests(unittest.TestCase):
    def _paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0123456789abcdef")

    def test_paths_are_exact_and_marker_protected(self) -> None:
        paths = self._paths()
        self.assertEqual(
            paths.test_root,
            "/opt/hardened-blikvm-test-0123456789abcdef",
        )
        self.assertEqual(
            paths.owner_marker,
            "/opt/.hardened-blikvm-test-owner-0123456789abcdef",
        )
        self.assertTrue(paths.rollback_script.startswith(paths.test_root + "/"))

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_systemd_gate.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        scripts = (
            gate.build_rollback_script(paths),
            gate.build_preflight_script(paths, 10_000_000, 18081),
            gate.build_runtime_script(paths, bundle, 18081),
            gate.build_cleanup_script(paths),
            gate.build_final_script(paths, 18081),
        )
        for script in scripts:
            result = subprocess.run(
                ["sh", "-n"],
                input=script.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_dropins_keep_credentials_out_of_server(self) -> None:
        paths = self._paths()
        broker = gate.build_broker_dropin(paths)
        server = gate.build_server_dropin(paths, 18081)
        self.assertIn("LoadCredential=kvmd-password:", broker)
        self.assertNotIn("LoadCredential", server)
        self.assertIn("User=daemon", broker)
        self.assertIn("User=nobody", server)
        self.assertIn("SocketBindAllow=tcp:18081", server)

    def test_rollback_refuses_unknown_htpasswd_state(self) -> None:
        paths = self._paths()
        rollback = gate.build_rollback_script(paths)
        self.assertIn('elif [ "$current_sha" != "$before_sha" ]', rollback)
        self.assertIn("exit 3", rollback)
        self.assertNotIn("kvmd-htpasswd del", rollback)
        cleanup = gate.build_cleanup_script(paths)
        self.assertIn('[ "$current" = "$before" ] || exit 4', cleanup)

    def test_policy_excludes_physical_and_kvmd_restart(self) -> None:
        policy = gate.policy_receipt()
        self.assertIs(policy["physicalActionsInvoked"], False)
        self.assertIs(policy["kvmdRestarted"], False)
        self.assertEqual(
            policy["persistentChangeScope"], "temporary-and-rollback-required"
        )


if __name__ == "__main__":
    _ = unittest.main()
