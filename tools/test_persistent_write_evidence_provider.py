from contextlib import ExitStack
import copy
import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from typing import cast
from unittest.mock import patch

# pyright: reportPrivateUsage=false

from tools import persistent_write_contract as contract
from tools import persistent_write_evidence_pins as pins
from tools import persistent_write_evidence_provider as provider
from tools import persistent_write_evidence_store as storage
from tools import test_persistent_write_component_view as plan_fixture
from tools import test_persistent_write_gui_failure_receipt as gui_fixture


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


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

        def accept(_root: Path, _binding: contract.EvidenceBinding, raw: bytes, _pins: object) -> None:
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

    def test_default_registry_covers_all_required_gates(self) -> None:
        self.assertEqual(set(provider.VALIDATORS), contract.REQUIRED_GATES)

    def test_default_gui_validators_accept_a_pinned_receipt(self) -> None:
        value = gui_fixture.document()
        value["runnerSha256"] = pins.CURRENT_PINS["runner"]
        value["browserSourceHashes"] = pins.CURRENT_PINS["failure_browser_sources"]
        receipts = {gate: gui_fixture.raw(value) for gate in ("gui_failure_retry", "gui_cancel", "gui_logout")}
        for gate in contract.REQUIRED_GATES - set(receipts):
            receipts[gate] = ("placeholder-" + gate).encode()

        def accept(_root: Path, _binding: contract.EvidenceBinding, _raw: bytes, _pins: object) -> None:
            return None

        binding = contract.EvidenceBinding("e" * 24, "f" * 64, contract.CONFIG_SHA256, gui_fixture.SLOT, 10, 46)
        store = storage.Store.create(binding)
        gates = {gate: store.put(receipts[gate]) for gate in sorted(receipts)}
        seal = store.seal(gates)
        validators: dict[str, provider.Validator] = {gate: accept for gate in contract.REQUIRED_GATES}
        for gate in ("gui_failure_retry", "gui_cancel", "gui_logout"):
            validators[gate] = provider.VALIDATORS[gate]
        evidence = provider.read_verified_evidence(Path("/var/lib/unused"), binding, expected_seal=seal,
                                                   pins=pins.CURRENT_PINS, validators=validators)
        self.assertEqual(frozenset(item.gate for item in evidence), contract.REQUIRED_GATES)



    def archived_plan(self, parent: Path, transaction: str, document: dict[str, object]) -> Path:
        archive = parent / (provider.ARCHIVE_PREFIX + transaction)
        archive.mkdir(mode=0o700)
        path = archive / "plan.json"
        path.write_bytes(json.dumps(document).encode())
        path.chmod(0o600)
        return path

    def test_component_views_require_private_root_plans(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            self.archived_plan(parent, "2" * 24, plan_fixture.plan())
            guard = parent / (contract.PREFIX + "3" * 24)
            guard.mkdir(mode=0o700)
            guard_plan = guard / "plan.json"
            guard_plan.write_bytes(json.dumps(plan_fixture.plan()).encode())
            guard_plan.chmod(0o600)
            with patch.object(provider, "ARCHIVE_PARENT", parent), \
                    patch.object(provider, "PLAN_OWNER_UID", os.getuid()):
                source = provider.source_components("2" * 24)
                self.assertEqual(source, provider.current_components(guard))
                with self.assertRaises(OSError):
                    provider.source_components("4" * 24)
                guard_plan.chmod(0o644)
                with self.assertRaises(ValueError):
                    provider.current_components(guard)

    def test_historical_check_rejects_component_drift(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            parent = Path(directory)
            source_txn = "2" * 24
            self.archived_plan(parent, source_txn, plan_fixture.plan())
            guard = parent / (contract.PREFIX + "3" * 24)
            guard.mkdir(mode=0o700)
            current_plan = plan_fixture.plan()
            guard_plan = guard / "plan.json"
            guard_plan.write_bytes(json.dumps(current_plan).encode())
            guard_plan.chmod(0o600)
            binding = contract.EvidenceBinding("3" * 24, "b" * 64, contract.CONFIG_SHA256,
                gui_fixture.SLOT, 10, 46)
            source_binding = contract.EvidenceBinding(source_txn, "b" * 64, contract.CONFIG_SHA256,
                gui_fixture.SLOT, 10, 46)
            terminal = (binding.baseline_slot, 10, 46)
            with patch.object(provider, "ARCHIVE_PARENT", parent), \
                    patch.object(provider, "PLAN_OWNER_UID", os.getuid()):
                provider._require_historical("boot_restore", binding, guard, source_binding, terminal, "d" * 64)
                drifted = copy.deepcopy(current_plan)
                section(section(drifted, "before"), "runtime")["trust"] = "9" * 64
                guard_plan.write_bytes(json.dumps(drifted).encode())
                guard_plan.chmod(0o600)
                with self.assertRaises(ValueError):
                    provider._require_historical("boot_restore", binding, guard, source_binding, terminal, "d" * 64)


if __name__ == "__main__":
    unittest.main()
