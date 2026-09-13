from contextlib import ExitStack
import hashlib
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_write_contract as contract
from tools import persistent_write_evidence_provider as provider
from tools import persistent_write_evidence_store as storage


def binding(transaction: str = "a" * 24) -> contract.EvidenceBinding:
    return contract.EvidenceBinding(transaction, "b" * 64, contract.CONFIG_SHA256,
        "app-00000000000000000008-" + "c" * 64, 8, 40)


class ProviderTests(unittest.TestCase):
    def setUp(self) -> None:
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.stack.enter_context(patch.object(storage, "PARENT", Path(self.directory.name)))
        self.stack.enter_context(patch.object(storage, "OWNER_UID", os.getuid()))
        self.bound = binding()
        self.receipts = {gate: ("receipt-" + gate).encode() for gate in contract.REQUIRED_GATES}
        store = storage.Store.create(self.bound)
        gates = {gate: store.put(self.receipts[gate]) for gate in sorted(self.receipts)}
        self.seal = store.seal(gates)
        self.calls: list[tuple[str, bytes]] = []

        def accept(_binding: contract.EvidenceBinding, raw: bytes, _pins: object) -> None:
            self.calls.append(("accept", raw))

        self.validators = {gate: accept for gate in contract.REQUIRED_GATES}

    def read(self, **overrides: object) -> tuple[contract.GateEvidence, ...]:
        kwargs: dict[str, object] = {"expected_seal": self.seal, "pins": {"sample": "pins"},
                                     "validators": self.validators}
        kwargs.update(overrides)
        return provider.read_verified_evidence(Path("/var/lib/unused"), self.bound, **kwargs)  # type: ignore[arg-type]

    def test_sealed_receipts_are_validated_and_returned(self) -> None:
        evidence = self.read()
        self.assertEqual(frozenset(item.gate for item in evidence), contract.REQUIRED_GATES)
        self.assertEqual(len(self.calls), len(contract.REQUIRED_GATES))
        self.assertTrue(all(item.binding == self.bound and item.passed for item in evidence))
        self.assertEqual({item.gate: item.receipt_sha256 for item in evidence},
                         {gate: hashlib.sha256(raw).hexdigest() for gate, raw in self.receipts.items()})

    def test_missing_validator_fails_closed(self) -> None:
        validators = dict(self.validators)
        del validators["host"]
        with self.assertRaisesRegex(ValueError, "gate validator missing: host"):
            self.read(validators=validators)

    def test_wrong_operator_seal_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "seal differs"):
            self.read(expected_seal="f" * 64)

    def test_missing_store_is_rejected(self) -> None:
        other = binding(transaction="d" * 24)
        with self.assertRaisesRegex(ValueError, "sealed evidence store missing"):
            provider.read_verified_evidence(Path("/var/lib/unused"), other, expected_seal=self.seal,
                                            pins={}, validators=self.validators)

    def test_validator_failure_propagates(self) -> None:
        def reject(*_args: object) -> None:
            raise ValueError("receipt semantics differ")

        validators = {gate: reject for gate in contract.REQUIRED_GATES}
        with self.assertRaisesRegex(ValueError, "receipt semantics differ"):
            self.read(validators=validators)


if __name__ == "__main__":
    unittest.main()
