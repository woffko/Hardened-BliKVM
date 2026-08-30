from __future__ import annotations

import hashlib
import importlib
import json
from pathlib import Path
import tarfile
import tempfile
from typing import Protocol, cast
import unittest

from tools.build_system_image_test_fixture import (
    BOOT_BYTES,
    ROOTFS_BYTES,
    build_fixture,
    validate_new_output,
)


class VerifyKeyProtocol(Protocol):
    def verify(self, signature: bytes, data: bytes) -> None: ...


class VerifyKeyFactoryProtocol(Protocol):
    def from_public_bytes(self, data: bytes) -> VerifyKeyProtocol: ...


class SystemImageTestFixtureTests(unittest.TestCase):
    def test_fixture_contains_only_public_metadata_and_valid_system_signatures(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "fixture"
            receipt = build_fixture(output, "2" * 40)
            self.assertEqual(receipt["schemaVersion"], 1)
            self.assertTrue(receipt["keyId"].startswith("sys-"))
            self.assertTrue(receipt["posixModesEnforced"])
            self.assertEqual(len(receipt["artifacts"]), 2)
            self.assertEqual(
                sorted(path.name for path in output.iterdir()),
                ["fixture.json", "system-rootfs-only.tar", "system-split-boot.tar"],
            )
            self.assertFalse(any("private" in path.name for path in output.iterdir()))

            ed25519 = importlib.import_module(
                "cryptography.hazmat.primitives.asymmetric.ed25519"
            )
            factory = cast(
                VerifyKeyFactoryProtocol, getattr(ed25519, "Ed25519PublicKey")
            )
            public_key = factory.from_public_bytes(
                bytes.fromhex(receipt["publicKeyHex"])
            )
            for artifact in receipt["artifacts"]:
                path = output / artifact["path"]
                self.assertEqual(path.stat().st_mode & 0o777, 0o600)
                self.assertEqual(
                    hashlib.sha256(path.read_bytes()).hexdigest(), artifact["sha256"]
                )
                with tarfile.open(path, mode="r:") as archive:
                    members = archive.getmembers()
                    names = [member.name.rstrip("/") for member in members]
                    self.assertEqual(names, sorted(names))
                    self.assertTrue(all(member.isfile() or member.isdir() for member in members))
                    self.assertTrue(
                        all(
                            member.uid == 0
                            and member.gid == 0
                            and member.mtime == 0
                            and member.mode == (0o755 if member.isdir() else 0o644)
                            for member in members
                        )
                    )
                    manifest_file = archive.extractfile("manifest.json")
                    signature_file = archive.extractfile("manifest.sig")
                    rootfs_file = archive.extractfile("payload/rootfs.img")
                    assert (
                        manifest_file is not None
                        and signature_file is not None
                        and rootfs_file is not None
                    )
                    manifest_bytes = manifest_file.read()
                    public_key.verify(signature_file.read(), manifest_bytes)
                    decoded = json.loads(manifest_bytes)
                    self.assertEqual(
                        json.dumps(decoded, ensure_ascii=True, separators=(",", ":")).encode(
                            "ascii"
                        ),
                        manifest_bytes,
                    )
                    self.assertEqual(decoded["format"], "hardened-blikvm-system-v1")
                    self.assertEqual(decoded["keyId"], receipt["keyId"])
                    self.assertEqual(decoded["layoutId"], artifact["layoutId"])
                    self.assertEqual(decoded["sourceCommit"], "2" * 40)
                    self.assertFalse(
                        {"device", "partition", "offset", "bootloader", "command"}
                        & decoded.keys()
                    )
                    rootfs = rootfs_file.read()
                    self.assertEqual(len(rootfs), ROOTFS_BYTES)
                    self.assertEqual(rootfs[1080:1082], b"\x53\xef")
                    boot_member = next(
                        (
                            member
                            for member in members
                            if member.name == "payload/boot.img"
                        ),
                        None,
                    )
                    if artifact["bootBytes"] is None:
                        self.assertIsNone(boot_member)
                    else:
                        assert boot_member is not None
                        boot_file = archive.extractfile(boot_member)
                        assert boot_file is not None
                        boot = boot_file.read()
                        self.assertEqual(len(boot), BOOT_BYTES)
                        self.assertEqual(boot[510:512], b"\x55\xaa")

    def test_output_and_source_commit_are_strict(self) -> None:
        with self.assertRaises(ValueError):
            validate_new_output("relative/path")
        with self.assertRaises(ValueError):
            validate_new_output("/home/w0w/not-approved")
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaises(ValueError):
                validate_new_output(temporary)


if __name__ == "__main__":
    _ = unittest.main()
