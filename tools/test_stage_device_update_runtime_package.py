from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_update_runtime_package as stage


class StageDeviceUpdateRuntimePackageTests(unittest.TestCase):
    def package(self) -> stage.PackageInfo:
        return stage.PackageInfo(
            path=Path("/update.deb"),
            sha256="a" * 64,
            size_bytes=1024,
            source_commit="b" * 40,
            source_bundle_sha256="c" * 64,
            builder_commit="d" * 40,
            installed_files={
                "/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor": (
                    "e" * 64,
                    0o755,
                ),
                "/lib/systemd/system/hardened-blikvm-update-watchdog.timer": (
                    "f" * 64,
                    0o644,
                ),
            },
        )

    def paths(self) -> stage.Paths:
        return stage.paths("a" * 64, "0123456789abcdef")

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "1" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "2" * 64,
        }

    def preflight(self) -> dict[str, str]:
        values = {key: "yes" for key in stage.PRE_KEYS}
        values.update(
            {
                "account_sha256": "3" * 64,
                "config_sha256": "4" * 64,
                "tls_sha256": "1" * 64,
                "key_sha256": "2" * 64,
                "architecture": "aarch64",
                "rootfs_mode": "rw",
                "broker_pid": "101",
                "video_pid": "102",
                "server_pid": "103",
                "hardened_nginx_pid": "104",
                "broker_restarts": "0",
                "video_restarts": "0",
                "server_restarts": "0",
                "hardened_nginx_restarts": "0",
            }
        )
        return values

    def test_paths_are_persistent_marker_bound_and_bounded(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.root.startswith("/var/lib/hardened-blikvm-update-package-"))
        self.assertTrue(paths.package.startswith(paths.root + "/"))
        self.assertRegex(paths.boot_service, r"^[a-z0-9.-]+\.service$")
        self.assertRegex(paths.timer, r"^[a-z0-9.-]+\.timer$")

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        package = self.package()
        paths = self.paths()
        baseline = self.baseline()
        preflight = self.preflight()
        scripts = (
            stage.build_preflight_script(paths, package, baseline),
            stage.build_upload_script(paths, package),
            stage.build_rollback_script(paths, package, baseline, preflight),
            stage.build_prepare_script(paths, package, baseline, preflight),
            stage.build_apply_script(paths, package, baseline, preflight),
            stage.build_final_script(
                paths, package, baseline, preflight, remove_guard=True
            ),
            stage.build_final_script(
                paths, package, baseline, preflight, remove_guard=False
            ),
            stage.build_cleanup_script(paths, package, baseline, preflight),
            stage.build_abandon_script(paths, package),
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

    def test_install_is_inert_guarded_and_exactly_reversible(self) -> None:
        package = self.package()
        paths = self.paths()
        baseline = self.baseline()
        preflight = self.preflight()
        apply = stage.build_apply_script(paths, package, baseline, preflight)
        rollback = stage.build_rollback_script(paths, package, baseline, preflight)
        prepare = stage.build_prepare_script(paths, package, baseline, preflight)
        self.assertIn('dpkg -i "$package_file"', apply)
        self.assertIn('dpkg --purge "$package_name"', rollback)
        self.assertIn("OnActiveSec=600s", stage.build_timer(paths))
        self.assertIn("WantedBy=sysinit.target", stage.build_boot_service(paths))
        self.assertIn("update_authority_absent", apply + rollback)
        self.assertIn("runtime_unchanged_checks", Path(stage.__file__).read_text())
        self.assertNotIn("hardened-blikvm-update-*", stage.REMOTE_HELPERS)
        for forbidden in (
            "systemctl reboot",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "poweroff",
            "shutdown",
            "/api/control",
            "/api/atx",
            "rm -rf",
        ):
            self.assertNotIn(forbidden, apply + rollback + prepare)

    def test_preflight_only_returns_before_upload(self) -> None:
        source = Path(stage.__file__).read_text(encoding="utf-8")
        branch = source.index("if options.preflight_only:")
        upload = source.index('stage = "upload"')
        self.assertLess(branch, upload)
        self.assertNotIn("build_upload_script", source[branch:upload])
        self.assertIn('"deviceChanged": False', source[branch:upload])

    def test_fact_parser_is_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(stage.APPLY_KEYS)).encode()
        self.assertEqual(set(stage.parse_facts(raw, stage.APPLY_KEYS)), stage.APPLY_KEYS)
        with self.assertRaises(ValueError):
            stage.parse_facts(raw + b"private=value\n", stage.APPLY_KEYS)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_update_runtime_package.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn(b"--preflight-only", result.stdout)


if __name__ == "__main__":
    unittest.main()
