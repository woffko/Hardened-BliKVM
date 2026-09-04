from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_update_trust_config as stage


class StageDeviceUpdateTrustConfigTests(unittest.TestCase):
    def assets(self) -> stage.Assets:
        return stage.Assets(
            validator=stage.Asset(
                Path("/tmp/hardened-blikvm-update-trust-config-" + "a" * 64),
                "a" * 64,
                503_800,
            ),
            request=stage.Asset(
                Path(
                    "/tmp/hardened-blikvm-update-trust-request-"
                    + "b" * 64
                    + ".json"
                ),
                "b" * 64,
                512,
            ),
            source_commit="c" * 40,
        )

    def paths(self) -> stage.Paths:
        return stage.paths("b" * 64, "0123456789abcdef")

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "1" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "2" * 64,
        }

    def preflight(self) -> dict[str, str]:
        values = {key: "yes" for key in stage.PRE_KEYS}
        values.update(
            {
                "architecture": "aarch64",
                "rootfs_mode": "rw",
                "account_sha256": "3" * 64,
                "nginx_config_sha256": "4" * 64,
                "tls_sha256": "1" * 64,
                "tls_key_sha256": "2" * 64,
                "broker_pid": "101",
                "video_pid": "102",
                "server_pid": "103",
                "hardened_nginx_pid": "104",
                "broker_restarts": "0",
                "video_restarts": "0",
                "server_restarts": "0",
                "hardened_nginx_restarts": "0",
            }
        )
        return values

    def test_paths_are_bounded_and_separate_tmp_from_persistent_rollback(self) -> None:
        paths = self.paths()
        self.assertTrue(paths.stage_root.startswith("/tmp/hardened-blikvm-trust-enrollment-"))
        self.assertTrue(
            paths.rollback_root.startswith(
                "/var/lib/hardened-blikvm-trust-enrollment-"
            )
        )
        self.assertEqual(
            paths.config_part,
            "/etc/hardened-blikvm/.update-supervisor-0123456789abcdef.part",
        )

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = self.paths()
        assets = self.assets()
        baseline = self.baseline()
        preflight = self.preflight()
        config_hash = "d" * 64
        scripts = (
            stage.build_preflight_script(paths, assets, baseline),
            stage.build_upload_script(paths, assets, request=False),
            stage.build_upload_script(paths, assets, request=True),
            stage.build_validate_script(paths, assets),
            stage.build_rollback_script(
                paths, assets, baseline, preflight, config_hash
            ),
            stage.build_prepare_script(
                paths, assets, baseline, preflight, config_hash
            ),
            stage.build_apply_script(
                paths, assets, baseline, preflight, config_hash
            ),
            stage.build_final_script(
                paths,
                assets,
                baseline,
                preflight,
                config_hash,
                remove_guard=True,
            ),
            stage.build_final_script(
                paths,
                assets,
                baseline,
                preflight,
                config_hash,
                remove_guard=False,
            ),
            stage.build_cleanup_script(
                paths, assets, baseline, preflight, config_hash
            ),
            stage.build_abandon_script(paths, assets),
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

    def test_transient_validator_and_publication_are_review_hash_and_rollback_bound(self) -> None:
        paths = self.paths()
        assets = self.assets()
        baseline = self.baseline()
        preflight = self.preflight()
        config_hash = "d" * 64
        validate = stage.build_validate_script(paths, assets)
        apply = stage.build_apply_script(
            paths, assets, baseline, preflight, config_hash
        )
        rollback = stage.build_rollback_script(
            paths, assets, baseline, preflight, config_hash
        )
        prepare = stage.build_prepare_script(
            paths, assets, baseline, preflight, config_hash
        )
        self.assertIn("--confirm-reviewed-public-key", validate)
        self.assertIn('chmod 600 "$config_part"', apply)
        self.assertIn('mv "$config_part" "$config_target"', apply)
        self.assertIn('sha256sum "$config_target"', apply + rollback)
        self.assertIn("OnActiveSec=600s", stage.build_timer(paths))
        self.assertIn("WantedBy=sysinit.target", stage.build_boot_service(paths))
        self.assertIn("rollback_ready=yes", prepare)
        self.assertIn("update_runtime_absent", apply + rollback)
        self.assertIn("update_units_inert", apply + rollback)
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
            self.assertNotIn(forbidden, validate + apply + rollback + prepare)

    def test_missing_review_confirmation_fails_before_reading_assets(self) -> None:
        options = stage.Options(
            validator="/missing-validator",
            expected_validator_sha256="a" * 64,
            source_commit="b" * 40,
            request="/missing-request",
            expected_request_sha256="c" * 64,
            confirm_reviewed_public_key=False,
            endpoint="ssh://example.invalid:22",
            username="root",
            known_hosts="/missing-known-hosts",
            baseline="/missing-baseline",
            output="/missing-output",
            preflight_only=True,
        )
        with self.assertRaisesRegex(ValueError, "confirmation"):
            stage.validate_assets(options)

    def test_preflight_only_returns_before_upload(self) -> None:
        source = Path(stage.__file__).read_text(encoding="utf-8")
        branch = source.index("if options.preflight_only:")
        upload = source.index('stage = "upload-validator"')
        self.assertLess(branch, upload)
        between = source[branch:upload]
        self.assertIn('"deviceChanged": False', between)
        self.assertIn('"configPublished": False', between)
        self.assertNotIn("build_upload_script", between)

    def test_fact_parser_is_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(stage.APPLY_KEYS)).encode()
        self.assertEqual(set(stage.parse_facts(raw, stage.APPLY_KEYS)), stage.APPLY_KEYS)
        with self.assertRaises(ValueError):
            stage.parse_facts(raw + b"private=value\n", stage.APPLY_KEYS)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_update_trust_config.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn(b"--confirm-reviewed-public-key", result.stdout)
        self.assertIn(b"--preflight-only", result.stdout)

    def test_design_records_external_review_gate_and_remaining_absence(self) -> None:
        design = Path(
            "docs/design/update-trust-provisioning-gate-v1.md"
        ).read_text(encoding="utf-8")
        for required in (
            "user-reviewed public-only enrollment request",
            "must stop before contacting the device",
            "A production private key is never an input",
            "update state or slots",
            "ATX Power/Reset",
        ):
            self.assertIn(required, design)
        for forbidden in ("10.0.", "admin1234", "ssh_password", "gui_password"):
            self.assertNotIn(forbidden, design)


if __name__ == "__main__":
    unittest.main()
