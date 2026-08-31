from pathlib import Path
import subprocess
import tempfile
import unittest

from tools import stage_device_diagnostic_collector as gate


class StageDeviceDiagnosticCollectorTests(unittest.TestCase):
    def paths(self) -> gate.Paths:
        return gate.stage_paths("a" * 64, "0123456789abcdef")

    def assets(self) -> gate.Assets:
        return gate.Assets(
            binary=gate.Asset(Path("/tmp/binary"), "a" * 64, 1_500_000),
            unit=gate.Asset(Path("/tmp/unit"), "b" * 64, 2_000),
            base_sysusers=gate.Asset(Path("/tmp/base-sysusers"), "c" * 64, 1_000),
            sysusers=gate.Asset(Path("/tmp/sysusers"), "e" * 64, 200),
            source_commit="d" * 40,
        )

    def test_paths_are_exact_marker_protected_and_unshipped(self) -> None:
        paths = self.paths()
        self.assertRegex(paths.root, gate.STAGE_ROOT)
        self.assertEqual(paths.marker, f"{paths.root}/.hardened-blikvm-owner")
        self.assertEqual(
            paths.binary_target,
            "/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-collector",
        )
        self.assertEqual(
            paths.unit_target,
            "/run/systemd/system/hardened-blikvm-diagnostic-collector.service",
        )
        self.assertEqual(
            paths.export_socket,
            "/run/hardened-blikvm-diagnostics/export.sock",
        )
        self.assertEqual(
            paths.watchdog,
            "hardened-blikvm-diagnostic-rollback-0123456789abcdef",
        )
        self.assertEqual(paths.account_backup, f"{paths.root}/account-backup")
        with self.assertRaises(ValueError):
            gate.stage_paths("a" * 63, "0123456789abcdef")

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        paths = self.paths()
        assets = self.assets()
        scripts = (
            gate.build_preflight_script(paths, 20_000_000),
            gate.build_first_upload_script(paths),
            gate.build_upload_script(paths, paths.unit, "600"),
            gate.build_rollback_script(paths, assets),
            gate.build_run_script(paths, assets),
            gate.build_cleanup_script(paths, assets),
            gate.build_final_script(paths, "present"),
            gate.build_final_script(paths, "absent"),
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

    def test_generated_client_has_only_fixed_protocol_modes(self) -> None:
        client = gate.build_client(self.paths())
        compile(client, "generated-diagnostic-client.py", "exec")
        self.assertIn("READY = bytes((1, 1, 0, 0, 0, 0, 0, 0))", client)
        self.assertIn(
            "CONTROL_DISABLED = bytes((1, 3, 2, 0, 0, 0, 0, 0))", client
        )
        self.assertIn(
            "EXPORT_READ = bytes((1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))",
            client,
        )
        self.assertIn("ACCEPTED = bytes((1, 1, 0, 0))", client)
        for mode in (
            "accepted",
            "control-disabled",
            "accepted-fast",
            "fill",
            "rejected",
            "malformed",
            "partial",
            "hold",
            "export-read",
            "export-repeat",
            "export-ack-mismatch",
            "export-ack",
            "export-ack-empty",
            "export-no-sealed",
            "export-rejected",
            "export-malformed",
            "export-partial",
            "export-hold",
            "export-over-capacity",
        ):
            self.assertIn(f'mode == "{mode}"', client)
        self.assertNotIn("subprocess", client)
        self.assertNotIn("open('/", client)

    def test_runtime_uses_restored_sysusers_af_unix_only_and_bounded(self) -> None:
        runtime = gate.build_run_script(self.paths(), self.assets())
        self.assertIn("systemd-sysusers --root=", runtime)
        self.assertIn("failure_phase", runtime)
        self.assertIn("phase=create-static-identities", runtime)
        self.assertIn('systemd-sysusers "$base_sysusers_source"', runtime)
        self.assertIn("static_identities created-and-restored", runtime)
        self.assertIn("account_database_restored yes", runtime)
        self.assertIn("systemctl show -p DynamicUser", runtime)
        self.assertIn("systemctl show -p PrivateNetwork", runtime)
        self.assertIn("five_sources_accepted 5", runtime)
        self.assertIn("control_rejection_accepted yes", runtime)
        self.assertIn("run_as hardened-blikvm-control control-disabled", runtime)
        self.assertIn("phase=export-connection-pressure", runtime)
        self.assertIn("run_as hardened-blikvm-control accepted-fast", runtime)
        self.assertIn("run_as hardened-blikvm-broker fill", runtime)
        self.assertIn("run_as hardened-blikvm-broker export-read", runtime)
        self.assertIn("run_as hardened-blikvm-broker export-ack-mismatch", runtime)
        self.assertIn("run_as hardened-blikvm-broker export-ack", runtime)
        self.assertIn("export_exact_consumer platform-broker", runtime)
        self.assertIn("producer_during_export_stall bounded", runtime)
        self.assertIn("graceful_export_shutdown yes", runtime)
        self.assertIn("duplicate_rejected yes", runtime)
        self.assertIn("partial_timeout bounded", runtime)
        self.assertIn("malformed_rejected yes", runtime)
        self.assertIn("root_rejected yes", runtime)
        self.assertIn("unknown_rejected yes", runtime)
        self.assertIn("graceful_shutdown yes", runtime)
        self.assertIn("--on-active=10min", runtime)
        self.assertIn(gate.BINARY_TARGET, runtime)
        self.assertIn('cp -a -- "$database" "$account_backup/$name"', runtime)
        self.assertNotIn("useradd", runtime)
        self.assertNotIn("usermod", runtime)
        self.assertIn('"$synthetic_root/etc/passwd"', runtime)

    def test_rollback_is_hash_bound_and_deletes_only_owned_external_paths(self) -> None:
        paths = self.paths()
        rollback = gate.build_rollback_script(paths, self.assets())
        self.assertIn('[ "$(cat "$marker")" = "$marker_value" ]', rollback)
        self.assertIn('sha256sum "$unit_target"', rollback)
        self.assertIn('sha256sum "$binary_target"', rollback)
        self.assertIn('! -name export.sock', rollback)
        self.assertIn('rm -f -- "$export_socket_path"', rollback)
        self.assertIn('sha256sum "$database"', rollback)
        self.assertIn('cmp -s "$account_backup/$name" "$database"', rollback)
        self.assertIn('elif [ "$current" != "$before" ]', rollback)
        self.assertIn('rmdir "$binary_parent"', rollback)
        self.assertIn(
            '[ "$parent_before" = absent ] && [ -e "$binary_parent" ]',
            rollback,
        )
        self.assertIn('rm -rf -- "$stage_root"', rollback)
        self.assertNotIn('rm -rf -- "$runtime_directory"', rollback)
        self.assertNotIn('rm -rf -- "$binary_parent"', rollback)
        self.assertNotIn("userdel", rollback)
        self.assertNotIn("groupdel", rollback)

    def test_policy_excludes_physical_system_and_kvmd_actions(self) -> None:
        policy = gate.policy_receipt()
        self.assertIs(policy["deviceActionsExecuted"], False)
        self.assertIs(policy["physicalPowerOrReset"], False)
        self.assertIs(policy["rawLogsRead"], False)
        self.assertIs(policy["persistentStateRestored"], True)
        self.assertEqual(policy["networkAccess"], "private-af-unix-only")
        paths = self.paths()
        scripts = "\n".join(
            (
                gate.build_preflight_script(paths, 20_000_000),
                gate.build_run_script(paths, self.assets()),
                gate.build_rollback_script(paths, self.assets()),
                gate.build_final_script(paths, "present"),
            )
        )
        self.assertNotRegex(
            scripts,
            r"(?m)^[ \t]*(?:rw|ro|reboot|shutdown|poweroff)(?:[ \t]|$)",
        )
        self.assertNotRegex(
            scripts,
            r"systemctl (?:stop|restart|reload) kvmd(?:-nginx)?\.service",
        )
        self.assertNotIn("/api/atx", scripts)
        self.assertNotIn("/api/hid", scripts)
        self.assertNotIn("/dev/gpio", scripts)

    def test_fact_validators_are_exact(self) -> None:
        self.assertEqual(
            gate.parse_runtime_failure(b"failure_phase=create-static-identities\n"),
            "create-static-identities",
        )
        preflight = {
            "account_absent_before": "yes",
            "account_database_ready": "yes",
            "architecture": "aarch64",
            "binary_absent_before": "yes",
            "binary_parent_before": "present",
            "identities_absent_before": "yes",
            "kvmd_active_before": "active",
            "kvmd_nginx_active_before": "active",
            "rootfs_mode_before": "rw",
            "runtime_absent_before": "yes",
            "export_socket_absent_before": "yes",
            "stage_absent_before": "yes",
            "systemd_ready": "yes",
            "tools_ready": "yes",
            "unit_absent_before": "yes",
            "watchdog_absent_before": "yes",
        }
        gate.validate_preflight(dict(sorted(preflight.items())))
        run = {
            "account_database_restored": "yes",
            "binary_hash_verified": "yes",
            "collector_caps": "none",
            "collector_identity": "static-sysusers",
            "collector_network_namespace": "private",
            "collector_no_new_privs": "yes",
            "collector_unit": "active-then-stopped",
            "control_rejection_accepted": "yes",
            "duplicate_rejected": "yes",
            "export_acknowledged": "yes",
            "export_exact_consumer": "platform-broker",
            "export_malformed_rejected": "yes",
            "export_mismatch_preserved": "yes",
            "export_no_sealed_after_ack": "yes",
            "export_over_capacity_rejected": "yes",
            "export_partial_timeout": "bounded",
            "export_repeatable": "yes",
            "export_socket_mode": "660",
            "export_socket_owner": "static-shared",
            "export_unauthorized_rejected": "root-and-wrong-role",
            "five_sources_accepted": "5",
            "graceful_export_shutdown": "yes",
            "graceful_shutdown": "yes",
            "kvmd_active_after_run": "active",
            "kvmd_nginx_active_after_run": "active",
            "malformed_rejected": "yes",
            "partial_timeout": "bounded",
            "producer_during_export_stall": "bounded",
            "root_rejected": "yes",
            "rollback_confirmed": "yes",
            "runtime_removed": "yes",
            "socket_mode": "660",
            "socket_owner": "static-shared",
            "static_identities": "created-and-restored",
            "sysusers_synthetic": "yes",
            "unit_hash_verified": "yes",
            "unknown_rejected": "yes",
            "watchdog_armed": "yes",
        }
        gate.validate_run(dict(sorted(run.items())))
        final = {
            "account_absent_final": "yes",
            "binary_absent_final": "yes",
            "binary_parent_final": "present",
            "kvmd_active_final": "active",
            "kvmd_nginx_active_final": "active",
            "rootfs_mode_final": "rw",
            "runtime_absent_final": "yes",
            "export_socket_absent_final": "yes",
            "stage_absent_final": "yes",
            "unit_absent_final": "yes",
            "watchdog_inactive_final": "yes",
        }
        gate.validate_final(dict(sorted(final.items())), "present")
        run["socket_mode"] = "666"
        with self.assertRaises(ValueError):
            gate.validate_run(dict(sorted(run.items())))

    def test_configuration_validation_requires_exact_source_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "collector.service"
            path.write_text("[Service]\nExecStart=/bin/true\n")
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                gate.validate_config(
                    str(path),
                    Path(
                        "packaging/systemd/"
                        "hardened-blikvm-diagnostic-collector.service"
                    ),
                )

    def test_binary_validation_rejects_non_hash_named_input(self) -> None:
        with tempfile.NamedTemporaryFile(dir="/tmp") as handle:
            handle.write(b"not-elf")
            handle.flush()
            with self.assertRaises(ValueError):
                gate.validate_binary(handle.name)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        script = Path(gate.__file__).resolve()
        result = subprocess.run(
            ["python3", str(script), "--help"],
            cwd="/tmp",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
