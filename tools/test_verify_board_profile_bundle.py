import hashlib
import io
import json
import subprocess
import sys
import tarfile
import tempfile
import unittest
from typing import cast
from pathlib import Path

import tools.verify_board_profile_bundle as verifier


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def build_bundle(root: Path, mutate: dict[str, object] | None = None) -> Path:
    mutate = mutate or {}
    root_value = mutate.get("root", verifier.EXPECTED_ROOT)
    profile = mutate.get("profile", verifier.EXPECTED_PROFILE)
    features = mutate.get("features", verifier.EXPECTED_FEATURES)
    acceptance = bool(mutate.get("acceptance", False))
    extra = mutate.get("extra", "")

    files: dict[str, bytes] = {}
    for member in verifier.REQUIRED_MEMBERS:
        if member in ("MANIFEST.json", "SHA256SUMS"):
            continue
        files[member] = b"content\n"
    if isinstance(extra, str) and extra:
        files[extra.lstrip("/")] = b"extra\n"

    manifest = {
        "schemaVersion": 1,
        "profile": profile,
        "cargoFeatures": features,
        "releaseAccepted": acceptance,
        "deviceAccepted": acceptance,
    }
    files["MANIFEST.json"] = json.dumps(manifest).encode()

    tampered = mutate.get("tamper")
    lines: list[str] = []
    for name, data in files.items():
        if mutate.get("omit_checksum") == name:
            continue
        recorded = sha256(data)
        if tampered == name:
            recorded = "0" * 64
        lines.append(f"{recorded}  ./{name}")
        if mutate.get("duplicate_checksum") == name:
            lines.append(f"{recorded}  ./{name}")
    files["SHA256SUMS"] = ("\n".join(sorted(lines)) + "\n").encode()

    bundle = root / "bundle.tar.gz"
    with tarfile.open(bundle, "w:gz") as archive:
        for name, data in files.items():
            info = tarfile.TarInfo(f"{root_value}/{name}")
            info.size = len(data)
            archive.addfile(info, io.BytesIO(data))
            if mutate.get("duplicate_member") == name:
                archive.addfile(info, io.BytesIO(data))
    return bundle


class VerifyBoardProfileBundleTests(unittest.TestCase):
    def test_incomplete_or_ambiguous_integrity_inventory_is_rejected(self) -> None:
        for mutation in ("omit_checksum", "duplicate_checksum", "duplicate_member"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory() as directory:
                bundle = build_bundle(
                    Path(directory), {mutation: "libexec/bli-kvm-server"}
                )
                with self.assertRaises(ValueError):
                    verifier.verify_bundle(bundle, "")

    def test_valid_bundle_verifies(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = build_bundle(Path(directory))
            receipt = verifier.verify_bundle(bundle, "")
            self.assertEqual(receipt["profile"], verifier.EXPECTED_PROFILE)
            self.assertEqual(receipt["cargoFeatures"], verifier.EXPECTED_FEATURES)
            count = cast(int, receipt["memberCount"])
            # SHA256SUMS does not list itself.
            self.assertGreaterEqual(count, len(verifier.REQUIRED_MEMBERS) - 1)

    def test_checksum_and_missing_member_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = build_bundle(Path(directory))
            with self.assertRaises(ValueError):
                verifier.verify_bundle(bundle, "0" * 64)

        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            with tarfile.open(path / "partial.tar.gz", "w:gz") as archive:
                info = tarfile.TarInfo(f"{verifier.EXPECTED_ROOT}/MANIFEST.json")
                payload = b"{}"
                info.size = len(payload)
                archive.addfile(info, io.BytesIO(payload))
            with self.assertRaises(ValueError):
                verifier.verify_bundle(path / "partial.tar.gz", "")

    def test_tampered_content_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = build_bundle(
                Path(directory), {"tamper": "libexec/bli-kvm-server"}
            )
            with self.assertRaises(ValueError):
                verifier.verify_bundle(bundle, "")

    def test_wrong_profile_features_and_acceptance_are_rejected(self) -> None:
        mutations: list[dict[str, object]] = [
            {"profile": "other"},
            {"features": ["restricted-control-surface"]},
            {"acceptance": True},
        ]
        for mutate in mutations:
            with tempfile.TemporaryDirectory() as directory:
                bundle = build_bundle(Path(directory), mutate)
                with self.subTest(mutate=mutate):
                    with self.assertRaises(ValueError):
                        verifier.verify_bundle(bundle, "")

    def test_unsafe_member_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            bundle = path / "unsafe.tar.gz"
            with tarfile.open(bundle, "w:gz") as archive:
                info = tarfile.TarInfo("../../escape")
                payload = b"x"
                info.size = len(payload)
                archive.addfile(info, io.BytesIO(payload))
            with self.assertRaises(ValueError):
                verifier.verify_bundle(bundle, "")

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/verify_board_profile_bundle.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
