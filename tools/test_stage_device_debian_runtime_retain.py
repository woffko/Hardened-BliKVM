from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_debian_runtime_retain as retain


class StageDeviceDebianRuntimeRetainTests(unittest.TestCase):
    def package(self) -> package_gate.PackageInfo:
        return package_gate.PackageInfo(
            path=Path("/package.deb"),
            sha256="a" * 64,
            size_bytes=4096,
            source_commit="b" * 40,
            source_bundle_sha256="c" * 64,
            builder_commit="d" * 40,
            installed_files={
                "/usr/libexec/hardened-blikvm/bli-kvm-server": ("1" * 64, 0o755),
                "/usr/libexec/hardened-blikvm/bli-kvm-platform-broker": (
                    "2" * 64,
                    0o755,
                ),
                "/usr/libexec/hardened-blikvm/bli-kvm-video-proxy": (
                    "3" * 64,
                    0o755,
                ),
                "/usr/libexec/hardened-blikvm/bli-kvm-control-broker": (
                    "4" * 64,
                    0o755,
                ),
                "/usr/share/hardened-blikvm/web/index.html": ("5" * 64, 0o644),
                "/usr/share/doc/hardened-blikvm-runtime/PACKAGE-MANIFEST.json": (
                    "6" * 64,
                    0o644,
                ),
                "/lib/systemd/system/hardened-blikvm-platform-broker.service": (
                    "7" * 64,
                    0o644,
                ),
                "/lib/systemd/system/hardened-blikvm-video-proxy.service": (
                    "8" * 64,
                    0o644,
                ),
                "/lib/systemd/system/hardened-blikvm-control-broker.service": (
                    "9" * 64,
                    0o644,
                ),
                "/lib/systemd/system/hardened-blikvm-server.service": (
                    "e" * 64,
                    0o644,
                ),
            },
        )

    def paths(self) -> retain.RemotePaths:
        return retain.remote_paths("a" * 64, "0123456789abcdef")

    def test_paths_separate_ephemeral_stage_and_persistent_rollback(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.stage_root.startswith("/tmp/hardened-blikvm-retain-"))
        self.assertTrue(
            paths.rollback_root.startswith(
                "/var/lib/hardened-blikvm-retain-rollback-"
            )
        )
        self.assertEqual(
            paths.boot_unit_path,
            "/etc/systemd/system/hardened-blikvm-retain-rollback-0123456789abcdef.service",
        )
        self.assertEqual(
            paths.boot_unit_link,
            "/etc/systemd/system/sysinit.target.wants/"
            "hardened-blikvm-retain-rollback-0123456789abcdef.service",
        )
        self.assertRegex(
            paths.watchdog_unit,
            r"^hardened-blikvm-retain-watchdog-[0-9a-f]{16}\.service$",
        )

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        scripts = (
            retain.build_preflight_script(self.paths(), self.package()),
            retain.build_upload_script(self.paths(), self.package()),
            retain.build_rollback_script(self.paths(), self.package()),
            retain.build_prepare_script(self.paths(), self.package()),
            retain.build_commit_script(self.paths(), self.package()),
            retain.build_cleanup_script(self.paths(), self.package()),
            retain.build_final_script(self.paths(), self.package()),
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

    def test_prepare_is_exact_inert_and_crash_guarded(self) -> None:
        preflight = retain.build_preflight_script(self.paths(), self.package())
        prepare = retain.build_prepare_script(self.paths(), self.package())
        boot_unit = retain.build_boot_unit(self.paths())
        self.assertIn("df --output=iavail", preflight)
        self.assertIn("getcap", preflight)
        self.assertIn("dpkg --audit", preflight)
        self.assertIn("other_package_hash", prepare)
        self.assertIn("dpkg -i", prepare)
        self.assertIn("systemd-run --quiet --collect", prepare)
        self.assertIn('ln -s "../$boot_unit" "$boot_unit_link"', prepare)
        self.assertIn("installed_tree_exact", prepare)
        self.assertIn("capabilities_absent", prepare)
        self.assertIn("WantedBy=sysinit.target", boot_unit)
        self.assertIn("Before=kvmd.service kvmd-nginx.service", boot_unit)
        self.assertNotIn("dpkg --purge", prepare)
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
            self.assertNotIn(forbidden, prepare)

    def test_inode_capacity_command_is_executable(self) -> None:
        result = subprocess.run(
            ["df", "--output=iavail", "/tmp"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        lines = result.stdout.decode("ascii").splitlines()
        self.assertGreaterEqual(len(lines), 2)
        self.assertRegex(lines[-1].strip(), r"^[1-9][0-9]*$")

    def test_commit_retains_package_and_removes_stage_before_watchdog(self) -> None:
        commit = retain.build_commit_script(self.paths(), self.package())
        self.assertNotIn("dpkg --purge", commit)
        self.assertIn("emit package_retained yes", commit)
        self.assertLess(commit.index("phase=remove-stage"), commit.index("phase=stop-watchdog"))
        self.assertLess(
            commit.index("phase=stop-watchdog"),
            commit.index("phase=remove-boot-rollback"),
        )
        self.assertIn("units_loaded_disabled_inactive", commit)
        self.assertIn("other_package_hash_before", commit)

    def test_failure_paths_purge_only_the_exact_package(self) -> None:
        rollback = retain.build_rollback_script(self.paths(), self.package())
        cleanup = retain.build_cleanup_script(self.paths(), self.package())
        for script in (rollback, cleanup):
            self.assertIn(
                'dpkg --purge --force-remove-reinstreq "$package_name"', script
            )
            self.assertIn("package_identity_exact", script)
            self.assertIn('"$marker_value"', script)
            self.assertNotIn("rm -rf", script)
            self.assertNotIn("systemctl stop kvmd", script)

    def test_fact_and_failure_parsers_are_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(retain.COMMIT_KEYS)).encode()
        self.assertEqual(set(retain.parse_facts(raw, retain.COMMIT_KEYS)), retain.COMMIT_KEYS)
        with self.assertRaises(ValueError):
            retain.parse_facts(raw + b"unexpected=yes\n", retain.COMMIT_KEYS)
        self.assertEqual(
            retain.parse_failure_phase(b"failure_phase=package-manager-idle\n"),
            "package-manager-idle",
        )
        with self.assertRaises(ValueError):
            retain.parse_failure_phase(b"failure_phase=private/value\n")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_debian_runtime_retain.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
