from pathlib import Path
import subprocess
import tempfile
import unittest

from tools import stage_device_diagnostic_maintenance as stage


class StageDeviceDiagnosticMaintenanceTests(unittest.TestCase):
    def assets(self) -> stage.Assets:
        return stage.Assets(
            binary=stage.Asset(Path("/tmp/binary"), "a" * 64, 485_048),
            init_unit=stage.Asset(Path("/tmp/init"), "b" * 64, 1_000),
            inspect_unit=stage.Asset(Path("/tmp/inspect"), "c" * 64, 1_000),
            tmpfiles=stage.Asset(Path("/tmp/tmpfiles"), "d" * 64, 100),
            source_commit="e" * 40,
        )

    def test_paths_are_unique_marker_protected_and_bounded(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        self.assertRegex(paths.root, stage.STAGE_ROOT)
        self.assertEqual(paths.marker, f"{paths.root}/.hardened-blikvm-owner")
        self.assertEqual(
            paths.watchdog,
            "hardened-blikvm-diagnostic-rollback-" + "b" * 16,
        )
        self.assertTrue(paths.synthetic_root.startswith(paths.root + "/"))
        with self.assertRaises(ValueError):
            stage.stage_paths("a" * 63, "b" * 16)

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = (
            stage.build_preflight_script(paths, 10_000_000),
            stage.build_first_upload_script(paths),
            stage.build_upload_script(paths, paths.init_unit),
            stage.build_rollback_script(paths, assets),
            stage.build_run_script(paths, assets),
            stage.build_cleanup_script(paths, assets),
            stage.build_final_script(paths, "present", "absent", "present"),
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

    def test_scope_excludes_physical_actions_raw_secrets_and_broad_delete(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = "\n".join(
            (
                stage.build_preflight_script(paths, 10_000_000),
                stage.build_rollback_script(paths, assets),
                stage.build_run_script(paths, assets),
                stage.build_cleanup_script(paths, assets),
                stage.build_final_script(paths, "present", "absent", "present"),
            )
        )
        self.assertNotRegex(
            scripts,
            r"(?m)^[ \t]*(?:rw|ro|reboot|shutdown|poweroff)(?:[ \t]|$)",
        )
        self.assertNotIn("rm -rf", scripts)
        self.assertNotIn('rm -f -- "$production_root"', scripts)
        self.assertNotIn('cat "$credential_target"', scripts)
        self.assertNotIn('sha256sum "$credential_target"', scripts)
        self.assertNotIn("admin1234", scripts)
        self.assertIn('rm -f -- "$path"', scripts)
        self.assertIn('rmdir "$production_root"', scripts)

    def test_synthetic_credential_is_fixed_random_and_never_emitted(self) -> None:
        script = stage.build_run_script(
            stage.stage_paths("a" * 64, "b" * 16), self.assets()
        )
        self.assertIn("printf 'BLIDKEY\\000\\001\\001", script)
        self.assertIn("dd if=/dev/urandom bs=32 count=1 status=none", script)
        self.assertIn("dd if=/dev/zero bs=48 count=1 status=none", script)
        self.assertIn("od -An -v -j16 -N32 -tx1", script)
        self.assertIn(f"= {stage.CREDENTIAL_BYTES} ]", script)
        self.assertNotIn("emit credential_", script.split("/bin/sh \"$rollback_script\"", 1)[0])

    def test_rollback_requires_marker_hashes_metadata_and_database_cas(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        rollback = stage.build_rollback_script(paths, self.assets())
        self.assertIn('[ "$(cat "$marker")" = "$marker_value" ]', rollback)
        self.assertIn("journal-a.bin journal-b.bin .journal-a.tmp .journal-b.tmp", rollback)
        self.assertIn(f"[ \"$size\" = {stage.COPY_BYTES} ]", rollback)
        self.assertIn(f"-le {stage.CREDENTIAL_BYTES}", rollback)
        self.assertIn('sha256sum "$binary_target"', rollback)
        self.assertIn('sha256sum "$path"', rollback)
        self.assertIn('current=$(sha256sum "$database"', rollback)
        self.assertIn('cmp -s "$account_backup/$name" "$database"', rollback)
        self.assertNotIn('rm -f -- "$database"', rollback)

    def test_account_transition_is_predicted_before_watchdog_and_live_write(self) -> None:
        script = stage.build_run_script(
            stage.stage_paths("a" * 64, "b" * 16), self.assets()
        )
        prediction = script.index("modified_%s=%s")
        watchdog = script.index("systemd-run --quiet --unit=\"$watchdog\"")
        live_write = script.index('systemd-sysusers "$sysusers_file"')
        self.assertLess(prediction, watchdog)
        self.assertLess(watchdog, live_write)
        self.assertIn("g hardened-blikvm %s -", script)
        self.assertIn("u hardened-blikvm-broker %s", script)

    def test_preflight_run_and_final_facts_are_exact(self) -> None:
        preflight = {
            "architecture": "aarch64",
            "binary_absent_before": "yes",
            "binary_parent_before": "present",
            "credential_absent_before": "yes",
            "credential_parent_before": "absent",
            "identity_state_before": "present",
            "init_unit_absent_before": "yes",
            "inspect_unit_absent_before": "yes",
            "kvmd_active_before": "active",
            "kvmd_nginx_active_before": "active",
            "production_root_absent_before": "yes",
            "rootfs_mode_before": "rw",
            "stage_absent_before": "yes",
            "tools_ready": "yes",
            "watchdog_absent_before": "yes",
        }
        stage.validate_preflight(dict(sorted(preflight.items())))
        run = {
            "account_databases_restored": "yes",
            "binary_hash_verified": "yes",
            "copies_equal": "yes",
            "copies_mode_verified": "yes",
            "credential_runtime_accepted": "yes",
            "empty_generation": "1",
            "init_result": "success",
            "inspect_nonmutating": "yes",
            "inspect_result": "success",
            "rollback_confirmed": "yes",
            "sandbox_sources_verified": "yes",
            "temporary_rejected_preserved": "yes",
            "watchdog_armed": "yes",
        }
        stage.validate_run(dict(sorted(run.items())))
        final = {
            "binary_absent_final": "yes",
            "binary_parent_final": "present",
            "credential_absent_final": "yes",
            "credential_parent_final": "absent",
            "identity_state_final": "present",
            "init_unit_absent_final": "yes",
            "inspect_unit_absent_final": "yes",
            "kvmd_active_final": "active",
            "kvmd_nginx_active_final": "active",
            "production_root_absent_final": "yes",
            "rootfs_mode_final": "rw",
            "stage_absent_final": "yes",
            "watchdog_inactive_final": "yes",
        }
        stage.validate_final(
            dict(sorted(final.items())), "present", "absent", "present"
        )
        final["production_root_absent_final"] = "no"
        with self.assertRaises(ValueError):
            stage.validate_final(
                dict(sorted(final.items())), "present", "absent", "present"
            )

    def test_configuration_validation_requires_exact_source_path(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "unit.service"
            path.write_text("[Service]\nExecStart=/bin/true\n")
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                stage.validate_config(
                    str(path),
                    Path(
                        "packaging/systemd/"
                        "hardened-blikvm-diagnostic-store-init.service"
                    ),
                )

    def test_binary_validation_rejects_non_hash_named_input(self) -> None:
        with tempfile.NamedTemporaryFile(dir="/tmp") as handle:
            handle.write(b"not-elf")
            handle.flush()
            with self.assertRaises(ValueError):
                stage.validate_binary(handle.name)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        script = Path(stage.__file__).resolve()
        result = subprocess.run(
            [str(script), "--help"],
            cwd="/tmp",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
