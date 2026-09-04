from pathlib import Path
import subprocess
import unittest


SCRIPT = Path("tools/playwright/device_test_account_setup_smoke.mjs")


class DeviceTestAccountSetupBrowserTests(unittest.TestCase):
    def test_script_is_syntax_valid_and_secret_stdin_only(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        result = subprocess.run(
            ["node", "--check", str(SCRIPT)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("for await (const chunk of process.stdin)", source)
        self.assertIn("mode: 0o600", source)
        self.assertIn('password = ""', source)
        self.assertNotIn("admin1234", source)
        self.assertNotIn("process.env", source)

    def test_setup_is_single_account_then_read_only_and_logout(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        for required in (
            "First Setup - Hardened BliKVM",
            "Create administrator account",
            "/api/auth/setup",
            "/api/auth/login",
            "/api/inventory",
            "/api/diagnostics",
            "/api/auth/logout",
            "setupClosedAfter: true",
            "controlRequests: 0",
            "secretEmitted: false",
        ):
            self.assertIn(required, source)
        for forbidden in (
            "/api/auth/password",
            "/api/control/",
            "/api/atx",
            "/api/hid",
            "/api/vm",
        ):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
