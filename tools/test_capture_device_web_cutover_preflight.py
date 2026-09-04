import subprocess
import sys
import unittest

import tools.capture_device_web_cutover_preflight as preflight


class CaptureDeviceWebCutoverPreflightTests(unittest.TestCase):
    def test_remote_probe_is_read_only_and_bounded(self) -> None:
        script = preflight.REMOTE_PROBE
        for required in (
            "systemctl is-active kvmd-nginx.service",
            "systemctl is-enabled",
            "/proc/$main_pid/cmdline",
            '"$nginx_binary" -T -c "$nginx_config_path"',
            "openssl s_client",
            "certificate_directive_count",
            "key_directive_count",
            "live_certificate_matches",
            "hardened_units",
            "control_boundary",
            "staging_absent",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "systemctl restart",
            "rm -",
            "mv ",
            "install ",
            "command -v rw",
            "reboot",
            "poweroff",
            "shutdown",
        ):
            self.assertNotIn(forbidden, script)

    def test_remote_probe_is_valid_posix_shell(self) -> None:
        result = subprocess.run(
            ["sh", "-n"],
            input=preflight.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_fact_parser_requires_exact_safe_schema(self) -> None:
        payload = b"".join(
            f"{key}=yes\n".encode() for key in sorted(preflight.SAFE_KEYS)
        )
        self.assertEqual(set(preflight.parse_safe_facts(payload)), preflight.SAFE_KEYS)
        with self.assertRaises(ValueError):
            preflight.parse_safe_facts(payload + b"private=value\n")
        with self.assertRaises(ValueError):
            preflight.parse_safe_facts(payload.replace(b"yes\n", b"bad\nline\n", 1))

    def test_partial_failure_parser_keeps_only_safe_facts(self) -> None:
        phase, facts = preflight.parse_partial_failure(
            b"architecture=aarch64\nrootfs_mode=rw\nfailure_phase=nginx-config\n"
        )
        self.assertEqual(phase, "nginx-config")
        self.assertEqual(
            facts, {"architecture": "aarch64", "rootfs_mode": "rw"}
        )
        self.assertEqual(
            preflight.parse_partial_failure(b"unexpected=value\n"),
            ("invalid-output", {}),
        )

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_web_cutover_preflight.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
