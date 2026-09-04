#!/usr/bin/env python3
"""Tests for the production application-update signing-key generator."""

from __future__ import annotations

import hashlib
import importlib.util
import json
from pathlib import Path
import stat
import tempfile
import unittest
from typing import Protocol, cast


class GeneratorModule(Protocol):
    SPKI_PREFIX: bytes

    def validate_key_id(self, value: str) -> str: ...

    def raw_public_key(self, public_der: bytes) -> bytes: ...

    def public_pem(self, public_der: bytes) -> bytes: ...

    def enrollment_request(self, key_id: str, public_key: bytes) -> bytes: ...

    def review_document(self, key_id: str, public_key: bytes, request: bytes) -> bytes: ...

    def create_new_file(self, path: Path, content: bytes, mode: int) -> None: ...


def load_generator() -> GeneratorModule:
    path = Path(__file__).with_name("generate_production_update_signing_key.py")
    spec = importlib.util.spec_from_file_location("production_key_generator", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("generator module could not be loaded")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return cast(GeneratorModule, module)


GENERATOR = load_generator()


class ProductionUpdateSigningKeyGeneratorTests(unittest.TestCase):
    def test_public_request_is_canonical_and_matches_contract(self) -> None:
        public_key = bytes(range(1, 33))
        request = GENERATOR.enrollment_request("app-production-2026-09-01", public_key)
        self.assertTrue(request.endswith(b"\n"))
        value = json.loads(request)
        self.assertEqual(
            value,
            {
                "schemaVersion": 1,
                "product": "Hardened BliKVM",
                "target": "bli-h616-pikvm",
                "generation": 1,
                "replacesKeyId": None,
                "keyId": "app-production-2026-09-01",
                "publicKeyHex": public_key.hex(),
                "publicKeySha256": hashlib.sha256(public_key).hexdigest(),
            },
        )
        self.assertNotIn(b"private", request.lower())

    def test_ed25519_spki_is_strictly_decoded_and_rendered(self) -> None:
        public_key = bytes(range(32))
        public_der = GENERATOR.SPKI_PREFIX + public_key
        self.assertEqual(GENERATOR.raw_public_key(public_der), public_key)
        pem = GENERATOR.public_pem(public_der)
        self.assertTrue(pem.startswith(b"-----BEGIN PUBLIC KEY-----\n"))
        self.assertTrue(pem.endswith(b"-----END PUBLIC KEY-----\n"))
        with self.assertRaises(ValueError):
            GENERATOR.raw_public_key(public_der[1:])
        with self.assertRaises(ValueError):
            GENERATOR.raw_public_key(GENERATOR.SPKI_PREFIX + bytes(32))

    def test_key_identifier_matches_application_verifier_rules(self) -> None:
        self.assertEqual(
            GENERATOR.validate_key_id("app-production-2026-09-01"),
            "app-production-2026-09-01",
        )
        for invalid in ("prod-2026", "app-invalid key", "app-a..b", "app-end."):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                GENERATOR.validate_key_id(invalid)

    def test_output_creation_is_exclusive_and_owner_only(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "public-artifact"
            GENERATOR.create_new_file(path, b"public\n", 0o400)
            self.assertEqual(path.read_bytes(), b"public\n")
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o400)
            with self.assertRaises(FileExistsError):
                GENERATOR.create_new_file(path, b"replacement\n", 0o400)

    def test_review_document_contains_only_public_metadata(self) -> None:
        public_key = bytes(range(1, 33))
        request = GENERATOR.enrollment_request("app-production-2026-09-01", public_key)
        review = GENERATOR.review_document("app-production-2026-09-01", public_key, request)
        self.assertIn(hashlib.sha256(public_key).hexdigest().encode(), review)
        self.assertIn(hashlib.sha256(request).hexdigest().encode(), review)
        self.assertNotIn(public_key, review)


if __name__ == "__main__":
    unittest.main()
