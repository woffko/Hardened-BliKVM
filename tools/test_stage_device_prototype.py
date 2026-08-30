import hashlib
import io
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest

from tools import stage_device_prototype as stage


class StageDevicePrototypeTests(unittest.TestCase):
    def test_endpoint_username_hash_and_port_validation(self) -> None:
        self.assertEqual(stage.parse_endpoint("ssh://device.invalid:2222"), ("device.invalid", 2222))
        self.assertEqual(stage.parse_endpoint("ssh://[2001:db8::1]"), ("2001:db8::1", 22))
        self.assertEqual(stage.validate_username("root"), "root")
        self.assertEqual(stage.validate_sha256("a" * 64), "a" * 64)
        self.assertEqual(stage.validate_bind_port(18081), 18081)
        for endpoint in (
            "https://device.invalid",
            "ssh://root:secret@device.invalid",
            "ssh://device.invalid/path",
            "ssh://bad_host.invalid",
        ):
            with self.assertRaises(ValueError):
                _ = stage.parse_endpoint(endpoint)
        with self.assertRaises(ValueError):
            _ = stage.validate_bind_port(443)

    def test_stage_path_is_bounded_and_marker_protected(self) -> None:
        paths = stage.stage_paths("b" * 64, "0123456789abcdef")
        self.assertEqual(
            paths.root,
            "/tmp/hardened-blikvm-prototype-bbbbbbbbbbbb-0123456789abcdef",
        )
        self.assertTrue(paths.marker.startswith(paths.root + "/"))
        self.assertTrue(paths.server.startswith(paths.root + "/"))
        with self.assertRaises(ValueError):
            _ = stage.stage_paths("b" * 64, "../../unsafe")

    def test_secret_fd_is_separate_from_process_stdin(self) -> None:
        secret = bytearray(b"private-test-value")

        def command(password_fd: int) -> list[str]:
            program = (
                "import hashlib, os, sys; "
                "secret=os.read(int(sys.argv[1]), 4096).rstrip(b'\\n'); "
                "payload=sys.stdin.buffer.read(); "
                "print(hashlib.sha256(secret).hexdigest()); "
                "print(hashlib.sha256(payload).hexdigest())"
            )
            return [sys.executable, "-c", program, str(password_fd)]

        with tempfile.TemporaryFile() as payload:
            _ = payload.write(b"bundle-stream")
            _ = payload.seek(0)
            result = stage.run_with_secret_fd(secret, command, stdin=payload, timeout=5)
        self.assertEqual(result.returncode, 0)
        self.assertEqual(
            result.stdout.decode().splitlines(),
            [
                hashlib.sha256(secret).hexdigest(),
                hashlib.sha256(b"bundle-stream").hexdigest(),
            ],
        )
        self.assertNotIn(secret.decode(), " ".join(command(99)))

    def test_parser_requires_complete_allowlisted_output(self) -> None:
        expected = {"one", "two"}
        self.assertEqual(stage.parse_facts(b"two=yes\none=200\n", expected), {"one": "200", "two": "yes"})
        for raw in (b"one=200\n", b"one=200\none=201\ntwo=yes\n", b"secret=value\n"):
            with self.assertRaises(ValueError):
                _ = stage.parse_facts(raw, expected)

    def test_runtime_failure_parser_exposes_only_phase_and_cleanup(self) -> None:
        raw = b"failure_phase=stop-server\nfailure_cleanup=removed\n"
        self.assertEqual(stage.parse_runtime_failure(raw), ("stop-server", True))
        result = subprocess.CompletedProcess(["ssh"], 1, stdout=raw, stderr=b"")
        with self.assertRaises(stage.StageError) as raised:
            _ = stage._require_result(  # pyright: ignore[reportPrivateUsage]
                "runtime", result
            )
        self.assertEqual(raised.exception.remote_phase, "stop-server")
        self.assertIs(raised.exception.remote_cleanup_confirmed, True)
        self.assertEqual(stage.parse_runtime_failure(b"password=unsafe\n"), (None, None))

    def test_bundle_validation_rejects_links_and_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as directory_name:
            directory = Path(directory_name)
            unsafe = directory / "unsafe.tar.gz"
            with tarfile.open(unsafe, "w:gz") as archive:
                link = tarfile.TarInfo(f"{stage.EXPECTED_BUNDLE_ROOT}/link")
                link.type = tarfile.SYMTYPE
                link.linkname = "/etc/passwd"
                archive.addfile(link)
            digest = hashlib.sha256(unsafe.read_bytes()).hexdigest()
            with self.assertRaises(ValueError):
                _ = stage.validate_bundle(str(unsafe), digest)

            traversal = directory / "traversal.tar.gz"
            with tarfile.open(traversal, "w:gz") as archive:
                member = tarfile.TarInfo(f"{stage.EXPECTED_BUNDLE_ROOT}/../escape")
                member.size = 1
                archive.addfile(member, io.BytesIO(b"x"))
            digest = hashlib.sha256(traversal.read_bytes()).hexdigest()
            with self.assertRaises(ValueError):
                _ = stage.validate_bundle(str(traversal), digest)

    def test_ssh_command_uses_strict_enrolled_host_key_and_no_secret(self) -> None:
        target = stage.RemoteTarget("device.invalid", 22, "root", Path("/private/known-hosts"))
        command = stage.ssh_command(target, 17, "true")
        rendered = " ".join(command)
        self.assertIn("StrictHostKeyChecking=yes", rendered)
        self.assertIn("UserKnownHostsFile=/private/known-hosts", rendered)
        self.assertNotIn("accept-new", rendered)
        self.assertNotIn("private-test-value", rendered)
        self.assertEqual(command[0:3], ["sshpass", "-d", "17"])

    def test_generated_remote_scripts_are_valid_posix_shell(self) -> None:
        paths = stage.stage_paths("c" * 64, "0123456789abcdef")
        bundle = stage.BundleInfo(Path("/bundle.tar.gz"), "c" * 64, 1024, "d" * 40)
        scripts = (
            stage.build_preflight_script(paths, 10_000_000, 18081),
            stage.build_upload_script(paths),
            stage.build_run_script(paths, bundle, 18081),
            stage.build_cleanup_script(paths),
            stage.build_final_script(paths, 18081),
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


if __name__ == "__main__":
    _ = unittest.main()
