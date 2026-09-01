from pathlib import Path
import subprocess
import sys
import unittest

import tools.build_debian_runtime_package as debian
import tools.stage_device_debian_runtime_package as stage


class StageDeviceDebianRuntimePackageTests(unittest.TestCase):
    def package(self) -> stage.PackageInfo:
        return stage.PackageInfo(
            path=Path("/package.deb"),
            sha256="a" * 64,
            size_bytes=1024,
            source_commit="b" * 40,
            source_bundle_sha256="f" * 64,
            builder_commit="c" * 40,
            installed_files={
                "/usr/libexec/hardened-blikvm/bli-kvm-server": ("d" * 64, 0o755),
                "/lib/systemd/system/hardened-blikvm-server.service": (
                    "e" * 64,
                    0o644,
                ),
            },
        )

    def paths(self) -> stage.RemotePaths:
        return stage.remote_paths("a" * 64, "0123456789abcdef")

    def test_stage_paths_are_unique_bounded_and_marker_protected(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.root.startswith("/tmp/hardened-blikvm-deb-"))
        self.assertTrue(paths.marker.startswith(paths.root + "/"))
        self.assertTrue(paths.package.startswith(paths.root + "/"))
        self.assertRegex(
            paths.watchdog_unit,
            r"^hardened-blikvm-deb-rollback-[0-9a-f]{16}\.service$",
        )

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        scripts = (
            stage.build_preflight_script(self.paths(), self.package()),
            stage.build_upload_script(self.paths(), self.package()),
            stage.build_rollback_script(self.paths()),
            stage.build_run_script(self.paths(), self.package()),
            stage.build_cleanup_script(self.paths()),
            stage.build_final_script(self.paths()),
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

    def test_run_is_inert_and_rollback_gated(self) -> None:
        preflight = stage.build_preflight_script(self.paths(), self.package())
        run = stage.build_run_script(self.paths(), self.package())
        rollback = stage.build_rollback_script(self.paths())
        self.assertIn("dpkg -i", run)
        self.assertIn("dpkg --purge", run)
        self.assertIn("systemd-run --quiet --collect", run)
        self.assertIn("sleep 300", rollback)
        self.assertIn("accounts_absent", run)
        self.assertIn("units_inactive", run)
        self.assertIn("nginx_hash_before", run)
        self.assertIn("account_hash_before", run)
        self.assertIn("emit dpkg_audit_clean yes", preflight)
        for forbidden in (
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "systemd-sysusers",
            "systemd-tmpfiles",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, run)

    def test_fact_parser_is_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(stage.RUN_KEYS)).encode()
        parsed = stage.parse_facts(raw, stage.RUN_KEYS)
        self.assertEqual(set(parsed), stage.RUN_KEYS)
        with self.assertRaises(ValueError):
            stage.parse_facts(raw + b"unexpected=yes\n", stage.RUN_KEYS)

    def test_package_name_matches_builder(self) -> None:
        self.assertEqual(debian.PACKAGE_NAME, "hardened-blikvm-runtime")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_debian_runtime_package.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
