from pathlib import Path, PurePosixPath
import subprocess
import sys
import tarfile
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
        self.assertIn("if getent passwd", stage.STATE_FUNCTIONS)
        self.assertIn("if getent group hardened-blikvm", stage.STATE_FUNCTIONS)
        self.assertIn("return 0", stage.STATE_FUNCTIONS)
        self.assertIn("units_inactive", run)
        self.assertIn("nginx_hash_before", run)
        self.assertIn("account_hash_before", run)
        self.assertIn("emit dpkg_audit_clean yes", preflight)
        for phase in (
            "phase=tools",
            "phase=package-manager-idle",
            "phase=watchdog",
            "phase=ports",
            "phase=htpasswd",
        ):
            self.assertIn(phase, preflight)
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
        self.assertEqual(
            stage.parse_preflight_failure(b"failure_phase=package-manager-idle\n"),
            "package-manager-idle",
        )
        with self.assertRaises(ValueError):
            stage.parse_preflight_failure(b"failure_phase=private/value\n")

    def test_package_name_matches_builder(self) -> None:
        self.assertEqual(debian.PACKAGE_NAME, "hardened-blikvm-runtime")

    def test_privileged_modes_and_extended_attributes_are_rejected(self) -> None:
        privileged = tarfile.TarInfo("usr/libexec/hardened-blikvm/binary")
        privileged.mode = 0o4755
        with self.assertRaisesRegex(ValueError, "privileged mode"):
            stage.validate_tar_metadata(privileged)

        attributed = tarfile.TarInfo("usr/libexec/hardened-blikvm/binary")
        attributed.mode = 0o755
        attributed.pax_headers = {"SCHILY.xattr.security.capability": "value"}
        with self.assertRaisesRegex(ValueError, "extended attribute"):
            stage.validate_tar_metadata(attributed)

    def test_data_members_are_restricted_to_inert_payload_roots(self) -> None:
        stage.validate_data_member_path(
            PurePosixPath("usr/libexec/hardened-blikvm/bli-kvm-server"), False
        )
        stage.validate_data_member_path(
            PurePosixPath("usr/share/hardened-blikvm/web/assets/app.js"), False
        )
        stage.validate_data_member_path(
            PurePosixPath(
                "lib/systemd/system/hardened-blikvm-platform-broker.service"
            ),
            False,
        )
        with self.assertRaisesRegex(ValueError, "payload allowlist"):
            stage.validate_data_member_path(PurePosixPath("usr/bin/unexpected"), False)
        with self.assertRaisesRegex(ValueError, "payload allowlist"):
            stage.validate_data_member_path(
                PurePosixPath("usr/libexec/hardened-blikvm/unexpected"), False
            )
        with self.assertRaisesRegex(ValueError, "payload allowlist"):
            stage.validate_data_member_path(PurePosixPath("root/payload"), False)

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
