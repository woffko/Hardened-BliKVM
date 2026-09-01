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
        self.assertEqual(
            paths.video_unit_path,
            "/run/systemd/system/hardened-blikvm-video-proxy.service",
        )

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
            gate.build_runtime_script(paths, bundle, 18081, board_observations_only=True),
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
        video = gate.build_video_dropin(paths)
        server = gate.build_server_dropin(paths, 18081)
        self.assertIn("LoadCredential=kvmd-password:", broker)
        self.assertNotIn("LoadCredential", server)
        self.assertIn("User=daemon", broker)
        self.assertIn("User=nobody", server)
        self.assertIn("User=www-data", video)
        self.assertIn("SupplementaryGroups=kvmd", video)
        self.assertNotIn("LoadCredential", video)
        self.assertIn("BLI_KVM_VIDEO_SOCKET=/run/hardened-blikvm-video/mjpeg.sock", server)
        self.assertIn("SocketBindAllow=tcp:18081", server)

    def test_runtime_video_gate_is_read_only_and_session_bound(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        runtime = gate.build_runtime_script(paths, bundle, 18081)
        self.assertIn("phase=verify-video-unit", runtime)
        self.assertIn("DevicePolicy --value hardened-blikvm-video-proxy.service", runtime)
        self.assertIn("/api/stream/mjpeg", runtime)
        self.assertIn("phase=video-concurrency", runtime)
        self.assertIn("capture_failure_class", runtime)
        self.assertIn("video-concurrency-both-", runtime)
        self.assertIn("video-concurrency-a-", runtime)
        self.assertIn("video-concurrency-b-", runtime)
        self.assertIn("phase=video-session-revocation", runtime)
        self.assertIn("phase=inventory-thermal", runtime)
        self.assertIn('"schemaVersion":3', runtime)
        self.assertIn("temperatureMillidegreesCelsius", runtime)
        self.assertIn("phase=thermal-capability", runtime)
        self.assertIn('"thermal":{"availability":"read_only"', runtime)
        self.assertIn("offsetFromSystemSeconds", runtime)
        self.assertIn("phase=inventory-rtc", runtime)
        self.assertIn('"rtc":{"availability":"read_only"', runtime)
        self.assertIn("phase=diagnostics-unauthorized", runtime)
        for phase in (
            "diagnostics-request",
            "diagnostics-schema",
            "diagnostics-history",
            "diagnostics-temperature",
            "diagnostics-rtc",
            "diagnostics-components",
            "diagnostics-privacy",
        ):
            self.assertIn(f"phase={phase}", runtime)
        self.assertIn("/api/diagnostics", runtime)
        self.assertIn('"availability":"current_only"', runtime)
        self.assertIn("persistence_not_provisioned", runtime)
        self.assertIn("diagnostics_temperature_bounded=yes", runtime)
        self.assertIn("diagnostics_rtc_bounded=yes", runtime)
        self.assertIn("diagnostics_component_states=typed", runtime)
        self.assertNotIn("/api/atx", runtime)
        self.assertNotIn("/api/hid", runtime)

    def test_board_observation_profile_is_explicit_and_skips_only_stream_checks(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        runtime = gate.build_runtime_script(
            paths, bundle, 18081, board_observations_only=True
        )
        self.assertIn("runtime_profile=board-observations-only", runtime)
        self.assertIn('if [ "$runtime_profile" = board-observations-only ]', runtime)
        self.assertIn("video_status=skipped", runtime)
        self.assertIn("video_session_revocation=skipped", runtime)
        self.assertIn("phase=inventory-thermal", runtime)
        self.assertIn("inventory_requests=20", runtime)

    def test_runtime_validator_rejects_out_of_range_thermal_observation(self) -> None:
        facts = {key: "placeholder" for key in gate.RUN_KEYS}
        facts.update(
            {
                "bundle_sha256": "a" * 64,
                "video_frames": "2",
                "thermal_millidegrees": "-40001",
            }
        )
        with self.assertRaisesRegex(ValueError, "thermal invariant"):
            gate.validate_run(facts, "a" * 64)

    def test_runtime_validator_rejects_out_of_range_rtc_observation(self) -> None:
        facts = {key: "placeholder" for key in gate.RUN_KEYS}
        facts.update(
            {
                "bundle_sha256": "a" * 64,
                "video_frames": "2",
                "thermal_millidegrees": "42000",
                "rtc_unix_seconds": "1759489393",
                "rtc_offset_seconds": "-315576001",
            }
        )
        with self.assertRaisesRegex(ValueError, "RTC invariant"):
            gate.validate_run(facts, "a" * 64)

    def test_jpeg_counter_is_executable_with_posix_awk(self) -> None:
        result = subprocess.run(
            ["awk", gate.JPEG_COUNTER_AWK],
            input=b"255 216 1 2 255 216 3 4\n",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(result.stdout, b"2\n")

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
        self.assertEqual(policy["runtimeProfile"], "full")
        self.assertEqual(
            gate.policy_receipt(board_observations_only=True)["runtimeProfile"],
            "board-observations-only",
        )


if __name__ == "__main__":
    _ = unittest.main()
