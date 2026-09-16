import hashlib
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

from tools import stage_device_board_bundle as upload


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def facts(**overrides: str) -> dict[str, str]:
    base = {
        "aarch64": "yes",
        "bundle_bytes": "1048576",
        "bundle_sha256": "a" * 64,
        "bundle_present": "yes",
        "part_absent": "yes",
    }
    base.update(overrides)
    return base


class BoardBundleUploadTests(unittest.TestCase):
    def test_upload_script_is_shell_valid_and_atomic(self) -> None:
        script = upload.build_upload_script("a" * 64)
        syntax = subprocess.run(
            ["sh", "-n"],
            input=script.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        part = script.index("part=${target}.part")
        digest = script.index("sha256sum")
        move = script.index('mv "$part" "$target"')
        self.assertLess(part, digest)
        self.assertLess(digest, move)
        self.assertIn(f"root={upload.GATE_ROOT}", script)
        self.assertIn(upload.TARGET_PATH, script)
        for forbidden in ("password", "sshpass", "/home/", "10.0."):
            self.assertNotIn(forbidden, script)

    def test_upload_script_rejects_an_invalid_expected_hash(self) -> None:
        for bad in ("", "0" * 63, "z" * 64, "A" * 64):
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    upload.build_upload_script(bad)

    def test_fact_validation_accepts_a_good_upload_and_rejects_mismatches(self) -> None:
        upload.validate_upload(facts(), "a" * 64, 1048576)
        for override in (
            {"aarch64": "no"},
            {"bundle_present": "no"},
            {"part_absent": "no"},
            {"bundle_sha256": "b" * 64},
            {"bundle_bytes": "1"},
        ):
            with self.subTest(override=override):
                with self.assertRaises(ValueError):
                    upload.validate_upload(facts(**override), "a" * 64, 1048576)

    def test_local_bundle_validation_rejects_unsafe_inputs(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            payload = b"x" * (1024 * 1024)
            bundle = root / "bundle.tar.gz"
            bundle.write_bytes(payload)
            digest = sha256(payload)
            with self.assertRaises(ValueError):
                upload.validate_local_bundle(bundle, "a" * 64)
            small = root / "small.tar.gz"
            small.write_bytes(b"x" * 16)
            with self.assertRaises(ValueError):
                upload.validate_local_bundle(small, sha256(b"x" * 16))
            link = root / "link.tar.gz"
            link.symlink_to(bundle)
            with self.assertRaises(ValueError):
                upload.validate_local_bundle(link, digest)

    def test_a_real_bundle_is_verified_before_any_transfer(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "bundle.tar.gz"
            bundle.write_bytes(b"y" * (1024 * 1024))
            digest = sha256(bundle.read_bytes())
            # The profile verifier rejects an archive that is not the reviewed
            # bundle, so the transfer refuses before contacting a device.
            with self.assertRaises(ValueError):
                upload.validate_local_bundle(bundle, digest)


if __name__ == "__main__":
    unittest.main()
