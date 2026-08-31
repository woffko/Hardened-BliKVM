from pathlib import Path
import subprocess
import tempfile
import unittest

from tools import stage_device_system_action_maintenance as stage


class StageDeviceSystemActionMaintenanceTests(unittest.TestCase):
    def assets(self) -> stage.Assets:
        return stage.Assets(
            binary=stage.Asset(Path("/tmp/binary"), "a" * 64, 441_928),
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
            "hardened-blikvm-system-action-rollback-" + "b" * 16,
        )
        with self.assertRaises(ValueError):
            stage.stage_paths("a" * 63, "b" * 16)

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = (
            stage.build_preflight_script(paths, 10_000_000),
            stage.build_first_upload_script(paths),
            stage.build_upload_script(paths, paths.init_unit, "600"),
            stage.build_rollback_script(paths, assets),
            stage.build_run_script(paths, assets),
            stage.build_cleanup_script(paths, assets),
            stage.build_final_script(paths, "present"),
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

    def test_scope_has_no_system_action_or_broad_persistent_delete(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        assets = self.assets()
        scripts = "\n".join(
            (
                stage.build_preflight_script(paths, 10_000_000),
                stage.build_rollback_script(paths, assets),
                stage.build_run_script(paths, assets),
                stage.build_cleanup_script(paths, assets),
                stage.build_final_script(paths, "present"),
            )
        )
        self.assertNotRegex(
            scripts,
            r"(?m)^[ \t]*(?:rw|ro|reboot|shutdown|poweroff)(?:[ \t]|$)",
        )
        self.assertNotIn('rm -rf -- "$production_root"', scripts)
        self.assertIn('rm -f -- "$production_root/$copy"', scripts)
        self.assertIn('rmdir "$production_root"', scripts)
        self.assertIn("initialize-empty", Path(
            "packaging/systemd/hardened-blikvm-system-action-store-init.service"
        ).read_text())
        self.assertIn("inspect-empty", Path(
            "packaging/systemd/hardened-blikvm-system-action-store-inspect.service"
        ).read_text())

    def test_rollback_requires_exact_marker_hashes_and_copy_metadata(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        rollback = stage.build_rollback_script(paths, self.assets())
        self.assertIn('[ "$(cat "$marker")" = "$marker_value" ]', rollback)
        self.assertIn("journal-a.bin journal-b.bin", rollback)
        self.assertIn("600:0:0:1:2076", rollback)
        self.assertIn('sha256sum "$binary_target"', rollback)
        self.assertIn('sha256sum "$path"', rollback)
        self.assertIn('$binary_target.part:$binary_size:755', rollback)
        self.assertIn('systemctl daemon-reload', rollback)

    def test_preflight_requires_all_original_absence_and_service_facts(self) -> None:
        facts = {
            "architecture": "aarch64",
            "binary_absent_before": "yes",
            "binary_parent_before": "present",
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
        stage.validate_preflight(dict(sorted(facts.items())))
        facts["binary_parent_before"] = "unsafe"
        with self.assertRaises(ValueError):
            stage.validate_preflight(dict(sorted(facts.items())))

    def test_run_and_final_facts_are_exact(self) -> None:
        run = {
            "binary_hash_verified": "yes",
            "copies_equal": "yes",
            "copies_mode_verified": "yes",
            "empty_generation": "1",
            "init_result": "success",
            "inspect_result": "success",
            "rollback_confirmed": "yes",
            "sandbox_sources_verified": "yes",
            "watchdog_armed": "yes",
        }
        stage.validate_run(dict(sorted(run.items())))
        final = {
            "binary_absent_final": "yes",
            "binary_parent_final": "present",
            "init_unit_absent_final": "yes",
            "inspect_unit_absent_final": "yes",
            "kvmd_active_final": "active",
            "kvmd_nginx_active_final": "active",
            "production_root_absent_final": "yes",
            "rootfs_mode_final": "rw",
            "stage_absent_final": "yes",
            "watchdog_inactive_final": "yes",
        }
        stage.validate_final(dict(sorted(final.items())), "present")
        final["production_root_absent_final"] = "no"
        with self.assertRaises(ValueError):
            stage.validate_final(dict(sorted(final.items())), "present")

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
                        "hardened-blikvm-system-action-store-init.service"
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
