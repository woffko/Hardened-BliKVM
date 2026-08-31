from pathlib import Path
import stat
import subprocess
import tempfile
import unittest

from tools import stage_device_system_action_store as stage


class StageDeviceSystemActionStoreTests(unittest.TestCase):
    def private_known_hosts(self, contents: str) -> tempfile.TemporaryDirectory[str]:
        directory = tempfile.TemporaryDirectory()
        path = Path(directory.name) / "known_hosts"
        path.write_text(contents, encoding="ascii")
        path.chmod(0o600)
        self.assertEqual(path.stat().st_mode & 0o777, 0o600)
        return directory

    def test_target_is_derived_from_one_explicit_known_hosts_endpoint(self) -> None:
        directory = self.private_known_hosts(
            "[192.0.2.10]:2222 ssh-ed25519 AAAA\n"
            "[192.0.2.10]:2222 ssh-rsa BBBB\n"
        )
        with directory:
            target = stage.target_from_known_hosts(
                str(Path(directory.name) / "known_hosts")
            )
            self.assertEqual(target.host, "192.0.2.10")
            self.assertEqual(target.port, 2222)
            self.assertEqual(target.username, "root")

    def test_hashed_wildcard_or_multiple_routing_is_rejected(self) -> None:
        cases = (
            "|1|hash|hash ssh-ed25519 AAAA\n",
            "*.example.test ssh-ed25519 AAAA\n",
            "192.0.2.10 ssh-ed25519 AAAA\n192.0.2.11 ssh-ed25519 BBBB\n",
        )
        for contents in cases:
            directory = self.private_known_hosts(contents)
            with directory, self.assertRaises(ValueError):
                stage.target_from_known_hosts(
                    str(Path(directory.name) / "known_hosts")
                )

    def test_explicit_endpoint_fallback_keeps_hashed_host_key_verification(self) -> None:
        directory = self.private_known_hosts(
            "|1|hash|hash ssh-ed25519 AAAA\n"
        )
        with directory:
            target = stage.target_from_known_hosts(
                str(Path(directory.name) / "known_hosts"),
                "ssh://192.0.2.10:2222",
            )
            self.assertEqual(target.host, "192.0.2.10")
            self.assertEqual(target.port, 2222)
            self.assertEqual(target.username, "root")

    def test_stage_paths_are_unique_bounded_and_marker_protected(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        self.assertRegex(paths.root, stage.STAGE_ROOT)
        self.assertTrue(paths.root.startswith("/tmp/"))
        self.assertEqual(paths.marker, f"{paths.root}/.hardened-blikvm-owner")
        self.assertEqual(paths.harness, f"{paths.root}/store-tests")
        self.assertEqual(paths.temporary_root, f"{paths.root}/tmp")
        with self.assertRaises(ValueError):
            stage.stage_paths("a" * 63, "b" * 16)

    def test_generated_remote_scripts_are_posix_shell(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        harness = stage.HarnessInfo(
            Path("/tmp/not-opened"), "a" * 64, 1_000_000, "b" * 40
        )
        scripts = (
            stage.build_preflight_script(paths, 10_000_000),
            stage.build_upload_script(paths),
            stage.build_run_script(paths, harness),
            stage.build_cleanup_script(paths),
            stage.build_final_script(paths),
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

    def test_remote_scope_has_no_system_action_or_production_write(self) -> None:
        paths = stage.stage_paths("a" * 64, "b" * 16)
        harness = stage.HarnessInfo(
            Path("/tmp/not-opened"), "a" * 64, 1_000_000, "b" * 40
        )
        scripts = "\n".join(
            (
                stage.build_preflight_script(paths, 10_000_000),
                stage.build_upload_script(paths),
                stage.build_run_script(paths, harness),
                stage.build_cleanup_script(paths),
                stage.build_final_script(paths),
            )
        )
        for forbidden in (
            "systemctl stop",
            "systemctl restart",
            "systemctl start",
        ):
            self.assertNotIn(forbidden, scripts)
        self.assertNotRegex(
            scripts,
            r"(?m)^[ \t]*(?:rw|ro|reboot|shutdown|poweroff)(?:[ \t]|$)",
        )
        self.assertNotIn(f'mkdir -m 700 "{stage.PRODUCTION_STORE_ROOT}"', scripts)
        self.assertIn(
            f"production_root={stage.PRODUCTION_STORE_ROOT}", scripts
        )
        self.assertIn('[ ! -e "$production_root" ]', scripts)
        self.assertIn('TMPDIR="$temporary_root" "$harness"', scripts)

    def test_fact_parser_is_exact_and_bounded(self) -> None:
        expected = {"alpha", "beta"}
        self.assertEqual(
            stage.parse_facts(b"beta=yes\nalpha=1\n", expected),
            {"alpha": "1", "beta": "yes"},
        )
        for raw in (
            b"alpha=1\n",
            b"alpha=1\nalpha=2\nbeta=yes\n",
            b"alpha=1\nbeta=yes\nunknown=no\n",
            b"alpha=1\nbeta=unsafe value\n",
        ):
            with self.assertRaises(ValueError):
                stage.parse_facts(raw, expected)

    def test_allowlisted_success_facts_require_all_invariants(self) -> None:
        harness = stage.HarnessInfo(
            Path("/tmp/not-opened"), "a" * 64, 1_006_704, "b" * 40
        )
        preflight = {
            "architecture": "aarch64",
            "kvmd_active_before": "active",
            "kvmd_nginx_active_before": "active",
            "production_root_absent_before": "yes",
            "rootfs_mode_before": "rw",
            "stage_absent_before": "yes",
            "tmp_exec": "yes",
            "tools_ready": "yes",
        }
        stage.validate_preflight(dict(sorted(preflight.items())))
        run = {
            "architecture": "aarch64",
            "harness_sha256": harness.sha256,
            "harness_size_bytes": str(harness.size_bytes),
            "kvmd_active_after": "active",
            "kvmd_active_before": "active",
            "kvmd_nginx_active_after": "active",
            "kvmd_nginx_active_before": "active",
            "production_root_absent_after": "yes",
            "production_root_absent_before": "yes",
            "rootfs_mode_after": "rw",
            "rootfs_mode_before": "rw",
            "tests_failed": "0",
            "tests_ignored": "0",
            "tests_passed": str(stage.EXPECTED_TESTS),
        }
        stage.validate_run(dict(sorted(run.items())), harness)
        run["tests_failed"] = "1"
        with self.assertRaises(ValueError):
            stage.validate_run(dict(sorted(run.items())), harness)

    def test_harness_validation_rejects_non_elf_and_unsafe_mode(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "harness"
            path.write_bytes(b"not-elf")
            path.chmod(0o755)
            self.assertTrue(stat.S_ISREG(path.stat().st_mode))
            with self.assertRaises(ValueError):
                stage.validate_harness(str(path), "a" * 40)

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
