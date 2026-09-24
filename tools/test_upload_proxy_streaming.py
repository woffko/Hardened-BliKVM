from pathlib import Path
from contextlib import nullcontext
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from tools import upload_proxy_streaming as proxy


class TransformTests(unittest.TestCase):
    def setUp(self) -> None:
        # This migration targets the legacy single-location recipe, not the
        # current package that already includes bounded streaming locations.
        self.original = b"""http {
    server {
        listen 443 ssl;
        client_max_body_size 16m;
        location / {
            proxy_pass http://127.0.0.1:18080;
            proxy_http_version 1.1;
            proxy_set_header Host $hardened_blikvm_forwarded_host;
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header X-Forwarded-For $remote_addr;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $hardened_blikvm_connection_upgrade;
            proxy_buffering off;
            proxy_read_timeout 3600s;
        }
    }
}
"""

    def test_current_packaged_streaming_config_is_not_migrated_again(self) -> None:
        current = Path("packaging/nginx/hardened-blikvm.conf").read_bytes()
        with self.assertRaisesRegex(ValueError, "upload streaming configuration already exists"):
            proxy.transform(current)
    def test_only_exact_upload_location_is_added(self) -> None:
        changed = proxy.transform(self.original)
        start = changed.index(b"        location = /api/application-update/upload {")
        end = changed.index(b"        location / {", start)
        addition = changed[start:end]
        self.assertEqual(changed[:start] + changed[end:], self.original)
        self.assertEqual(changed.count(b"proxy_request_buffering off;"), 1)
        for line in (b"proxy_pass http://127.0.0.1:18080;", b"proxy_http_version 1.1;",
                     b"proxy_set_header Host $hardened_blikvm_forwarded_host;",
                     b"proxy_set_header X-Forwarded-Proto https;"):
            self.assertIn(line, addition)
        self.assertEqual(changed.count(b"client_max_body_size 16m;"), 1)
    def test_second_apply_and_ambiguous_locations_are_rejected(self) -> None:
        for content in (proxy.transform(self.original), self.original + self.original,
                        self.original.replace(b"location / {", b"location /other {")):
            with self.assertRaises(ValueError):
                proxy.transform(content)


    def test_other_upstream_or_nested_policy_is_not_adopted(self) -> None:
        for content in (self.original.replace(b"127.0.0.1:18080", b"127.0.0.1:19000"),
                        self.original.replace(b"proxy_buffering off;", b"if ($x) { return 403; }"),
                        self.original.replace(b"proxy_buffering off;", b"rewrite ^ /private;")):
            with self.assertRaises(ValueError):
                proxy.transform(content)


class RecoveryTests(unittest.TestCase):
    def test_failure_after_atomic_replace_can_restore_backup_bytes_and_mode(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            config = root / "nginx.conf"
            original = b"original configuration\n"
            candidate = b"candidate configuration\n"
            config.write_bytes(original)
            config.chmod(0o640)
            (root / "original.conf").write_bytes(original)
            plan: dict[str, object] = {"phase": "applying", "mode": 0o640,
                "originalSha256": proxy.digest(original), "candidateSha256": proxy.digest(candidate)}
            with patch.object(proxy, "CONFIG", config), patch.object(proxy, "config_bytes", side_effect=config.read_bytes), \
                    patch.object(proxy, "load", return_value=plan), patch.object(proxy.native, "private_file", return_value=original), \
                    patch.object(proxy.u, "checked"), patch.object(proxy, "verify"), patch.object(proxy, "save"):
                proxy.replace_config(root, plan, candidate)
                self.assertEqual(config.read_bytes(), candidate)
                # Model termination after replacement, before the acceptance marker.
                proxy.recover(root)
                self.assertEqual(config.read_bytes(), original)
                self.assertEqual(config.stat().st_mode & 0o777, 0o640)
                self.assertEqual((root / "original.conf").read_bytes(), original)

    def test_main_and_proof_services_share_recovery_mechanism(self) -> None:
        root = Path("/var/lib") / (proxy.PREFIX + "a" * 24)
        for proof in (None, "failure", "timeout"):
            argv = proxy.service_command(root, proof=proof)
            self.assertIn("--property=KillMode=control-group", argv)
            hooks = [arg for arg in argv if arg.startswith("--property=ExecStopPost=")]
            self.assertEqual(len(hooks), 1)
            self.assertIn(str(root / "controller.py"), hooks[0])
            self.assertIn("guard-proof" if proof else "recover", hooks[0])
        self.assertIn("--property=TimeoutStartSec=1", proxy.service_command(root, proof="timeout"))

    def test_already_restored_recovery_only_verifies(self) -> None:
        with patch.object(proxy.native.temporary.guard, "locked", return_value=nullcontext()), \
                patch.object(proxy, "load", return_value={"phase": "restored"}), \
                patch.object(proxy, "verify") as verify, patch.object(proxy, "replace_config") as replace, \
                patch.object(proxy.u, "checked") as command:
            proxy.recover(Path("/unused"))
        verify.assert_called_once_with(Path("/unused"), {"phase": "restored"}, changed=False)
        replace.assert_not_called()
        command.assert_not_called()

    def test_staged_controller_imports_without_workspace_dependencies(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = Path(proxy.__file__).parent
            for name in proxy.ASSETS:
                if name.endswith(".py"):
                    (root / name).write_bytes((source / name).read_bytes())
            (root / "controller.py").write_bytes(Path(proxy.__file__).read_bytes())
            result = subprocess.run([sys.executable, "-B", str(root / "controller.py"), "--help"],
                                    capture_output=True, timeout=5, check=False)
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_accepted_change_is_not_undone_by_stop_post(self) -> None:
        with patch.object(proxy.native.temporary.guard, "locked", return_value=nullcontext()), \
                patch.object(proxy, "load", return_value={"phase": "accepted"}), \
                patch.object(proxy, "replace_config") as replace, patch.object(proxy.u, "checked") as command:
            proxy.recover(Path("/unused"))
        replace.assert_not_called()
        command.assert_not_called()

    def test_failed_apply_restores_before_reload_and_verification(self) -> None:
        events: list[str] = []
        plan: dict[str, object] = {"phase": "applying", "originalSha256": "old"}
        def replace(*_args: object) -> None: events.append("restore")
        def command(argv: list[str], *, timeout: int) -> bytes:
            events.append("syntax" if "-t" in argv else "reload")
            return b""
        def verify(*_args: object, changed: bool) -> None:
            self.assertFalse(changed)
            events.append("verify")
        def save(*_args: object) -> None:
            self.assertEqual(plan["phase"], "restored")
            events.append("record")
        with patch.object(proxy.native.temporary.guard, "locked", return_value=nullcontext()), \
                patch.object(proxy, "load", return_value=plan), patch.object(proxy.native, "private_file", return_value=b"old"), \
                patch.object(proxy, "replace_config", side_effect=replace), patch.object(proxy.u, "checked", side_effect=command), \
                patch.object(proxy, "verify", side_effect=verify), patch.object(proxy, "save", side_effect=save):
            proxy.recover(Path("/unused"))
        self.assertEqual(events, ["restore", "syntax", "reload", "verify", "record"])
