from pathlib import Path
import subprocess
import sys
import unittest

import tools.runtime_provisioning_policy as identity_policy
import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_runtime_activation as activation
import tools.stage_device_runtime_provision as provision


class StageDeviceRuntimeActivationTests(unittest.TestCase):
    def assets(self) -> provision.Assets:
        payload = identity_policy.SYSUSERS_SOURCE.read_bytes()
        package = package_gate.PackageInfo(
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
                identity_policy.INSTALLED_SYSUSERS_PATH: (
                    identity_policy.EXPECTED_SYSUSERS_SHA256,
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
        return provision.Assets(
            package,
            identity_policy.SysusersInfo(
                identity_policy.SYSUSERS_SOURCE,
                identity_policy.EXPECTED_SYSUSERS_SHA256,
                len(payload),
                payload,
            ),
        )

    def paths(self) -> activation.RemotePaths:
        return activation.remote_paths("a" * 64, "0" * 16)

    def test_paths_are_marker_bound_and_private(self) -> None:
        paths = self.paths()
        self.assertTrue(
            paths.root.startswith(
                "/var/lib/hardened-blikvm-activation-rollback-aaaaaaaaaaaa-"
            )
        )
        self.assertTrue(paths.password.startswith(paths.root + "/"))
        self.assertEqual(paths.boot_unit_link.rsplit("/", 1)[-1], paths.boot_unit)

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        scripts = (
            activation.build_preflight_script(self.paths(), self.assets()),
            activation.build_prepare_script(self.paths(), self.assets()),
            activation.build_rollback_script(self.paths(), self.assets()),
            activation.build_apply_script(self.paths(), self.assets()),
            activation.build_commit_script(self.paths(), self.assets()),
            activation.build_cleanup_script(self.paths(), self.assets()),
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

    def test_preflight_is_read_only_and_requires_provisioned_inactive_state(self) -> None:
        script = activation.build_preflight_script(self.paths(), self.assets())
        for required in (
            "units_inactive",
            "control_boundary_absent",
            "private_artifacts_absent",
            "socket_tail_clean",
            "os.getrandom(1, os.GRND_NONBLOCK)",
            "activation_gates_absent",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "\nsystemctl start",
            "\nsystemctl enable",
            "\nmkdir ",
            "\nmv ",
            "\nrm ",
        ):
            self.assertNotIn(forbidden, script)

    def test_prepare_arms_boot_and_timer_before_service_start(self) -> None:
        script = activation.build_prepare_script(self.paths(), self.assets())
        self.assertIn("os.getrandom(32, os.GRND_NONBLOCK).hex()", script)
        self.assertIn("WantedBy=sysinit.target", activation.build_boot_unit(self.paths()))
        self.assertIn("systemd-run --quiet --collect", script)
        self.assertNotIn("systemctl start hardened-blikvm", script)
        self.assertNotIn("native-gate.conf", script)

    def test_apply_starts_only_three_units_and_exercises_read_only_runtime(self) -> None:
        script = activation.build_apply_script(self.paths(), self.assets())
        self.assertIn("BLI_KVM_SECURE_COOKIES=0", activation.build_dropin())
        self.assertIn('systemctl start "$broker_unit"', script)
        self.assertIn('systemctl start "$video_unit"', script)
        self.assertIn('systemctl start "$server_unit"', script)
        self.assertNotIn("systemctl start hardened-blikvm-control", script)
        for required in (
            "process_no_udp_or_abstract",
            "server_listener_exact",
            "verify_zero_caps",
            "control_boundary_absent",
            "CREDENTIALS_DIRECTORY",
            "setpriv --reuid",
            "/api/auth/setup",
            "/api/auth/login",
            "/api/inventory",
            "/api/diagnostics",
            "/api/stream/mjpeg",
            "/api/auth/logout",
            "persistence_not_provisioned",
            "session_revoked_stream",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "systemctl enable",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg -i",
            "dpkg --purge",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, script)

    def test_rollback_and_commit_remove_state_before_disarming(self) -> None:
        rollback = activation.build_rollback_script(self.paths(), self.assets())
        commit = activation.build_commit_script(self.paths(), self.assets())
        for script in (rollback, commit):
            self.assertIn("cgroup_empty", script)
            self.assertIn("socket_tail_clean", script)
            self.assertIn("systemctl daemon-reload", script)
            self.assertIn('rmdir "$state_root"', script)
            self.assertIn('rmdir "$dropin_dir"', script)
            self.assertNotIn("rm -rf", script)
            self.assertNotIn("systemctl stop kvmd", script)
            self.assertNotIn("/etc/hardened-blikvm/kvmd-password", script)
        cleanup = commit.index('rmdir "$state_root"')
        disarm = commit.index('systemctl stop "$watchdog_unit"')
        self.assertLess(cleanup, disarm)

    def test_fact_and_failure_parsers_are_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(activation.COMMIT_KEYS)).encode()
        self.assertEqual(
            set(activation.parse_facts(raw, activation.COMMIT_KEYS)),
            activation.COMMIT_KEYS,
        )
        with self.assertRaises(ValueError):
            activation.parse_facts(raw + b"extra=yes\n", activation.COMMIT_KEYS)
        self.assertEqual(
            activation.parse_failure_phase(
                b"apply_failure_phase=diagnostics\napply_cleanup=restored\n",
                "apply",
            ),
            ("diagnostics", True),
        )

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_runtime_activation.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
