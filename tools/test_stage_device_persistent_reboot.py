import subprocess
import sys
import unittest
from pathlib import Path

import tools.stage_device_persistent_reboot as reboot


class StageDevicePersistentRebootTests(unittest.TestCase):
    def paths(self) -> reboot.Paths:
        return reboot.paths("0123456789abcdef")

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "a" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "b" * 64,
        }

    def test_paths_are_marker_owned_and_fixed(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.root.startswith("/var/lib/hardened-blikvm-reboot-guard-"))
        self.assertTrue(paths.guard_script.startswith(paths.root + "/"))
        self.assertTrue(paths.timer_link.endswith(paths.timer))

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = self.paths()
        baseline = self.baseline()
        scripts = (
            reboot.build_preflight_script(paths, baseline),
            reboot.build_guard_script(paths, "c" * 64, baseline),
            reboot.build_prepare_script(paths, "c" * 64, baseline),
            reboot.build_postboot_script(paths, "d" * 64, "c" * 64, baseline),
            reboot.build_disarm_script(paths, "c" * 64, baseline),
        )
        for script in scripts:
            result = subprocess.run(
                ["sh", "-n"],
                input=script.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_guard_is_delayed_persistent_and_restores_only_web_fallback(self) -> None:
        paths = self.paths()
        timer = reboot.build_guard_timer(paths)
        guard = reboot.build_guard_script(paths, "c" * 64, self.baseline())
        self.assertIn("OnActiveSec=240s", timer)
        self.assertIn("WantedBy=timers.target", timer)
        self.assertIn("persistent_state", guard)
        self.assertIn("account_state_valid", guard)
        self.assertIn("cutover_orphans_absent", guard)
        self.assertIn("runtime_package_ready", guard)
        self.assertIn("0.1.0~dev2", guard)
        self.assertIn(
            "! grep -Fq 'ConditionPathExists=/run/kvmd/'",
            reboot.REMOTE_HELPERS,
        )
        self.assertIn("systemctl disable --now hardened-blikvm-nginx.service", guard)
        self.assertIn("systemctl enable kvmd-nginx.service", guard)
        self.assertNotIn("systemctl stop kvmd.service", guard)
        self.assertNotIn("systemctl reboot", guard)

    def test_host_flow_uses_software_reboot_and_no_target_power_control(self) -> None:
        source = Path("tools/stage_device_persistent_reboot.py").read_text(
            encoding="utf-8"
        )
        self.assertIn('target, secret, "systemctl reboot"', source)
        self.assertIn("sshAndHttpsDisappeared", source)
        self.assertIn("boot_id_changed", source)
        self.assertIn('"targetPowerControl": False', source)
        self.assertIn('"atxPowerReset": False', source)
        self.assertIn("subprocess.SubprocessError", source)
        for forbidden in (
            "/api/control",
            "/api/atx",
            "/api/hid",
            "poweroff",
            "shutdown",
            "reboot -f",
            "rm -rf",
        ):
            self.assertNotIn(forbidden, source)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_persistent_reboot.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    unittest.main()
