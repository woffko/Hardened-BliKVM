from pathlib import Path
import subprocess
import unittest

import tools.build_debian_runtime_package as debian
import tools.runtime_provisioning_policy as identity_policy
import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_runtime_provision as provision
import tools.stage_device_web_cutover as cutover


class StageDeviceWebCutoverTests(unittest.TestCase):
    def assets(self) -> provision.Assets:
        payload = identity_policy.SYSUSERS_SOURCE.read_bytes()
        package = package_gate.PackageInfo(
            path=Path("/package.deb"),
            sha256="a" * 64,
            size_bytes=4096,
            source_commit="b" * 40,
            source_bundle_sha256="c" * 64,
            builder_commit="d" * 40,
            installed_files={},
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

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "1" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "2" * 64,
        }

    def paths(self) -> cutover.Paths:
        return cutover.paths("a" * 64, "0123456789abcdef")

    def test_rendered_nginx_and_unit_preserve_narrow_boundary(self) -> None:
        config = cutover.render_nginx(self.baseline())
        validation = cutover.render_validation_nginx(
            self.baseline(), config, self.paths().validation_root
        )
        unit = cutover.render_unit(self.baseline())
        self.assertEqual(config.count("proxy_pass http://127.0.0.1:18080;"), 1)
        self.assertIn("listen 80 default_server;", config)
        self.assertIn("listen 443 ssl default_server;", config)
        self.assertIn("ssl_protocols TLSv1.2 TLSv1.3;", config)
        self.assertIn("/etc/kvmd/nginx/ssl/server.key", config)
        self.assertIn("/etc/kvmd/nginx/ssl/server.key", validation)
        self.assertNotIn("/run/credentials/", validation)
        self.assertIn(self.paths().validation_root, validation)
        self.assertNotIn("/run/hardened-blikvm-nginx/", validation)
        self.assertIn("User=root", unit)
        self.assertIn("-c /etc/hardened-blikvm/nginx.conf", unit)
        self.assertNotIn("LoadCredential=", unit)
        self.assertIn("Conflicts=kvmd-nginx.service", unit)
        self.assertIn(
            "CapabilityBoundingSet=CAP_NET_BIND_SERVICE CAP_SETGID CAP_SETUID",
            unit,
        )
        self.assertIn("AmbientCapabilities=\n", unit)
        self.assertIn("SocketBindAllow=tcp:80", unit)
        self.assertIn("SocketBindAllow=tcp:443", unit)
        self.assertNotIn("IPAddressAllow=localhost", unit)

    def test_generated_scripts_are_valid_posix_shell(self) -> None:
        paths = self.paths()
        assets = self.assets()
        baseline = self.baseline()
        config = cutover.render_nginx(baseline)
        unit = cutover.render_unit(baseline)
        scripts = (
            cutover.build_prepare_script(paths, assets, baseline, config, unit),
            cutover.build_rollback_script(paths, assets, baseline, config, unit),
            cutover.build_apply_script(paths, assets, baseline, config, unit),
            cutover.build_persistent_check_script(
                paths, baseline, config, unit, commit=True
            ),
            cutover.build_persistent_check_script(
                paths, baseline, config, unit, commit=False
            ),
            cutover.build_cleanup_script(paths, baseline, config, unit),
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

    def test_apply_replaces_only_vendor_web_and_enables_hardened_runtime(self) -> None:
        script = cutover.build_apply_script(
            self.paths(),
            self.assets(),
            self.baseline(),
            cutover.render_nginx(self.baseline()),
            cutover.render_unit(self.baseline()),
        )
        for required in (
            "systemctl disable --now kvmd-nginx.service",
            'systemctl enable "$nginx_unit"',
            "systemctl enable hardened-blikvm-platform-broker.service",
            "first_setup_required",
            "served_certificate_hash",
            "nginx_validation_b64",
            "phase=validate-nginx-config",
            'mkdir -m 700 "$validation_root/client-temp"',
            'chmod 600 "$config_part"',
            "00000000000004c0",
            "control_boundary_absent",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "systemctl stop kvmd.service",
            "systemctl disable kvmd.service",
            "systemctl start hardened-blikvm-control",
            "reboot",
            "poweroff",
            "shutdown",
            "rm -rf",
        ):
            self.assertNotIn(forbidden, script)

    def test_rollback_restores_vendor_web_and_removes_only_owned_state(self) -> None:
        script = cutover.build_rollback_script(
            self.paths(),
            self.assets(),
            self.baseline(),
            cutover.render_nginx(self.baseline()),
            cutover.render_unit(self.baseline()),
        )
        self.assertIn("systemctl enable kvmd-nginx.service", script)
        self.assertIn("systemctl start kvmd-nginx.service", script)
        self.assertIn("product_inactive_disabled", script)
        self.assertIn("remove_account_state", script)
        self.assertNotIn("rm -rf", script)
        self.assertNotIn("/etc/kvmd/nginx/ssl/server.key\"", script.split("rm -f --")[-1])

    def test_boot_rollback_precedes_both_web_units(self) -> None:
        unit = cutover.build_boot_unit(self.paths())
        self.assertIn(
            "Before=kvmd-nginx.service hardened-blikvm-nginx.service multi-user.target",
            unit,
        )
        self.assertIn("WantedBy=sysinit.target", unit)

    def test_browser_gate_never_creates_an_account(self) -> None:
        source = cutover.BROWSER_SCRIPT.read_text(encoding="utf-8")
        self.assertIn("First Setup - Hardened BliKVM", source)
        self.assertIn("setup.body.data.required", source)
        self.assertNotIn(".fill(", source)
        self.assertNotIn('name: "Create account" }).click', source)
        self.assertNotIn('method() === "POST"', source)

    def test_package_maps_the_disabled_nginx_unit_template(self) -> None:
        self.assertEqual(
            debian.FILE_MAP["share/systemd/hardened-blikvm-nginx.service"],
            "lib/systemd/system/hardened-blikvm-nginx.service",
        )

    def test_root_master_has_a_separate_bounded_security_threshold(self) -> None:
        verifier = Path("scripts/verify-systemd-units.sh").read_text(encoding="utf-8")
        self.assertIn("--threshold=20", verifier)
        self.assertIn("--threshold=22", verifier)
        self.assertIn(
            '"$project_root/packaging/systemd/hardened-blikvm-nginx.service"',
            verifier,
        )


if __name__ == "__main__":
    unittest.main()
