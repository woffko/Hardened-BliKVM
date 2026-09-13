from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "tools/probe_restricted_control.py"


class RestrictedControlProbeTests(unittest.TestCase):
    def test_probe_is_read_only_for_the_target(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        # The only report is an all-zero keyboard report, which translates to no
        # KVMD events, so the probe never injects visible input.
        self.assertIn("EMPTY_KEYBOARD = [0, 0, 0, 0, 0, 0, 0, 0]", source)
        self.assertIn('"mode": "keyboard"', source)
        self.assertNotIn("relative_mouse", source)
        self.assertNotIn("absolute_mouse", source)

    def test_probe_reads_the_csrf_token_from_the_login_body(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        self.assertIn('"csrfToken"', source)
        self.assertIn('headers["x-csrf-token"] = self.csrf_token', source)

    def test_probe_keeps_secrets_out_of_arguments_and_receipt(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        self.assertIn("sys.stdin.buffer.read", source)
        self.assertIn("--username-file", source)
        for forbidden in ("password = \"", "10.0.", "not-a-real", "/home/w0w"):
            self.assertNotIn(forbidden, source)
        self.assertIn("write_json_0600_atomic", source)

    def test_probe_requires_an_https_origin(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        self.assertIn('parts.scheme != "https"', source)
        self.assertIn('"Origin": self.origin', source)


if __name__ == "__main__":
    unittest.main()
