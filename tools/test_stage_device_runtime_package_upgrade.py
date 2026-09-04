from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_runtime_package_upgrade as upgrade


class StageDeviceRuntimePackageUpgradeTests(unittest.TestCase):
    def package(self, version: str, digest: str) -> package_gate.PackageInfo:
        del version
        return package_gate.PackageInfo(
            path=Path("/package.deb"),
            sha256=digest,
            size_bytes=1024,
            source_commit="b" * 40,
            source_bundle_sha256="c" * 64,
            builder_commit="d" * 40,
            installed_files={
                "/lib/systemd/system/hardened-blikvm-platform-broker.service": (
                    "e" * 64,
                    0o644,
                ),
                "/lib/systemd/system/hardened-blikvm-video-proxy.service": (
                    "f" * 64,
                    0o644,
                ),
            },
        )

    def paths(self) -> upgrade.Paths:
        return upgrade.paths("a" * 64, "0123456789abcdef")

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "1" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "2" * 64,
        }

    def test_paths_are_persistent_marker_bound_and_bounded(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.root.startswith("/var/lib/hardened-blikvm-runtime-upgrade-"))
        self.assertTrue(paths.marker.startswith(paths.root + "/"))
        self.assertTrue(paths.new_package.startswith(paths.root + "/"))
        self.assertTrue(paths.old_package.startswith(paths.root + "/"))
        self.assertRegex(paths.boot_service, r"^[a-z0-9.-]+\.service$")
        self.assertRegex(paths.timer, r"^[a-z0-9.-]+\.timer$")

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        new = self.package("dev2", "a" * 64)
        old = self.package("dev1", "b" * 64)
        scripts = (
            upgrade.build_preflight_script(self.paths(), new, old, self.baseline()),
            upgrade.build_upload_script(self.paths(), new, old, old=False),
            upgrade.build_upload_script(self.paths(), new, old, old=True),
            upgrade.build_rollback_script(
                self.paths(), new, old, self.baseline(), "3" * 64
            ),
            upgrade.build_prepare_script(
                self.paths(), new, old, self.baseline(), "3" * 64
            ),
            upgrade.build_abandon_script(self.paths(), new, old),
            upgrade.build_apply_script(
                self.paths(), new, old, self.baseline(), "3" * 64
            ),
            upgrade.build_commit_script(
                self.paths(),
                new,
                old,
                self.baseline(),
                "3" * 64,
                remove_guard=True,
            ),
            upgrade.build_cleanup_script(
                self.paths(), new, old, self.baseline(), "3" * 64
            ),
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

    def test_upgrade_is_real_package_rollback_gated_and_power_safe(self) -> None:
        new = self.package("dev2", "a" * 64)
        old = self.package("dev1", "b" * 64)
        rollback = upgrade.build_rollback_script(
            self.paths(), new, old, self.baseline(), "3" * 64
        )
        prepare = upgrade.build_prepare_script(
            self.paths(), new, old, self.baseline(), "3" * 64
        )
        apply = upgrade.build_apply_script(
            self.paths(), new, old, self.baseline(), "3" * 64
        )
        self.assertIn('dpkg -i "$old_package"', rollback)
        self.assertIn('dpkg -i "$new_package"', apply)
        self.assertIn("OnActiveSec=600s", upgrade.build_timer(self.paths()))
        self.assertIn("WantedBy=sysinit.target", upgrade.build_boot_service(self.paths()))
        self.assertIn("systemctl enable kvmd-nginx.service", rollback)
        self.assertIn("systemctl disable --now kvmd-nginx.service", apply)
        self.assertIn("account_sha256", prepare + apply + rollback)
        self.assertIn("rollback_ready=yes", prepare)
        self.assertIn("unexpected=$(find", upgrade.build_abandon_script(self.paths(), new, old))
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
            self.assertNotIn(forbidden, prepare + apply + rollback)

    def test_package_units_replace_only_the_transient_socket_conditions(self) -> None:
        new = self.package("dev2", "a" * 64)
        old = self.package("dev1", "b" * 64)
        common = "Requires=kvmd.service\nAfter=kvmd.service\n"
        with mock.patch.object(
            upgrade,
            "_package_member",
            side_effect=[
                common.encode(),
                (common + "ConditionPathExists=/run/kvmd/kvmd.sock\n").encode(),
                common.encode(),
                (common + "ConditionPathExists=/run/kvmd/ustreamer.sock\n").encode(),
            ],
        ):
            upgrade.validate_unit_semantics(new, old)

    def test_rollback_requires_new_only_paths_to_be_absent(self) -> None:
        new = self.package("dev2", "a" * 64)
        old = self.package("dev1", "b" * 64)
        new.installed_files["/lib/systemd/system/new-only.service"] = (
            "1" * 64,
            0o644,
        )
        checks = upgrade.removed_on_rollback_checks(new, old)
        self.assertIn("[ ! -e /lib/systemd/system/new-only.service ]", checks)

    def test_fact_parser_requires_the_exact_schema(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(upgrade.APPLY_KEYS)).encode()
        self.assertEqual(set(upgrade.parse_facts(raw, upgrade.APPLY_KEYS)), upgrade.APPLY_KEYS)
        with self.assertRaises(ValueError):
            upgrade.parse_facts(raw + b"private=value\n", upgrade.APPLY_KEYS)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_runtime_package_upgrade.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn(b"--preflight-only", result.stdout)

    def test_preflight_only_returns_before_any_upload(self) -> None:
        source = Path("tools/stage_device_runtime_package_upgrade.py").read_text()
        branch = source.index("if options.preflight_only:")
        upload = source.index('stage = "upload-new"')
        self.assertLess(branch, upload)
        between = source[branch:upload]
        self.assertIn('"deviceChanged": False', between)
        self.assertNotIn("build_upload_script", between)

    def test_design_records_the_separate_acceptance_boundaries(self) -> None:
        design = Path("docs/design/runtime-package-upgrade-gate-v1.md").read_text()
        for required in (
            "exact prior package used for rollback",
            "ten-minute rollback timer",
            "Browser login,",
            "video, and Diagnostics are validated separately",
            "No device reboot, target Power/Reset action",
        ):
            self.assertIn(required, design)


if __name__ == "__main__":
    unittest.main()
