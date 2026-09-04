import subprocess
import sys
import unittest

import tools.capture_device_persistent_soak as soak


class CaptureDevicePersistentSoakTests(unittest.TestCase):
    def test_remote_soak_is_fixed_read_only_and_power_safe(self) -> None:
        source = soak.REMOTE_SOAK
        for required in (
            "samples=30",
            "interval=10",
            "unit_rss_kib",
            "NRestarts",
            "MainPID",
            "thermal_zone0/temp",
            "https://127.0.0.1/api/health",
            '"required":false',
            "https://127.0.0.1/api/inventory",
            "https://127.0.0.1/api/diagnostics",
            "https://127.0.0.1/api/stream/mjpeg",
            "tlsv1.2",
            "tlsv1.3",
        ):
            self.assertIn(required, source)
        for forbidden in (
            "systemctl start",
            "systemctl stop",
            "systemctl restart",
            "systemctl enable",
            "systemctl disable",
            "reboot",
            "shutdown",
            "poweroff",
            "/api/control",
            "/api/atx",
            "/api/hid",
            "rm -rf",
        ):
            self.assertNotIn(forbidden, source)

    def test_remote_soak_is_valid_posix_shell(self) -> None:
        result = subprocess.run(
            ["sh", "-n"],
            input=soak.REMOTE_SOAK.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_parser_requires_exact_expected_facts(self) -> None:
        tokens = {
            "architecture": "aarch64",
            "samples": "30",
            "duration_seconds": "300",
            "health_requests": "30",
            "setup_requests": "30",
            "redirect_requests": "6",
            "inventory_denials": "30",
            "diagnostic_denials": "30",
            "video_denials": "30",
            "pids_stable": "yes",
            "restarts_stable": "yes",
            "units_persistent": "yes",
            "ports_exact": "yes",
            "setup_closed": "yes",
            "tls12": "passed",
            "tls13": "passed",
            "kvmd_active": "active",
            "control_boundary_absent": "yes",
            "rootfs_mode": "rw",
            "broker_rss_max_kib": "12000",
            "video_rss_max_kib": "13000",
            "server_rss_max_kib": "14000",
            "nginx_rss_max_kib": "15000",
            "thermal_min_millidegrees": "40000",
            "thermal_max_millidegrees": "50000",
        }
        raw = "".join(f"{key}={tokens[key]}\n" for key in sorted(tokens)).encode()
        self.assertEqual(set(soak.parse_facts(raw)), soak.FACT_KEYS)
        with self.assertRaises(ValueError):
            soak.parse_facts(raw + b"private=value\n")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_persistent_soak.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
