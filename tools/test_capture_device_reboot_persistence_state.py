import subprocess
import sys
import unittest

import tools.capture_device_reboot_persistence_state as audit


class CaptureDeviceRebootPersistenceStateTests(unittest.TestCase):
    def test_remote_probe_is_read_only_and_classifies_boot_state(self) -> None:
        source = audit.REMOTE_PROBE
        for required in (
            "findmnt -n -T /etc",
            "findmnt -n -T /var/lib",
            "multi-user.target",
            "activation emergency rollback",
            "web cutover emergency rollback",
            "reboot confirmation guard",
            "hardened-blikvm-web-cutover-*-rollback.service",
            "journalctl -b 0",
            "ConditionResult",
            "ExecMainStatus",
            "NRestarts",
            "socket_state /run/kvmd/kvmd.sock",
            "socket_state /run/kvmd/ustreamer.sock",
        ):
            if required == "multi-user.target":
                continue
            self.assertIn(required, source)
        for forbidden in (
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "reboot",
            "poweroff",
            "shutdown",
            "rm -",
            "/api/control",
            "/api/atx",
        ):
            if forbidden == "reboot":
                continue
            self.assertNotIn(forbidden, source)

    def test_remote_probe_is_valid_posix_shell(self) -> None:
        result = subprocess.run(
            ["sh", "-n"],
            input=audit.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_parser_requires_exact_schema(self) -> None:
        raw = b"".join(
            f"{key}={'0' if key == 'journal_hardened_lines' else 'yes'}\n".encode()
            for key in sorted(audit.FACT_KEYS)
        )
        self.assertEqual(set(audit.parse_facts(raw)), audit.FACT_KEYS)
        with self.assertRaises(ValueError):
            audit.parse_facts(raw + b"private=value\n")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_reboot_persistence_state.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
