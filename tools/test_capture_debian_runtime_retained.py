from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_debian_runtime_retain as retain


class CaptureDebianRuntimeRetainedTests(unittest.TestCase):
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

    def test_remote_probe_is_read_only_and_exact(self) -> None:
        paths = retain.remote_paths("a" * 64, "0123456789abcdef")
        script = retain.build_final_script(paths, self.package())
        for required in (
            "package_installed",
            "installed_tree_exact_final",
            "capabilities_absent_final",
            "unit_templates_loaded_disabled_inactive_final",
            "gates_absent_final",
            "dpkg_audit_clean_final",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "dpkg -i",
            "dpkg --purge",
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "mkdir",
            "chmod",
            "mv ",
            "rm ",
            "ln -s",
            "systemd-sysusers",
            "systemd-tmpfiles",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, script)

    def test_generated_probe_is_valid_posix_shell(self) -> None:
        paths = retain.remote_paths("a" * 64, "0123456789abcdef")
        result = subprocess.run(
            ["sh", "-n"],
            input=retain.build_final_script(paths, self.package()).encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_debian_runtime_retained.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
