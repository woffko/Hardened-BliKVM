from pathlib import Path
import subprocess
import sys
import unittest

import tools.runtime_provisioning_policy as policy
import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_runtime_provision as provision


class StageDeviceRuntimeProvisionTests(unittest.TestCase):
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
                policy.INSTALLED_SYSUSERS_PATH: (
                    policy.EXPECTED_SYSUSERS_SHA256,
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

    def assets(self) -> provision.Assets:
        payload = policy.SYSUSERS_SOURCE.read_bytes()
        return provision.Assets(
            self.package(),
            policy.SysusersInfo(
                policy.SYSUSERS_SOURCE,
                policy.EXPECTED_SYSUSERS_SHA256,
                len(payload),
                payload,
            ),
        )

    def paths(self) -> provision.RemotePaths:
        return provision.remote_paths(policy.EXPECTED_SYSUSERS_SHA256, "0" * 16)

    def test_paths_are_fixed_marker_bound_and_boot_persistent(self) -> None:
        paths = self.paths()
        self.assertTrue(
            paths.root.startswith(
                "/var/lib/hardened-blikvm-provision-rollback-8685ad67a137-"
            )
        )
        self.assertEqual(paths.config_stage, "/etc/.hardened-blikvm-0000000000000000")
        self.assertEqual(paths.boot_unit_link.rsplit("/", 1)[-1], paths.boot_unit)
        self.assertRegex(
            paths.watchdog_unit,
            r"^hardened-blikvm-provision-watchdog-[0-9a-f]{16}\.service$",
        )

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        scripts = (
            provision.build_preflight_script(self.paths(), self.assets()),
            provision.build_prepare_script(self.paths(), self.assets()),
            provision.build_rollback_script(self.paths(), self.assets()),
            provision.build_apply_script(self.paths(), self.assets()),
            provision.build_commit_script(self.paths(), self.assets()),
            provision.build_cleanup_script(self.paths(), self.assets()),
            provision.build_final_script(self.paths(), self.assets()),
            provision.build_recovery_discovery_script(self.assets()),
            provision.build_rollback_replacement_script(
                self.paths(), self.assets(), "f" * 64, 1024
            ),
            provision.build_execute_recovery_script(
                self.paths(), self.assets(), "f" * 64
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

    def test_preflight_is_read_only_fixed_and_entropy_gated(self) -> None:
        script = provision.build_preflight_script(self.paths(), self.assets())
        for required in (
            "package_installed_exact",
            policy.INSTALLED_SYSUSERS_PATH,
            policy.EXPECTED_SYSUSERS_SHA256,
            "os.getrandom(1, os.GRND_NONBLOCK)",
            "/etc/ptmp",
            "/etc/gtmp",
            "runtime_accounts_absent",
            "units_loaded_disabled_inactive",
            "gates_absent_before",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "\nsystemd-sysusers ",
            "\nkvmd-htpasswd set",
            "\nmkdir ",
            "\nmv ",
            "\nrm ",
            "\nsystemctl start",
            "\nsystemctl enable",
        ):
            self.assertNotIn(forbidden, script)

    def test_prepare_predicts_twice_before_any_live_mutation(self) -> None:
        script = provision.build_prepare_script(self.paths(), self.assets())
        self.assertEqual(script.count('systemd-sysusers --root="$synthetic_root"'), 2)
        self.assertNotIn('systemd-sysusers "$pinned_sysusers"', script)
        self.assertIn("printf 'before_%s=%s", script)
        self.assertIn("printf 'after_%s=%s", script)
        self.assertIn("record_aux pwd_lock .pwd.lock", script)
        self.assertIn("after_%s_hash", script)
        self.assertIn("os.getrandom(32, os.GRND_NONBLOCK).hex()", script)
        self.assertIn("WantedBy=sysinit.target", provision.build_boot_unit(self.paths()))
        self.assertIn("systemd-run --quiet --collect", script)
        self.assertLess(script.index("phase=state"), script.index("phase=watchdog"))

    def test_apply_uses_stdin_credential_and_never_activates_services(self) -> None:
        script = provision.build_apply_script(self.paths(), self.assets())
        self.assertIn("username_bytes=23", script)
        self.assertIn('systemd-sysusers "$pinned_sysusers"', script)
        self.assertIn(
            'kvmd-htpasswd set -i -q "$kvmd_account" < "$password_file"', script
        )
        self.assertIn("curl --silent --max-time 8 --config - --unix-socket", script)
        self.assertIn('mv "$config_stage" "$config_root"', script)
        self.assertIn("setpriv --reuid", script)
        self.assertIn("write_phase sysusers", script)
        self.assertIn("write_phase htpasswd", script)
        self.assertIn("write_phase config", script)
        for forbidden in (
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg -i",
            "dpkg --purge",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, script)

    def test_rollback_is_per_file_cas_and_concurrency_safe(self) -> None:
        script = provision.build_rollback_script(self.paths(), self.assets())
        self.assertIn("username_bytes=23", script)
        self.assertIn('[ "$current" = "$before" ]', script)
        self.assertIn('[ "$current" = "$after" ]', script)
        self.assertIn("exit 3", script)
        self.assertIn("precheck_aux passwd_backup passwd-", script)
        self.assertIn("precheck_aux pwd_lock .pwd.lock", script)
        self.assertIn("restore_aux pwd_lock .pwd.lock", script)
        self.assertIn("'$1 != account {print $0}'", script)
        self.assertIn('cmp -s "$account_backup/htpasswd" "$filtered_htpasswd"', script)
        self.assertNotIn("kvmd-htpasswd del", script)
        self.assertNotIn("rm -rf", script)
        self.assertNotIn("systemctl stop kvmd", script)
        self.assertNotIn("dpkg --purge", script)

    def test_commit_disarms_only_after_revalidation(self) -> None:
        script = provision.build_commit_script(self.paths(), self.assets())
        validation = script.index("verify_identities")
        stop = script.index('systemctl stop "$watchdog_unit"')
        remove = script.rindex("\nremove_rollback_tree\n")
        self.assertLess(validation, stop)
        self.assertLess(stop, remove)
        self.assertIn("emit package_provisioned yes", script)

    def test_final_inspector_is_read_only_and_exact(self) -> None:
        script = provision.build_final_script(self.paths(), self.assets())
        for required in (
            "verify_identities_final",
            "verify_config_final",
            "authenticate_kvmd_final",
            "package_installed_final",
            "gates_absent_final",
            "unit_templates_inactive_final",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "dpkg -i",
            "dpkg --purge",
            "systemd-sysusers",
            "kvmd-htpasswd set",
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "mkdir",
            "chmod",
            "mv ",
            "rm ",
            "ln -s",
        ):
            self.assertNotIn(forbidden, script)

    def test_fact_and_failure_parsers_are_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(provision.APPLY_KEYS)).encode()
        self.assertEqual(
            set(provision.parse_facts(raw, provision.APPLY_KEYS)),
            provision.APPLY_KEYS,
        )
        with self.assertRaises(ValueError):
            provision.parse_facts(raw + b"extra=yes\n", provision.APPLY_KEYS)
        self.assertEqual(
            provision.parse_failure_phase(
                b"apply_failure_phase=config\napply_cleanup=restored\n", "apply"
            ),
            ("config", True),
        )
        with self.assertRaises(ValueError):
            provision.parse_failure_phase(b"apply_failure_phase=private/value\n", "apply")

    def test_recovery_is_marker_bound_and_accepts_only_bounded_state(self) -> None:
        discovery = provision.build_recovery_discovery_script(self.assets())
        replacement = provision.build_rollback_replacement_script(
            self.paths(), self.assets(), "f" * 64, 1024
        )
        execute = provision.build_execute_recovery_script(
            self.paths(), self.assets(), "f" * 64
        )
        self.assertIn('[ "$count" = 1 ]', discovery)
        self.assertIn('[ "${#run_id}" = 16 ]', discovery)
        self.assertIn("database_state", discovery)
        self.assertIn("htpasswd_state", discovery)
        self.assertIn('"$marker_value"', replacement)
        self.assertIn('cat > "$rollback_script.part"', replacement)
        self.assertIn('sha256sum "$rollback_script.part"', replacement)
        self.assertIn('/bin/sh "$rollback_script" explicit', execute)
        self.assertNotIn("kvmd-password", replacement)
        facts = {
            "account_state": "present",
            "boot_state": "armed",
            "config_state": "present",
            "database_state": "after",
            "htpasswd_state": "after",
            "kvmd_state": "active",
            "package_state": "installed",
            "phase": "htpasswd",
            "rollback_state": "ready",
            "rootfs_mode": "rw",
            "run_id": "0" * 16,
            "unit_state": "inactive",
            "watchdog_state": "active",
        }
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        self.assertEqual(provision.parse_recovery_facts(raw), dict(sorted(facts.items())))
        with self.assertRaises(ValueError):
            provision.parse_recovery_facts(raw + b"private=value\n")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_runtime_provision.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
