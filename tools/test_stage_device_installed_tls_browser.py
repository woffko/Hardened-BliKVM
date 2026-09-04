from pathlib import Path
import subprocess
import sys
import unittest

import tools.runtime_provisioning_policy as identity_policy
import tools.stage_device_debian_runtime_package as package_gate
import tools.stage_device_installed_tls_browser as installed_tls
import tools.stage_device_runtime_provision as provision


class StageDeviceInstalledTlsBrowserTests(unittest.TestCase):
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

    def paths(self) -> installed_tls.Paths:
        return installed_tls.paths("a" * 64, "0" * 16)

    def test_paths_are_marker_owned_and_non_conflicting(self) -> None:
        paths = self.paths()
        self.assertTrue(
            paths.root.startswith(
                "/var/lib/hardened-blikvm-installed-tls-rollback-aaaaaaaaaaaa-"
            )
        )
        for value in (
            paths.certificate,
            paths.key,
            paths.nginx_config,
            paths.nginx_state,
            paths.rollback_script,
        ):
            self.assertTrue(value.startswith(paths.root + "/"))
        self.assertNotIn("kvmd-nginx", paths.nginx_unit)

    def test_rendered_nginx_is_fixed_tls_loopback_and_installed_upstream(self) -> None:
        rendered = installed_tls.render_nginx(self.paths())
        self.assertIn("listen 127.0.0.1:18443 ssl;", rendered)
        self.assertNotIn("listen [::]", rendered)
        self.assertIn("proxy_pass http://127.0.0.1:18080;", rendered)
        self.assertIn("ssl_protocols TLSv1.2 TLSv1.3;", rendered)
        self.assertIn("ssl_session_tickets off;", rendered)
        self.assertNotIn("/etc/hardened-blikvm/tls", rendered)
        self.assertNotIn("/etc/nginx", rendered)

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        scripts = (
            installed_tls.build_preflight_script(self.paths()),
            installed_tls.build_rollback_script(self.paths(), self.assets()),
            installed_tls.build_start_script(self.paths(), self.assets()),
            installed_tls.build_commit_script(self.paths(), self.assets()),
            installed_tls.build_cleanup_script(self.paths(), self.assets()),
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

    def test_start_uses_installed_units_and_unprivileged_shadow_proxy(self) -> None:
        script = installed_tls.build_start_script(self.paths(), self.assets())
        for required in (
            "systemctl start hardened-blikvm-platform-broker.service",
            "systemctl start hardened-blikvm-video-proxy.service",
            "systemctl start hardened-blikvm-server.service",
            "--property=User=www-data",
            "--property=IPAddressDeny=any",
            "--property=IPAddressAllow=localhost",
            "--property=SocketBindAllow=tcp:18443",
            "--property=CapabilityBoundingSet=",
            "--property=ReadWritePaths=",
            "--tlsv1.2 --tls-max 1.2",
            "--tlsv1.3 --tls-max 1.3",
            "control_boundary_absent",
            "nginx_tree_hash",
        ):
            self.assertIn(required, script)
        for forbidden in (
            "systemctl start hardened-blikvm-control",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg -i",
            "dpkg --purge",
            "/etc/nginx/",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, script)

    def test_cleanup_preserves_package_identity_credentials_and_working_nginx(self) -> None:
        rollback = installed_tls.build_rollback_script(self.paths(), self.assets())
        commit = installed_tls.build_commit_script(self.paths(), self.assets())
        for script in (rollback, commit):
            self.assertIn("cgroup_empty", script)
            self.assertIn("socket_tail_clean", script)
            self.assertIn("control_boundary_absent", script)
            self.assertIn("nginx_tree_hash", script)
            self.assertNotIn("rm -rf", script)
            self.assertNotIn("/etc/hardened-blikvm/kvmd-password", script)
            self.assertNotIn("/etc/passwd", script)
            self.assertNotIn("systemctl stop kvmd", script)

    def test_host_flow_reuses_strict_tunnel_and_browser_contract(self) -> None:
        source = Path("tools/stage_device_installed_tls_browser.py").read_text()
        self.assertIn("tls.browser_runtime()", source)
        self.assertIn("tls.tunnel_command", source)
        self.assertIn("tls.validate_browser", source)
        self.assertIn("tls.validate_browser_failure", source)
        self.assertIn('"workingNginxMutation": False', source)
        self.assertIn('"servicesEnabled": False', source)
        self.assertIn('"controlServiceStarted": False', source)
        self.assertIn('"secretEmitted": False', source)

    def test_fact_and_failure_parsers_are_exact(self) -> None:
        raw = "".join(f"{key}=yes\n" for key in sorted(installed_tls.COMMIT_KEYS)).encode()
        self.assertEqual(
            set(installed_tls.parse_facts(raw, installed_tls.COMMIT_KEYS)),
            installed_tls.COMMIT_KEYS,
        )
        with self.assertRaises(ValueError):
            installed_tls.parse_facts(raw + b"private=yes\n", installed_tls.COMMIT_KEYS)
        self.assertEqual(
            installed_tls.parse_failure(
                b"failure_phase=nginx-start\nfailure_cleanup=restored\n"
            ),
            ("nginx-start", True),
        )

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_installed_tls_browser.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
