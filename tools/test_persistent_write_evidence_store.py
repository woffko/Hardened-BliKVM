from dataclasses import replace
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_write_evidence_store as storage


class StoreTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.parent = Path(temporary.name)
        parent = patch.object(storage, 'PARENT', self.parent)
        owner = patch.object(storage, 'OWNER_UID', os.geteuid())
        parent.start(); owner.start()
        self.addCleanup(parent.stop); self.addCleanup(owner.stop)
        self.binding = storage.contract.EvidenceBinding('a' * 24, 'b' * 64, storage.contract.CONFIG_SHA256,
            'app-00000000000000000009-' + 'c' * 64, 9, 43)
        self.store = storage.Store.create(self.binding)

    def test_original_bytes_idempotency_and_sealing(self) -> None:
        raw = b'{ "original": true }\n'
        digest = self.store.put(raw)
        self.assertEqual(digest, hashlib.sha256(raw).hexdigest())
        self.assertEqual(self.store.put(raw), digest)
        self.assertEqual(self.store.get(digest), raw)
        gates = {name: digest for name in storage.contract.REQUIRED_GATES}
        seal = self.store.seal(gates)
        self.assertEqual(self.store.seal(gates), seal)
        self.store.verify_seal(gates, seal)
        with self.assertRaises(ValueError): self.store.put(b'new')

    def test_verification_never_creates_seal(self) -> None:
        digest = self.store.put(b'original')
        gates = {name: digest for name in storage.contract.REQUIRED_GATES}
        with self.assertRaises(FileNotFoundError): self.store.verify_seal(gates, 'd' * 64)
        self.assertFalse((self.store.root / 'seal.json').exists())

    def test_foreign_binding_and_namespace_are_rejected(self) -> None:
        foreign = storage.Store(replace(self.binding, baseline_generation=44))
        with self.assertRaises(ValueError): foreign.put(b'x')
        with self.assertRaises(FileExistsError): storage.Store.create(self.binding)
        (self.store.root / 'foreign').write_bytes(b'user data')
        with self.assertRaises(ValueError): self.store.put(b'x')
        self.assertEqual((self.store.root / 'foreign').read_bytes(), b'user data')

    def test_partial_corrupt_symlink_and_hardlink_objects_fail_closed(self) -> None:
        raw = b'complete source'
        digest = hashlib.sha256(raw).hexdigest()
        path = self.store.root / (digest + '.json')
        path.write_bytes(b'partial'); path.chmod(0o400)
        with self.assertRaises(ValueError): self.store.put(raw)
        self.assertEqual(path.read_bytes(), b'partial')
        path.unlink()
        outside = self.parent / 'outside'; outside.write_bytes(raw); outside.chmod(0o400)
        path.symlink_to(outside)
        with self.assertRaises(OSError): self.store.get(digest)
        path.unlink(); os.link(outside, path)
        with self.assertRaises(ValueError): self.store.get(digest)

    def test_missing_references_changed_seal_and_quota_fail(self) -> None:
        with self.assertRaises(ValueError): self.store.seal({})
        with self.assertRaises(FileNotFoundError): self.store.seal({name: 'f' * 64 for name in storage.contract.REQUIRED_GATES})
        with patch.object(storage, 'MAX_OBJECTS', 1):
            digest = self.store.put(b'first')
            with self.assertRaises(ValueError): self.store.put(b'second')
        gates = {name: digest for name in storage.contract.REQUIRED_GATES}
        seal = self.store.seal(gates)
        with self.assertRaises(ValueError): self.store.verify_seal(gates, '0' * 64)
        self.assertEqual(self.store.seal(gates), seal)

    def test_sync_failure_does_not_seal_or_overwrite_receipt(self) -> None:
        raw = b'original'
        digest = hashlib.sha256(raw).hexdigest()
        with patch.object(storage.os, 'fsync', side_effect=OSError('injected sync failure')), self.assertRaises(OSError):
            self.store.put(raw)
        self.assertFalse((self.store.root / 'seal.json').exists())
        self.assertEqual((self.store.root / (digest + '.json')).read_bytes(), raw)
        self.assertEqual(self.store.put(raw), digest)

    def test_restrictive_umask_still_produces_exact_private_modes(self) -> None:
        previous = os.umask(0o777)
        try:
            store = storage.Store.create(replace(self.binding, transaction_id='b' * 24))
            digest = store.put(b'original')
            self.assertEqual(store.root.stat().st_mode & 0o777, 0o700)
            self.assertEqual((store.root / (digest + '.json')).stat().st_mode & 0o777, 0o400)
        finally:
            os.umask(previous)

    def test_partial_seal_is_rejected_and_preserved(self) -> None:
        digest = self.store.put(b'original')
        gates = {name: digest for name in storage.contract.REQUIRED_GATES}
        path = self.store.root / 'seal.json'
        path.write_bytes(b'{"schemaVersion":'); path.chmod(0o400)
        with self.assertRaises(ValueError): self.store.seal(gates)
        self.assertEqual(path.read_bytes(), b'{"schemaVersion":')


if __name__ == '__main__':
    unittest.main()
