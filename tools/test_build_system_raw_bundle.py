"""Host tests for the RAW system bundle builder.

The Rust test `the_bundle_builder_output_stages_and_writes` in
`crates/bli-kvm-system-raw-update` stages and writes the builder's output
with the real verifier; these tests cover the builder's own checks.
"""

from __future__ import annotations

import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_system_raw_bundle as builder


TOOLS = ("openssl", "zstd", "mke2fs", "e2fsck", "resize2fs", "dumpe2fs", "debugfs")


@unittest.skipUnless(all(shutil.which(tool) for tool in TOOLS), "image tools missing")
class BuildSystemRawBundleTest(unittest.TestCase):
    def setUp(self) -> None:
        self.work = Path(tempfile.mkdtemp(prefix="raw-bundle-test."))
        self.key = self.work / "key.pem"
        subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ed25519", "-out", str(self.key)],
            check=True,
            capture_output=True,
        )
        self.trust = self.work / "trust.json"
        self.trust.write_text(
            json.dumps(
                {
                    "keyId": "sys-test-builder",
                    "publicKey": builder.public_key_hex(self.key),
                    "layoutId": "layout-test-v1",
                }
            )
        )
        self.rootfs = self.make_root("2\n")

    def make_root(self, sequence: str | None, name: str = "root.ext4") -> Path:
        """An ext4 root whose system sequence file says `sequence`."""
        source = self.work / f"{name}.d"
        if sequence is not None:
            directory = source / "usr/share/hardened-blikvm/system-update"
            directory.mkdir(parents=True)
            (directory / "system-sequence").write_text(sequence)
        else:
            source.mkdir()
        rootfs = self.work / name
        subprocess.run(
            ["mke2fs", "-q", "-t", "ext4", "-b", "4096", "-d", str(source), str(rootfs), "16M"],
            check=True,
            capture_output=True,
        )
        return rootfs

    def tearDown(self) -> None:
        shutil.rmtree(self.work)

    def build(self, **overrides: object) -> Path:
        arguments: dict[str, object] = {
            "rootfs": self.rootfs,
            "private_key": self.key,
            "trust": self.trust,
            "version": "1.0.0",
            "sequence": 2,
            "source_commit": "a" * 40,
            "output_dir": self.work / "out",
            "compression_level": 1,
        }
        arguments.update(overrides)
        return builder.build(**arguments)  # type: ignore[arg-type]

    def test_the_bundle_has_the_verifier_layout_and_a_valid_signature(self) -> None:
        bundle = self.build()
        self.assertEqual(bundle.name, "hardened-blikvm-system-1.0.0-seq2.tar")
        with tarfile.open(bundle) as archive:
            members = archive.getmembers()
            self.assertEqual(
                [member.name for member in members],
                ["manifest.json", "manifest.sig", "payload", "payload/rootfs.img.zst"],
            )
            for member in members:
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                self.assertEqual(member.mode, 0o755 if member.isdir() else 0o644)
            manifest_bytes = archive.extractfile("manifest.json").read()  # type: ignore[union-attr]
            signature = archive.extractfile("manifest.sig").read()  # type: ignore[union-attr]
            payload = archive.extractfile("payload/rootfs.img.zst").read()  # type: ignore[union-attr]
        manifest = json.loads(manifest_bytes)
        self.assertEqual(
            json.dumps(manifest, separators=(",", ":")).encode(), manifest_bytes
        )
        self.assertEqual(list(manifest), [
            "format", "product", "target", "architecture", "keyId", "version",
            "sequence", "minimumRecoverySequence", "minimumBootControlSequence",
            "layoutId", "sourceCommit", "components",
        ])
        component = manifest["components"][0]
        self.assertEqual(component["contentType"], "ext4_zstd")
        self.assertEqual(component["size"], len(payload))
        self.assertTrue(payload.startswith(bytes.fromhex("28b52ffd")))
        self.assertEqual(component["imageSize"] % 4096, 0)
        self.assertLess(component["imageSize"], 16 * 1024 * 1024)
        self.assertEqual(manifest["layoutId"], "layout-test-v1")
        (self.work / "m.json").write_bytes(manifest_bytes)
        (self.work / "m.sig").write_bytes(signature)
        (self.work / "pub.pem").write_bytes(
            subprocess.run(
                ["openssl", "pkey", "-in", str(self.key), "-pubout"],
                check=True,
                capture_output=True,
            ).stdout
        )
        verified = subprocess.run(
            [
                "openssl", "pkeyutl", "-verify", "-rawin", "-pubin",
                "-inkey", str(self.work / "pub.pem"),
                "-in", str(self.work / "m.json"), "-sigfile", str(self.work / "m.sig"),
            ],
            capture_output=True,
        )
        self.assertEqual(verified.returncode, 0, verified.stdout + verified.stderr)
        # The source image is left untouched.
        self.assertEqual(self.rootfs.stat().st_size, 16 * 1024 * 1024)

    def test_a_key_that_does_not_match_the_trust_file_is_refused(self) -> None:
        other = self.work / "other.pem"
        subprocess.run(
            ["openssl", "genpkey", "-algorithm", "ed25519", "-out", str(other)],
            check=True,
            capture_output=True,
        )
        with self.assertRaises(builder.BuildError):
            self.build(private_key=other)

    def test_the_root_must_name_the_bundles_sequence(self) -> None:
        # The root says which system it is, so a card written from its image
        # takes no older bundle (security review of 2026-10-18, MED-07).
        for sequence, name in (("3\n", "other.ext4"), (None, "none.ext4"), ("x\n", "junk.ext4")):
            with self.subTest(sequence=sequence), self.assertRaises(builder.BuildError):
                self.build(rootfs=self.make_root(sequence, name))
        self.assertTrue(self.build().exists())

    def test_invalid_versions_commits_and_sequences_are_refused(self) -> None:
        for overrides in (
            {"version": "one"},
            {"source_commit": "A" * 40},
            {"source_commit": "a" * 39},
            {"sequence": 0},
        ):
            with self.subTest(overrides=overrides), self.assertRaises(builder.BuildError):
                self.build(**overrides)


if __name__ == "__main__":
    unittest.main()
