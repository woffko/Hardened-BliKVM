from pathlib import Path
import subprocess
import unittest

import tools.stage_device_broker_inventory as broker
import tools.stage_device_prototype as prototype


class StageDeviceBrokerInventoryTests(unittest.TestCase):
    def _paths(self) -> broker.BrokerPaths:
        return broker.broker_paths("a" * 64, "0123456789abcdef")

    def test_paths_are_bounded_and_marker_protected(self) -> None:
        paths = self._paths()
        self.assertTrue(paths.stage.root.startswith("/tmp/hardened-blikvm-prototype-"))
        self.assertEqual(paths.runtime_root, "/run/hardened-blikvm")
        self.assertEqual(
            paths.runtime_marker,
            "/run/.hardened-blikvm-owner-0123456789abcdef",
        )
        self.assertTrue(paths.credentials.startswith(paths.stage.root + "/"))

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        scripts = (
            broker.build_preflight_script(paths, 10_000_000, 18081),
            broker.build_credential_upload_script(paths),
            broker.build_runtime_script(paths, bundle, 18081),
            broker.build_cleanup_script(paths),
            broker.build_final_script(paths, 18081),
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

    def test_runtime_script_has_read_only_inventory_boundary(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        script = broker.build_runtime_script(paths, bundle, 18081)
        self.assertIn("/api/inventory", script)
        for forbidden in (
            "/api/atx/click",
            "/api/atx/power",
            "/api/hid/events",
            "/api/msd/set_params",
            "reboot",
            "poweroff",
        ):
            self.assertNotIn(forbidden, script)

    def test_failure_parser_accepts_only_allowlisted_fields(self) -> None:
        self.assertEqual(
            broker.parse_failure(
                b"failure_phase=inventory\nfailure_cleanup=removed\n"
            ),
            ("inventory", True),
        )
        self.assertEqual(broker.parse_failure(b"password=unsafe\n"), (None, None))

    def test_policy_records_transition_only_scope(self) -> None:
        policy = broker.policy_receipt()
        self.assertEqual(policy["kvmdAccess"], "five-read-only-GET-endpoints")
        self.assertIs(policy["physicalActionsInvoked"], False)
        self.assertIs(policy["persistentPathsChanged"], False)


if __name__ == "__main__":
    _ = unittest.main()
