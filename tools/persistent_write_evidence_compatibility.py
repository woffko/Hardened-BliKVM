"""Compatibility only: not receipt authenticity, gate acceptance or retention.

The future privileged adapter must read and validate original receipt contents
and archived state before using these rules. Never convert this result into a
passed GateEvidence by changing the original transaction identifier.
"""

from dataclasses import dataclass
from collections.abc import Mapping
import re
from types import MappingProxyType

if not __package__:
    import persistent_write_contract as contract
else:
    from tools import persistent_write_contract as contract


FRESH_GATES = frozenset({'host', 'native_upload', 'gui_failure_retry', 'gui_cancel', 'gui_logout'})
HISTORICAL_GATES = frozenset({'native_activation', 'gui_happy_path', 'boot_restore', 'disable_drain'})
COMMON = frozenset({'updaterPackage', 'applicationPayload', 'trustRoot', 'unitContracts'})
COMPONENTS = {
    'host': COMMON | {'controller', 'testHarness'},
    'native_upload': COMMON | {'controller', 'testHarness'},
    'gui_failure_retry': COMMON | {'controller', 'testHarness', 'proxyExecutable', 'proxyConfiguration'},
    'gui_cancel': COMMON | {'controller', 'testHarness', 'proxyExecutable', 'proxyConfiguration'},
    'gui_logout': COMMON | {'controller', 'testHarness', 'proxyExecutable', 'proxyConfiguration'},
    'native_activation': COMMON,
    'gui_happy_path': COMMON | {'proxyExecutable', 'proxyConfiguration'},
    'boot_restore': COMMON | {'controller', 'recoveryPolicy'},
    'disable_drain': COMMON | {'controller', 'recoveryPolicy'},
}


@dataclass(frozen=True)
class Source:
    gate: str
    binding: contract.EvidenceBinding
    terminal_slot: str
    terminal_floor: int
    terminal_generation: int
    receipt_sha256: str
    components: Mapping[str, str]

    def __post_init__(self) -> None:
        object.__setattr__(self, 'components', MappingProxyType(dict(self.components)))


@dataclass(frozen=True)
class Compatibility:
    target: contract.EvidenceBinding
    original: Source
    historical: bool


def check(source: Source, current: contract.EvidenceBinding, components: dict[str, str]) -> Compatibility:
    require = contract.temporary.require
    require(source.gate in COMPONENTS, 'unknown qualification gate')
    source.binding.validate()
    current.validate()
    require(re.fullmatch(r'[a-f0-9]{64}', source.receipt_sha256) is not None, 'invalid original receipt hash')
    require(source.binding.device_fingerprint == current.device_fingerprint
            and source.binding.configuration_sha256 == current.configuration_sha256, 'device/write configuration differs')
    required = COMPONENTS[source.gate]
    require(required <= source.components.keys() and required <= components.keys(), 'missing compatibility components')
    for name in required:
        require(re.fullmatch(r'[a-f0-9]{64}', source.components[name]) is not None
                and source.components[name] == components[name], 'safety-critical component differs: ' + name)
    require(type(source.terminal_floor) is int and type(source.terminal_generation) is int
            and (source.terminal_slot, source.terminal_floor, source.terminal_generation)
                == (current.baseline_slot, current.baseline_floor, current.baseline_generation),
            'source terminal state does not establish current baseline')
    historical = source.gate in HISTORICAL_GATES
    if not historical:
        require(source.binding == current, 'fresh gate must belong to current transaction')
    else:
        require(source.binding.transaction_id != current.transaction_id, 'historical source was relabeled as current')
        if source.gate in {'boot_restore', 'disable_drain'}:
            require((source.binding.baseline_slot, source.binding.baseline_floor, source.binding.baseline_generation)
                    == (current.baseline_slot, current.baseline_floor, current.baseline_generation),
                    'recovery prerequisite tested another baseline')
        else:
            require(source.binding.baseline_floor < source.terminal_floor
                    and source.binding.baseline_generation < source.terminal_generation,
                    'activation prerequisite must prove forward confirmation, not an already-active retry')
    return Compatibility(current, source, historical)
