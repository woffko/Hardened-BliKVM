import subprocess
import sys
import unittest

import tools.recover_device_web_cutover_fallback as recovery


class RecoverDeviceWebCutoverFallbackTests(unittest.TestCase):
    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "a" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "b" * 64,
        }

    def test_script_is_posix_and_bounded_to_inactive_hardened_web_files(self) -> None:
        script = recovery.build_recovery_script(self.baseline())
        result = subprocess.run(
            ["sh", "-n"],
            input=script.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn('rm -f -- "$config_target" "$unit_target"', script)
        self.assertIn("product_inactive_disabled", script)
        self.assertIn("validate_orphan", script)
        self.assertIn("remove_orphan", script)
        self.assertIn(
            "hardened-blikvm-web-cutover-[0-9a-f]{12}-[0-9a-f]{16}",
            script,
        )
        self.assertIn("root_count", script)
        self.assertNotIn("systemctl stop", script)
        self.assertNotIn("systemctl disable", script)
        self.assertNotIn("rm -rf", script)
        self.assertNotIn("account.json\"", script.split("rm -f --")[-1])
        self.assertNotIn("reboot", script)
        self.assertNotIn("poweroff", script)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/recover_device_web_cutover_fallback.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
