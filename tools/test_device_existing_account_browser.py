from pathlib import Path
import subprocess
import unittest


SCRIPT = Path("tools/playwright/device_existing_account_smoke.mjs")


class DeviceExistingAccountBrowserTests(unittest.TestCase):
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
        self.assertNotIn("password,", source)

    def test_browser_contract_is_authenticated_read_only_and_logout_only(self) -> None:
        source = SCRIPT.read_text(encoding="utf-8")
        for required in (
            "Login - Hardened BliKVM",
            "Remote Desktop - Hardened BliKVM",
            "/api/auth/setup",
            "/api/auth/login",
            "/api/inventory",
            "/api/diagnostics",
            "/api/stream/mjpeg",
            "/api/auth/logout",
            "videoDecoded: true",
            "videoReconnects: streamResponses",
            "streamRevoked: true",
            "initial MJPEG stream survived reload",
            "MJPEG stream survived logout",
            "secureSessionCookie",
            "sessionCleared",
            "controlRequests: 0",
            "secretEmitted: false",
        ):
            self.assertIn(required, source)
        for forbidden in (
            "/api/auth/password",
            "Create account",
            "/api/control/",
            "/api/hid",
            "/api/atx",
            "/api/vm",
        ):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
