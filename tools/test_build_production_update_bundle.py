#!/usr/bin/env python3
"""Tests for the production application-update bundle builder."""

from __future__ import annotations

import hashlib
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest

from tools import build_production_update_bundle as builder
from tools import stage_device_debian_runtime_package as package_gate


def package_archive(files: dict[str, tuple[int, bytes]]) -> bytes:
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        directories = {"."}
        for path in files:
            current = Path(".")
            for component in Path(path).parts[:-1]:
                current /= component
                directories.add(current.as_posix())
        for name in sorted(directories):
            info = tarfile.TarInfo(name)
            info.type = tarfile.DIRTYPE
            info.mode = 0o755
            info.uid = 0
            info.gid = 0
            info.mtime = 0
            archive.addfile(info)
        for name, (mode, content) in sorted(files.items()):
            info = tarfile.TarInfo(name)
            info.type = tarfile.REGTYPE
            info.mode = mode
            info.uid = 0
            info.gid = 0
            info.mtime = 0
            info.size = len(content)
            archive.addfile(info, io.BytesIO(content))
    return output.getvalue()


def package_info(files: dict[str, tuple[int, bytes]]) -> package_gate.PackageInfo:
    installed = {
        "/" + path: (hashlib.sha256(content).hexdigest(), mode)
        for path, (mode, content) in files.items()
    }
    return package_gate.PackageInfo(
        Path("/fixture.deb"),
        "a" * 64,
        1,
        "b" * 40,
        "c" * 64,
        "d" * 40,
        installed,
    )


def required_package_files() -> dict[str, tuple[int, bytes]]:
    files = {
        str(path): (0o755, str(path).encode("ascii"))
        for path in builder.RUNTIME_BINARY_MAPPING
    }
    files["usr/share/hardened-blikvm/web/index.html"] = (0o644, b"<html></html>")
    files["usr/share/hardened-blikvm/web/assets/app.js"] = (0o644, b"app")
    files["usr/share/doc/hardened-blikvm-runtime/README.md"] = (0o644, b"ignored")
    return files


