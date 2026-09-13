"""Privileged evidence provider: sealed custody plus pinned semantic gates.

Reading never approves retention; the commit backend requires the explicit
operator approval separately. Missing validators, missing store objects or a
seal mismatch fail closed before any GateEvidence is constructed.
"""

from collections.abc import Callable, Mapping
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import re
import stat
from typing import cast

if not __package__:
    import persistent_write_activation_receipt as activation_receipt
    import persistent_write_evidence_compatibility as compatibility
    import persistent_write_component_view as component_view
    import persistent_write_contract as contract
    import persistent_write_evidence_pins as pins_module
    import persistent_write_evidence_store as storage
    import persistent_write_gui_gate_receipt as gui_gate
    import persistent_write_host_receipt as host_receipt
    import persistent_write_recovery_receipt as recovery_receipt
    import persistent_write_upload_receipt as upload_receipt
else:
    from tools import persistent_write_activation_receipt as activation_receipt
    from tools import persistent_write_evidence_compatibility as compatibility
    from tools import persistent_write_component_view as component_view
    from tools import persistent_write_contract as contract
    from tools import persistent_write_evidence_pins as pins_module
    from tools import persistent_write_evidence_store as storage
    from tools import persistent_write_gui_gate_receipt as gui_gate
    from tools import persistent_write_host_receipt as host_receipt
    from tools import persistent_write_recovery_receipt as recovery_receipt
    from tools import persistent_write_upload_receipt as upload_receipt

require = contract.temporary.require
Validator = Callable[[Path, contract.EvidenceBinding, bytes, Mapping[str, object]], None]
ARCHIVE_PREFIX = "hardened-blikvm-write-restored-"
ARCHIVE_PARENT = Path("/var/lib")
PLAN_OWNER_UID = 0
RECOVERY_ACTIONS = {"boot_restore": "boot", "disable_drain": "pending"}


def _gui_failure_validator(gate: str) -> Validator:
    def validate(_root: Path, binding: contract.EvidenceBinding, raw: bytes, pins: Mapping[str, object]) -> None:
        sources, tool = pins.get("failure_browser_sources"), pins.get("gui_tool")
        require(isinstance(sources, dict) and isinstance(tool, str), "GUI gate pins missing")
        assert isinstance(tool, str)
        contents = gui_gate.parse(raw, expected_sources=cast(Mapping[str, str], sources), expected_tool=tool)
        require(contents.binding == binding, "GUI gate receipt belongs to another transaction")
        gui_gate.validate_gate(gate, contents)
    return validate


def _host_validator(_root: Path, binding: contract.EvidenceBinding, raw: bytes, pins: Mapping[str, object]) -> None:
    sources = pins.get("host_sources")
    tool = pins.get("host_tool")
    require(isinstance(sources, dict) and isinstance(tool, str), "host gate pins missing")
    assert isinstance(tool, str)
    contents = host_receipt.parse(raw, expected_python_sources=cast(Mapping[str, str], sources),
                                  expected_tool=tool)
    require(contents.binding == binding, "host gate receipt belongs to another transaction")


VALIDATORS: dict[str, Validator] = {gate: _gui_failure_validator(gate)
                                    for gate in ("gui_failure_retry", "gui_cancel", "gui_logout")}
VALIDATORS["host"] = _host_validator


def _upload_validator(_root: Path, binding: contract.EvidenceBinding, raw: bytes,
                      pins: Mapping[str, object]) -> None:
    sources = pins.get("upload_sources")
    tool = pins.get("upload_tool")
    require(isinstance(sources, dict) and isinstance(tool, str), "upload gate pins missing")
    assert isinstance(tool, str)
    contents = upload_receipt.parse(raw, expected_sources=cast(Mapping[str, str], sources),
                                    expected_tool=tool)
    require(contents.binding == binding, "upload gate receipt belongs to another transaction")


VALIDATORS["native_upload"] = _upload_validator


def _private_plan(path: Path) -> dict[str, object]:
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == PLAN_OWNER_UID and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and 0 < meta.st_size <= 4 * 1024 * 1024,
            "unsafe transaction plan")
    return component_view.read_plan(path.read_bytes())


def _harness_component() -> str:
    return hashlib.sha256(json.dumps(pins_module.CURRENT_PINS, sort_keys=True,
                                     separators=(",", ":")).encode()).hexdigest()


def current_components(root: Path) -> dict[str, str]:
    plan = _private_plan(root / "plan.json")
    return component_view.component_view(plan, test_harness=_harness_component())


def source_components(transaction: object) -> dict[str, str]:
    require(isinstance(transaction, str) and re.fullmatch(r"[a-f0-9]{24}", transaction) is not None,
            "invalid source transaction")
    assert isinstance(transaction, str)
    archive = ARCHIVE_PARENT / (ARCHIVE_PREFIX + transaction)
    plan = _private_plan(archive / "plan.json")
    return component_view.component_view(plan, test_harness=_harness_component())


def _require_historical(gate: str, current: contract.EvidenceBinding, root: Path,
                        source: contract.EvidenceBinding, terminal: tuple[str, int, int],
                        receipt_sha256: str) -> None:
    slot, floor, generation = terminal
    record = compatibility.Source(gate, source, slot, floor, generation, receipt_sha256,
                                  source_components(source.transaction_id))
    compatibility.check(record, current, current_components(root))


def _activation_validator(gate: str) -> Validator:
    def validate(root: Path, binding: contract.EvidenceBinding, raw: bytes,
                 _pins: Mapping[str, object]) -> None:
        historical = pins_module.HISTORICAL_PINS.get("native_activation")
        require(isinstance(historical, dict), "historical activation pins missing")
        entry = cast(Mapping[str, object], historical)
        runner, sources = entry.get("runner"), entry.get("browser_sources")
        require(isinstance(runner, str) and isinstance(sources, dict), "invalid historical activation pins")
        assert isinstance(runner, str)
        contents = activation_receipt.parse(raw, expected_runner=runner,
                                            expected_browser_sources=cast(Mapping[str, str], sources))
        _require_historical(gate, binding, root, contents.source_binding,
                            (contents.terminal_slot, contents.terminal_floor, contents.terminal_generation),
                            contents.receipt_sha256)
    return validate


def _recovery_validator(gate: str) -> Validator:
    action = RECOVERY_ACTIONS[gate]

    def validate(root: Path, binding: contract.EvidenceBinding, raw: bytes,
                 pins: Mapping[str, object]) -> None:
        runner, probes = pins.get("runner"), pins.get("probe_sources")
        require(isinstance(runner, str) and isinstance(probes, dict), "recovery pins missing")
        probe_entry = cast(Mapping[str, object], probes).get(action)
        require(isinstance(probe_entry, dict) and isinstance(runner, str), "recovery probe pins missing")
        assert isinstance(runner, str)
        contents = recovery_receipt.parse(raw, action=action, expected_runner=runner,
                                          expected_probe_sources=cast(Mapping[str, str], probe_entry))
        _require_historical(gate, binding, root, contents.source_binding,
                            (binding.baseline_slot, binding.baseline_floor, binding.baseline_generation),
                            contents.receipt_sha256)
    return validate


VALIDATORS["native_activation"] = _activation_validator("native_activation")
VALIDATORS["gui_happy_path"] = _activation_validator("gui_happy_path")
VALIDATORS["boot_restore"] = _recovery_validator("boot_restore")
VALIDATORS["disable_drain"] = _recovery_validator("disable_drain")


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
        validator(root, binding, raw, resolved_pins)
        evidence.append(contract.GateEvidence(gate, binding, gates[gate], True))
    return tuple(evidence)
