from __future__ import annotations

import hashlib
import importlib
import json
from pathlib import Path
import tarfile
import tempfile
from typing import Protocol, cast
import unittest

from tools.build_update_test_fixture import build_fixture, validate_new_output


class VerifyKeyProtocol(Protocol):
    def verify(self, signature: bytes, data: bytes) -> None: ...


class VerifyKeyFactoryProtocol(Protocol):
    def from_public_bytes(self, data: bytes) -> VerifyKeyProtocol: ...


class UpdateTestFixtureTests(unittest.TestCase):
    def test_real_server_payload_is_signed_only_for_first_two_slots(self) -> None:
        binary = b"\x7fELFbounded test bytes"
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "fixture"
            receipt = build_fixture(output, "2" * 40, binary)
            for artifact in receipt["artifacts"]:
                with tarfile.open(output / artifact["path"], mode="r:") as archive:
                    stream = archive.extractfile("payload/bin/bli-kvm-server")
                    manifest_stream = archive.extractfile("manifest.json")
                    assert stream is not None and manifest_stream is not None
                    payload_bytes = stream.read()
                    manifest = json.loads(manifest_stream.read())
                    entry = next(item for item in manifest["files"] if item["path"] == "bin/bli-kvm-server")
                    self.assertEqual(entry["sha256"], hashlib.sha256(payload_bytes).hexdigest())
                    if artifact["sequence"] < 3:
                        self.assertEqual(payload_bytes, binary)
                    else:
                        self.assertFalse(payload_bytes.startswith(b"\x7fELF"))

    def test_fixture_contains_only_public_metadata_and_three_valid_signatures(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "fixture"
            receipt = build_fixture(output, "1" * 40)
            self.assertEqual(receipt["schemaVersion"], 1)
            self.assertEqual(len(receipt["artifacts"]), 3)
            self.assertEqual(
                sorted(path.name for path in output.iterdir()),
                [
                    "application-1.tar",
                    "application-2.tar",
                    "application-3.tar",
                    "fixture.json",
                ],
            )
            self.assertFalse(any("private" in path.name for path in output.iterdir()))
            ed25519 = importlib.import_module(
                "cryptography.hazmat.primitives.asymmetric.ed25519"
            )
            factory = cast(
                VerifyKeyFactoryProtocol, getattr(ed25519, "Ed25519PublicKey")
            )
            public_key = factory.from_public_bytes(bytes.fromhex(receipt["publicKeyHex"]))
            for artifact in receipt["artifacts"]:
                path = output / artifact["path"]
                self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), artifact["sha256"])
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                with tarfile.open(path, mode="r:") as archive:
                    members = archive.getmembers()
                    names = [member.name.rstrip("/") for member in members]
                    self.assertEqual(names, sorted(names))
                    self.assertTrue(all(member.isfile() or member.isdir() for member in members))
                    manifest = archive.extractfile("manifest.json")
                    signature = archive.extractfile("manifest.sig")
                    assert manifest is not None and signature is not None
                    manifest_bytes = manifest.read()
                    public_key.verify(signature.read(), manifest_bytes)
                    decoded = json.loads(manifest_bytes)
                    self.assertEqual(decoded["keyId"], receipt["keyId"])
                    self.assertEqual(decoded["sequence"], artifact["sequence"])
                    self.assertEqual(decoded["sourceCommit"], "1" * 40)

    def test_output_must_be_new_absolute_and_below_an_approved_root(self) -> None:
        with self.assertRaises(ValueError):
            validate_new_output("relative/path")
        with self.assertRaises(ValueError):
            validate_new_output("/home/w0w/not-approved")
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(ValueError):
                validate_new_output(temporary)


if __name__ == "__main__":
    _ = unittest.main()
