import json
from pathlib import Path
import subprocess
import sys
import unittest

import tools.stage_device_prototype as prototype
import tools.stage_device_tls_browser as tls


class StageDeviceTlsBrowserTests(unittest.TestCase):
    def _paths(self) -> tls.TlsPaths:
        return tls.tls_paths("a" * 64, "0123456789abcdef")

    def test_tls_paths_are_inside_marker_protected_stage(self) -> None:
        paths = self._paths()
        for value in (
            paths.nginx_state,
            paths.nginx_config,
            paths.tls_certificate,
            paths.tls_key,
            paths.ready,
            paths.browser_done,
            paths.browser_result,
            paths.video_runtime,
            paths.video_socket,
        ):
            self.assertTrue(value.startswith(paths.stage.root + "/"))

    def test_rendered_nginx_is_tls_loopback_and_uses_test_upstream(self) -> None:
        rendered = tls.render_test_nginx(self._paths(), 18081)
        self.assertIn("listen 127.0.0.1:18443 ssl;", rendered)
        self.assertNotIn("listen [::]", rendered)
        self.assertIn("proxy_pass http://127.0.0.1:18081;", rendered)
        self.assertIn("ssl_protocols TLSv1.2 TLSv1.3;", rendered)
        self.assertNotIn("/etc/hardened-blikvm/tls.key", rendered)

    def test_remote_gate_starts_and_cleans_the_video_proxy(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        remote = tls.build_remote_script(paths, bundle, 18081)
        cleanup = tls.build_cleanup_script(paths)
        self.assertIn("phase=start-video", remote)
        self.assertIn("BLI_KVM_USTREAMER_SOCKET=/run/kvmd/ustreamer.sock", remote)
        self.assertIn('BLI_KVM_VIDEO_SOCKET="$video_socket"', remote)
        self.assertIn('video.pid:$video', cleanup)

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        paths = self._paths()
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        scripts = (
            tls.build_preflight_script(paths, 10_000_000, 18081),
            tls.build_remote_script(paths, bundle, 18081),
            tls.build_signal_script(paths, "passed"),
            tls.build_cleanup_script(paths),
            tls.build_final_script(paths, 18081),
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

    def test_tunnel_is_local_only_and_has_no_remote_shell(self) -> None:
        target = prototype.RemoteTarget("device.invalid", 22, "root", Path("/known-hosts"))
        command = tls.tunnel_command(target, 17, 34567)
        rendered = " ".join(command)
        self.assertIn("127.0.0.1:34567:127.0.0.1:18443", rendered)
        self.assertIn("ExitOnForwardFailure=yes", rendered)
        self.assertIn(" -N ", f" {rendered} ")
        self.assertNotIn("sh -c", rendered)
        self.assertNotIn("ClearAllForwardings=yes", rendered)

    def test_browser_script_does_not_contain_fixed_password(self) -> None:
        script = (
            Path(__file__).resolve().parent
            / "playwright/device_tls_auth_smoke.mjs"
        ).read_text()
        self.assertIn("crypto.randomBytes", script)
        self.assertNotIn("admin1234", script)
        self.assertNotIn("10.0.", script)

    def test_browser_failure_envelope_is_bounded_and_allowlisted(self) -> None:
        self.assertEqual(
            tls.validate_browser_failure(
                b'{"schemaVersion":1,"result":"failed","phase":"cookies","errorType":"AssertionError"}'
            ),
            ("cookies", "AssertionError"),
        )
        with self.assertRaises(ValueError):
            _ = tls.validate_browser_failure(
                b'{"schemaVersion":1,"result":"failed","phase":"cookies","errorType":"SecretError"}'
            )

    def test_browser_success_requires_decoded_reconnected_read_only_video(self) -> None:
        summary = {
            "schemaVersion": 1,
            "result": "passed",
            "protocol": "https",
            "title": "Remote Desktop - Hardened BliKVM",
            "sessionCookie": "secure-httponly-strict",
            "csrfCookie": "secure-readable-strict",
            "inventoryStatus": 200,
            "inventoryBackend": "kvmd_transition",
            "videoTransport": "mjpeg",
            "videoDecoded": "passed",
            "videoReconnects": 2,
            "streamRevocation": "passed",
            "controlRequestsObserved": 0,
            "webSocketsObserved": 0,
            "passwordRotation": "passed",
            "logout": "passed",
            "apiResponsesObserved": 8,
        }
        validated = tls.validate_browser(json.dumps(summary, separators=(",", ":")).encode())
        self.assertEqual(validated["videoReconnects"], 2)

        summary["videoReconnects"] = 1
        with self.assertRaises(ValueError):
            _ = tls.validate_browser(json.dumps(summary, separators=(",", ":")).encode())

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/stage_device_tls_browser.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
