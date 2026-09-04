import subprocess
import sys
import unittest

import tools.capture_device_web_cutover_failure as failure


class CaptureDeviceWebCutoverFailureTests(unittest.TestCase):
    def test_remote_probe_returns_only_fixed_classes(self) -> None:
        source = failure.REMOTE_JOURNAL_PROBE
        self.assertIn("journalctl -u hardened-blikvm-nginx.service", source)
        self.assertIn("--since '-5 minutes'", source)
        self.assertIn("-n 120", source)
        self.assertIn("journal_sha256", source)
        self.assertNotIn("systemctl start", source)
        self.assertNotIn("systemctl stop", source)
        self.assertNotIn("rm ", source)
        self.assertNotIn("cat /", source)

    def test_remote_probe_is_valid_posix_shell(self) -> None:
        result = subprocess.run(
            ["sh", "-n"],
            input=failure.REMOTE_JOURNAL_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_parser_requires_exact_bounded_schema(self) -> None:
        raw = b"".join(
            f"{key}={'1' if key == 'journal_lines' else 'no'}\n".encode()
            for key in sorted(failure.JOURNAL_KEYS)
        ) + f"journal_sha256={'a' * 64}\n".encode()
        facts, digest = failure.parse_journal_facts(raw)
        self.assertEqual(set(facts), failure.JOURNAL_KEYS)
        self.assertEqual(digest, "a" * 64)
        with self.assertRaises(ValueError):
            failure.parse_journal_facts(raw + b"raw=value\n")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_web_cutover_failure.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