class ProductionUpdateBundleBuilderTests(unittest.TestCase):
    def test_exact_runtime_payload_is_reverse_mapped_from_validated_package(self) -> None:
        package_files = required_package_files()
        payload = builder.extract_application_payload(
            package_archive(package_files), package_info(package_files)
        )
        self.assertEqual(
            set(payload),
            {
                *builder.REQUIRED_BUNDLE_PATHS,
                "share/web/assets/app.js",
            },
        )
        self.assertEqual(payload["bin/bli-kvm-server"][0], 0o755)
        self.assertEqual(payload["share/web/index.html"][0], 0o644)
        self.assertNotIn("README.md", "\n".join(payload))

    def test_incomplete_changed_or_duplicate_payload_is_rejected(self) -> None:
        package_files = required_package_files()
        incomplete = dict(package_files)
        del incomplete["usr/share/hardened-blikvm/web/index.html"]
        with self.assertRaises(ValueError):
            builder.extract_application_payload(
                package_archive(incomplete), package_info(incomplete)
            )

        changed_archive = dict(package_files)
        changed_archive["usr/share/hardened-blikvm/web/index.html"] = (
            0o644,
            b"changed",
        )
        with self.assertRaises(ValueError):
            builder.extract_application_payload(
                package_archive(changed_archive), package_info(package_files)
            )

        archive = io.BytesIO()
        with tarfile.open(fileobj=archive, mode="w", format=tarfile.USTAR_FORMAT) as tar:
            for _ in range(2):
                info = tarfile.TarInfo("usr/share/hardened-blikvm/web/index.html")
                info.mode = 0o644
                info.uid = 0
                info.gid = 0
                info.mtime = 0
                info.size = 1
                tar.addfile(info, io.BytesIO(b"x"))
        duplicate_info = package_info(
            {"usr/share/hardened-blikvm/web/index.html": (0o644, b"x")}
        )
        with self.assertRaisesRegex(ValueError, "duplicate"):
            builder.extract_application_payload(archive.getvalue(), duplicate_info)

    def test_manifest_and_tar_are_canonical_and_private_key_free(self) -> None:
        options = builder.Options(
            package="/package.deb",
            expected_package_sha256="a" * 64,
            expected_source_commit="b" * 40,
            expected_source_bundle_sha256="c" * 64,
            expected_builder_commit="d" * 40,
            enrollment_request="/request.json",
            expected_enrollment_request_sha256="e" * 64,
            key_id="app-production-test-1",
            version="0.1.0-dev2",
            sequence=1,
            minimum_system_sequence=1,
            output_dir="/tmp/output",
            acknowledge_unencrypted_host_key_risk=False,
        )
        enrollment = builder.Enrollment("app-production-test-1", bytes(range(32)), "e" * 64)
        files = {
            "bin/bli-kvm-server": (0o755, b"server"),
            "share/web/index.html": (0o644, b"index"),
        }
        manifest = builder.canonical_manifest(
            builder.manifest_document(options, enrollment, files)
        )
        decoded = json.loads(manifest)
        self.assertEqual(decoded["sequence"], 1)
        self.assertEqual(decoded["minimumSystemSequence"], 1)
        self.assertEqual([item["path"] for item in decoded["files"]], sorted(files))
        self.assertNotIn(b"private", manifest.lower())

        signature = bytes(range(64))
        bundle = builder.build_bundle(manifest, signature, files)
        with tarfile.open(fileobj=io.BytesIO(bundle), mode="r:") as archive:
            members = archive.getmembers()
            self.assertEqual([member.name for member in members], sorted(member.name for member in members))
            self.assertTrue(all(member.uid == 0 and member.gid == 0 for member in members))
            self.assertTrue(all(member.mtime == 0 for member in members))
            signature_stream = archive.extractfile("manifest.sig")
            self.assertIsNotNone(signature_stream)
            assert signature_stream is not None
            self.assertEqual(signature_stream.read(), signature)

    def test_enrollment_is_closed_and_hash_bound(self) -> None:
        public_key = bytes(range(1, 33))
        request = key_request = {
            "schemaVersion": 1,
            "product": builder.PRODUCT,
            "target": builder.TARGET,
            "generation": 1,
            "replacesKeyId": None,
            "keyId": "app-production-test-1",
            "publicKeyHex": public_key.hex(),
            "publicKeySha256": hashlib.sha256(public_key).hexdigest(),
        }
        raw = json.dumps(request, separators=(",", ":")).encode() + b"\n"
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "request.json"
            path.write_bytes(raw)
            path.chmod(0o600)
            loaded = builder.load_enrollment(
                str(path), hashlib.sha256(raw).hexdigest(), "app-production-test-1"
            )
            self.assertEqual(loaded.public_key, public_key)
            with self.assertRaises(ValueError):
                builder.load_enrollment(str(path), "0" * 64, "app-production-test-1")
            key_request["privateKey"] = "forbidden"
            path.write_text(json.dumps(key_request))
            with self.assertRaises(ValueError):
                builder.load_enrollment(
                    str(path), hashlib.sha256(path.read_bytes()).hexdigest(), "app-production-test-1"
                )

    def test_output_and_version_validation_are_bounded(self) -> None:
        self.assertEqual(builder.validate_version("0.1.0-dev2"), "0.1.0-dev2")
        for invalid in ("", ".1", "1.", "1..2", "1_bad"):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                builder.validate_version(invalid)
        with self.assertRaises(ValueError):
            builder.validate_output_directory("relative")

    def test_private_key_policy_requires_exact_passwordless_acknowledgement(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            self.assertEqual(
                builder.load_private_key_policy(directory, "app-production-test-1"),
                builder.key_ceremony.ENCRYPTED_PROTECTION,
            )
            policy = builder.key_ceremony.policy_document(
                "app-production-test-1",
                builder.key_ceremony.UNENCRYPTED_PROTECTION,
            )
            path = directory / builder.key_ceremony.POLICY_NAME
            path.write_bytes(policy)
            path.chmod(0o400)
            self.assertEqual(
                builder.load_private_key_policy(directory, "app-production-test-1"),
                builder.key_ceremony.UNENCRYPTED_PROTECTION,
            )
            decoded = json.loads(policy)
            decoded["hostRiskAcknowledged"] = False
            path.chmod(0o600)
            path.write_text(json.dumps(decoded))
            path.chmod(0o400)
            with self.assertRaises(ValueError):
                builder.load_private_key_policy(directory, "app-production-test-1")


if __name__ == "__main__":
    unittest.main()
