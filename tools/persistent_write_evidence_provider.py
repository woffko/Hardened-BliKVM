"""Privileged evidence provider: sealed custody plus pinned semantic gates.

Reading never approves retention; the commit backend requires the explicit
operator approval separately. Missing validators, missing store objects or a
seal mismatch fail closed before any GateEvidence is constructed.
"""

from collections.abc import Callable, Mapping
from dataclasses import asdict
import json
from pathlib import Path
import re

if not __package__:
    import persistent_write_contract as contract
    import persistent_write_evidence_pins as pins_module
    import persistent_write_evidence_store as storage
else:
    from tools import persistent_write_contract as contract
    from tools import persistent_write_evidence_pins as pins_module
    from tools import persistent_write_evidence_store as storage

require = contract.temporary.require
Validator = Callable[[contract.EvidenceBinding, bytes, Mapping[str, object]], None]
VALIDATORS: dict[str, Validator] = {}


def unique(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, "duplicate evidence seal field")
        result[key] = value
    return result


def read_gate_map(raw: bytes, binding: contract.EvidenceBinding) -> dict[str, str]:
    require(0 < len(raw) <= storage.MAX_BYTES, "evidence seal size outside bounds")
    value = json.loads(raw, object_pairs_hook=unique)
    require(isinstance(value, dict), "invalid evidence seal object")
    require(set(value) == {"schemaVersion", "binding", "gates"} and type(value["schemaVersion"]) is int
            and value["schemaVersion"] == 1, "invalid evidence seal schema")
    require(value["binding"] == asdict(binding), "evidence seal binding differs")
    gates = value["gates"]
    require(isinstance(gates, dict), "invalid evidence gate map")
    result: dict[str, str] = {}
    for name, digest in gates.items():
        require(name in contract.REQUIRED_GATES and isinstance(digest, str)
                and re.fullmatch(r"[a-f0-9]{64}", digest) is not None, "invalid evidence gate reference")
        result[name] = digest
    require(set(result) == set(contract.REQUIRED_GATES), "complete evidence gate map required")
    return result


def read_verified_evidence(root: Path, binding: contract.EvidenceBinding, *, expected_seal: str,
                           pins: Mapping[str, object] | None = None,
                           validators: Mapping[str, Validator] | None = None) -> tuple[contract.GateEvidence, ...]:
    """Return the nine GateEvidence records for a sealed current transaction.

    The caller must hold the common controller lock. `expected_seal` is the
    operator anchor printed by the native evidence action; it is never proof by
    itself and never replaces semantic validation of the original bytes.
    """
    _ = root  # Historical archive lookups arrive with their validators.
    registry = dict(VALIDATORS if validators is None else validators)
    resolved_pins = pins_module.CURRENT_PINS if pins is None else pins
    binding.validate()
    require(re.fullmatch(r"[a-f0-9]{64}", expected_seal) is not None, "invalid expected evidence seal")
    store = storage.Store(binding)
    try:
        raw_seal = store.read("seal.json")
    except FileNotFoundError:
        raise ValueError("sealed evidence store missing") from None
    gates = read_gate_map(raw_seal, binding)
    store.verify_seal(gates, expected_seal)
    evidence: list[contract.GateEvidence] = []
    for gate in sorted(contract.REQUIRED_GATES):
        validator = registry.get(gate)
        require(validator is not None, "gate validator missing: " + gate)
        assert validator is not None
        raw = store.get(gates[gate])
        validator(binding, raw, resolved_pins)
        evidence.append(contract.GateEvidence(gate, binding, gates[gate], True))
    return tuple(evidence)
