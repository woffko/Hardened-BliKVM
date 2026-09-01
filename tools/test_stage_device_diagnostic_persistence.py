from pathlib import Path
import subprocess
import tempfile
import unittest

from tools import stage_device_diagnostic_persistence as stage


class StageDeviceDiagnosticPersistenceTests(unittest.TestCase):
    def assets(self) -> stage.Assets:
        return stage.Assets(
            binary=stage.Asset(Path("/tmp/gate"), "a" * 64, 1_200_000),
            unit=stage.Asset(Path("/tmp/gate.service"), "b" * 64, 1_500),
            source_commit="c" * 40,
        )

    def test_paths_are_unique_marker_protected_and_bounded(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        self.assertRegex(paths.root, stage.STAGE_ROOT)
        self.assertEqual(paths.marker, f"{paths.root}/.hardened-blikvm-owner")
        self.assertEqual(
            paths.watchdog,
            "hardened-blikvm-diagnostic-persistence-rollback-" + "b" * 16,
        )
        self.assertTrue(paths.synthetic_root.startswith(paths.root + "/"))
        with self.assertRaises(ValueError):
            stage.stage_paths("a" * 63, "b" * 16)

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = (
            stage.build_preflight_script(paths, 20_000_000),
            stage.build_first_upload_script(paths),
            stage.build_upload_script(paths, paths.unit),
            stage.build_rollback_script(paths, assets),
            stage.build_run_script(paths, assets),
            stage.build_cleanup_script(paths, assets),
            stage.build_final_script(paths, "present", "absent"),
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

    def test_scope_excludes_physical_actions_secrets_and_broad_delete(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = "\n".join(
            (
                stage.build_preflight_script(paths, 20_000_000),
                stage.build_rollback_script(paths, assets),
                stage.build_run_script(paths, assets),
                stage.build_cleanup_script(paths, assets),
                stage.build_final_script(paths, "present", "absent"),
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
        self.assertIn('rmdir "$production_root"', scripts)
        self.assertIn('rmdir "$runtime_root"', scripts)

    def test_synthetic_credential_is_random_fixed_and_never_emitted(self) -> None:
        script = stage.build_run_script(
            stage.stage_paths("a" * 64, "b" * 16), self.assets()
        )
        self.assertIn("printf 'BLIDKEY\\000\\001\\001", script)
        self.assertIn("dd if=/dev/urandom bs=32 count=1 status=none", script)
        self.assertIn("dd if=/dev/zero bs=48 count=1 status=none", script)
        self.assertIn("od -An -v -j16 -N32 -tx1", script)
        self.assertNotIn("emit credential_", script.split("phase=run-gate", 1)[0])

    def test_rollback_is_marker_hash_metadata_and_account_cas_bound(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        rollback = stage.build_rollback_script(paths, self.assets())
        self.assertIn('[ "$(cat "$marker")" = "$marker_value" ]', rollback)
        self.assertIn("journal-a.bin journal-b.bin .journal-a.tmp .journal-b.tmp", rollback)
        self.assertIn(f'[ "$size" = {stage.COPY_BYTES} ]', rollback)
        self.assertIn(f"-le {stage.CREDENTIAL_BYTES}", rollback)
        self.assertIn('sha256sum "$binary_target"', rollback)
        self.assertIn('sha256sum "$unit_target"', rollback)
        self.assertIn('sha256sum "$result_file"', rollback)
        self.assertIn('current=$(sha256sum "$database"', rollback)
        self.assertIn('cmp -s "$account_backup/$name" "$database"', rollback)
        self.assertNotIn('rm -f -- "$database"', rollback)
        self.assertIn("rollback_failure_phase=%s", rollback)

    def test_early_cleanup_has_exact_sysusers_auxiliary_allowlist(self) -> None:
        cleanup = stage.build_cleanup_script(
            stage.stage_paths("a" * 64, "b" * 16), self.assets()
        )
        self.assertIn("! -name rollback.state.part", cleanup)
        self.assertIn("synthetic-root/etc/.pwd.lock", cleanup)
        for name in ("passwd-", "group-", "shadow-", "gshadow-"):
            self.assertIn(f"synthetic-root/etc/{name}", cleanup)
        self.assertNotIn("rm -rf", cleanup)

    def test_account_transition_is_predicted_before_watchdog_and_write(self) -> None:
        script = stage.build_run_script(
            stage.stage_paths("a" * 64, "b" * 16), self.assets()
        )
        prediction = script.index("modified_%s=%s")
        watchdog = script.index('systemd-run --quiet --unit="$watchdog"')
        live_write = script.index('systemd-sysusers "$sysusers_file"')
        self.assertLess(prediction, watchdog)
        self.assertLess(watchdog, live_write)
        for phase in (
            "prepare-rollback-state",
            "arm-watchdog",
            "create-identities",
            "install-runtime",
            "create-credential",
            "create-root",
            "run-gate",
            "rollback",
        ):
            self.assertIn(f"phase={phase}", script)

    def test_preflight_run_and_final_facts_are_exact(self) -> None:
        preflight = {
            "architecture": "aarch64",
            "binary_absent_before": "yes",
            "binary_parent_before": "present",
            "credential_absent_before": "yes",
            "credential_parent_before": "absent",
            "identities_absent_before": "yes",
            "kvmd_active_before": "active",
            "kvmd_nginx_active_before": "active",
            "production_root_absent_before": "yes",
            "rootfs_mode_before": "rw",
            "runtime_root_absent_before": "yes",
            "stage_absent_before": "yes",
            "tools_ready": "yes",
            "unit_absent_before": "yes",
            "watchdog_absent_before": "yes",
        }
        stage.validate_preflight(dict(sorted(preflight.items())))
        run = {
            "account_databases_restored": "yes",
            "acknowledged_entries": "2",
            "acknowledgement_requests": "3",
            "automatic_retries": "0",
            "binary_hash_verified": "yes",
            "cancelled_acknowledgements": "1",
            "copies_equal": "yes",
            "copies_mode_verified": "yes",
            "credential_runtime_accepted": "yes",
            "entry_count": "2",
            "final_generation": "7",
            "read_requests": "7",
            "rollback_confirmed": "yes",
            "sandbox_verified": "yes",
            "watchdog_armed": "yes",
        }
        stage.validate_run(dict(sorted(run.items())))
        final = {
            "binary_absent_final": "yes",
            "binary_parent_final": "present",
            "credential_absent_final": "yes",
            "credential_parent_final": "absent",
            "identities_absent_final": "yes",
            "kvmd_active_final": "active",
            "kvmd_nginx_active_final": "active",
            "production_root_absent_final": "yes",
            "rootfs_mode_final": "rw",
            "runtime_root_absent_final": "yes",
            "stage_absent_final": "yes",
            "unit_absent_final": "yes",
            "watchdog_inactive_final": "yes",
        }
        stage.validate_final(dict(sorted(final.items())), "present", "absent")
        final["production_root_absent_final"] = "no"
        with self.assertRaises(ValueError):
            stage.validate_final(dict(sorted(final.items())), "present", "absent")

    def test_expected_result_is_exact_and_public(self) -> None:
        self.assertEqual(stage.EXPECTED_GATE_RESULT_BYTES, 171)
        self.assertRegex(stage.EXPECTED_GATE_RESULT_SHA256, stage.SHA256)
        self.assertEqual(
            stage.EXPECTED_GATE_RESULT.splitlines()[1], "result=passed"
        )
        self.assertNotIn("key", stage.EXPECTED_GATE_RESULT)
        self.assertNotIn("binding", stage.EXPECTED_GATE_RESULT)

    def test_configuration_and_binary_validation_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "unit.service"
            path.write_text("[Service]\nExecStart=/bin/true\n")
            path.chmod(0o644)
            options = stage.Options(
                binary="/tmp/not-valid",
                source_commit="a" * 40,
                unit=str(path),
                endpoint="",
                known_hosts="private",
                output="private",
            )
            with self.assertRaises(ValueError):
                stage.validate_assets(options)
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
