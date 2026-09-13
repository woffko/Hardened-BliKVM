from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "tools/playwright/restricted_control_gui_probe.mjs"


class RestrictedControlGuiProbeTests(unittest.TestCase):
    def test_probe_is_syntax_valid_and_secret_stdin_only(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        result = subprocess.run(
            ["node", "--check", str(PROBE)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("for await (const chunk of process.stdin)", source)
        self.assertIn('password = ""', source)
        self.assertNotIn("process.env", source)
        self.assertNotIn("10.0.", source)
        self.assertNotIn("/home/w0w", source)
        self.assertIn("mode: 0o600", source)

    def test_probe_drives_the_confirmed_restricted_path(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        for required in (
            "Login - Hardened BliKVM",
            "Remote Desktop - Hardened BliKVM",
            '"/api/hid"',
            '"/api/auth/login"',
            "Confirm and control",
            'page.keyboard.press("KeyA")',
            'path.resolve("evidence/private")',
        ):
            self.assertIn(required, source)


if __name__ == "__main__":
    unittest.main()
