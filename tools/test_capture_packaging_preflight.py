import subprocess
import sys
import unittest

import tools.capture_packaging_preflight as packaging


class PackagingPreflightTests(unittest.TestCase):
    def valid_facts(self) -> dict[str, str]:
        facts = {key: sorted(values)[0] for key, values in packaging.ENUMS.items()}
        facts.update({key: "1048576" for key in packaging.INTEGER_KEYS})
        return facts

    def test_parser_requires_exact_allowlisted_values(self) -> None:
        facts = self.valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in sorted(facts.items())).encode()
        self.assertEqual(packaging.parse_facts(raw), dict(sorted(facts.items())))

        with self.assertRaises(ValueError):
            packaging.parse_facts(raw + b"hostname=private\n")
        with self.assertRaises(ValueError):
            packaging.parse_facts(raw.replace(b"dpkg=available", b"dpkg=yes"))
        with self.assertRaises(ValueError):
            packaging.parse_facts(raw.replace(b"usr_available_bytes=1048576", b"usr_available_bytes=-1"))

    def test_remote_probe_is_read_only_and_bounded(self) -> None:
        probe = packaging.REMOTE_PROBE
        for required in (
            "available dpkg dpkg",
            "systemd-sysusers",
            "package_state",
            "/usr/libexec/hardened-blikvm",
            "/usr/share/hardened-blikvm/web",
            "/etc/hardened-blikvm",
            "port_18080",
            "port_18443",
            "kvmd_htpasswd_mode",
        ):
            self.assertIn(required, probe)
        for forbidden in (
            "apt ",
            "dpkg -i",
            "systemctl start",
            "systemctl stop",
            "systemctl restart",
            "systemctl enable",
            "systemctl disable",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, probe)
        for command in ("mkdir", "install", "rm", "mv", "cp", "chmod", "chown", "useradd"):
            self.assertNotRegex(probe, rf"(?m)^\s*{command}(?:\s|$)")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_packaging_preflight.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
