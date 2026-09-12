"""Parse a pinned-producer activation receipt; no archive trust or commit yet."""

from collections.abc import Mapping
from dataclasses import dataclass
import hashlib
import json
import re
from typing import cast

if not __package__:
    import persistent_write_contract as contract
else:
    from tools import persistent_write_contract as contract

require = contract.temporary.require
BROWSER_SOURCES = frozenset({'device_application_write.mjs', 'application_write_acceptance.mjs',
    'application_write_profiles.mjs', 'status_api_acceptance.mjs', 'application_response_capture.mjs'})
TARGET_SLOT = 'app-00000000000000000009-5f1aee4802addd6c0c0b444224345624cd8b9b8c46950a2231ad7e2b42de37c2'


def object_value(value: object) -> dict[str, object]:
    require(isinstance(value, dict), 'receipt object required')
    return cast(dict[str, object], value)


def unique(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, 'duplicate receipt field')
        result[key] = value
    return result


def number(value: object) -> int:
    require(type(value) is int and value >= 0, 'invalid receipt counter')
    return cast(int, value)


def digest(value: object) -> str:
    require(isinstance(value, str) and re.fullmatch(r'[a-f0-9]{64}', value) is not None, 'invalid digest')
    return cast(str, value)


def reject_nonfinite(_value: str) -> object:
    raise ValueError('nonfinite receipt value')


def read_binding(value: object) -> contract.EvidenceBinding:
    bound = object_value(value)
    require(set(bound) == {'transaction_id', 'device_fingerprint', 'configuration_sha256',
                          'baseline_slot', 'baseline_floor', 'baseline_generation'}, 'invalid binding fields')
    require(all(isinstance(bound[k], str) for k in ('transaction_id', 'device_fingerprint', 'configuration_sha256', 'baseline_slot')),
            'invalid binding types')
    binding = contract.EvidenceBinding(cast(str, bound['transaction_id']), cast(str, bound['device_fingerprint']),
        cast(str, bound['configuration_sha256']), cast(str, bound['baseline_slot']),
        number(bound['baseline_floor']), number(bound['baseline_generation']))
    binding.validate()
    return binding


@dataclass(frozen=True)
class Contents:
    original_bytes: bytes
    receipt_sha256: str
    source_binding: contract.EvidenceBinding
    terminal_slot: str
    terminal_floor: int
    terminal_generation: int


def parse(raw: bytes, *, expected_runner: str, expected_browser_sources: Mapping[str, str]) -> Contents:
    """Expected producer hashes must come from reviewed code, never the receipt.

    Caller must separately establish root-owned custody/archive correspondence.
    Parsing a copied or user-authored document does not establish authenticity.
    The original transaction is retained and is never replaced with a new one.
    """
    require(0 < len(raw) <= 1048576, 'receipt size outside bounds')
    digest(expected_runner)
    require(set(expected_browser_sources) == set(BROWSER_SOURCES), 'incomplete reviewed browser producer')
    for value in expected_browser_sources.values(): digest(value)
    try:
        value = object_value(json.loads(raw, object_pairs_hook=unique, parse_constant=reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid receipt encoding/depth') from error
    require(value.get('probe') == 'persistent-write-wiring-rehearsal' and value.get('action') == 'activate'
            and value.get('result') == 'passed', 'not a completed activation rehearsal')
    require(value.get('runnerSha256') == expected_runner
            and value.get('browserSourceHashes') == dict(expected_browser_sources), 'unreviewed receipt producer')
    for key in ('prepareSubmitted', 'cleanupSubmitted', 'runtimeWritesClosed', 'controllerStagingRemoved', 'browserSubmitted'):
        require(value.get(key) is True, 'missing completed lifecycle evidence')
    for key in ('guardRetained', 'retentionApproved', 'targetPowerControl', 'rebootRequested'):
        require(value.get(key) is False, 'unexpected retained policy or physical/reboot action')
    require(number(value.get('applicationUploads')) == 1 and number(value.get('applicationActivations')) == 1,
            'activation was missing or repeated')
    prepared = object_value(value.get('prepared'))
    binding = read_binding(prepared.get('binding'))
    root = '/var/lib/' + contract.PREFIX + binding.transaction_id
    require(value.get('guardRoot') == root and prepared.get('guardRoot') == root
        and prepared.get('result') == 'prepared' and prepared.get('phase') == 'wired'
        and prepared.get('deadlineArmed') is True and prepared.get('retentionApproved') is False, 'preparation not bound')
    before, after = object_value(value.get('before')), object_value(value.get('after'))
    require(digest(before.get('bootIdSha256')) == digest(after.get('bootIdSha256')),
            'activation crossed boot identities')
    require(before.get('runtimeWritesClosed') is True and after.get('runtimeWritesClosed') is True,
            'closed boundary not established')
    initial, terminal = object_value(before.get('status')), object_value(after.get('status'))
    require(initial.get('activeSlotId') == initial.get('confirmedSlotId') == binding.baseline_slot
        and number(initial.get('floorSequence')) == binding.baseline_floor
        and number(initial.get('generation')) == binding.baseline_generation
        and 'pending' in initial and initial['pending'] is None, 'source baseline differs')
    slot = terminal.get('confirmedSlotId')
    require(isinstance(slot, str) and re.fullmatch(r'app-[0-9]{20}-[a-f0-9]{64}', slot) is not None,
            'invalid terminal slot')
    assert isinstance(slot, str)
    floor, generation = number(terminal.get('floorSequence')), number(terminal.get('generation'))
    require(terminal.get('activeSlotId') == slot and int(slot.split('-')[1]) == floor > binding.baseline_floor
        and generation > binding.baseline_generation and terminal.get('previousSlotId') == binding.baseline_slot
        and 'pending' in terminal and terminal['pending'] is None
        and 'lastRollback' in terminal and terminal['lastRollback'] is None, 'forward confirmation not proved')
    restored, cleaned = object_value(value.get('restored')), object_value(value.get('cleaned'))
    require(restored.get('result') == 'restored' and restored.get('guardRoot') == root
        and restored.get('runtimeVerified') is True and restored.get('boot') is False
        and restored.get('retentionApproved') is False, 'restoration missing')
    require(cleaned.get('result') == 'cleaned' and cleaned.get('guardRoot') == root
        and cleaned.get('archiveRoot') == '/var/lib/hardened-blikvm-write-restored-' + binding.transaction_id
        and cleaned.get('runtimeWritesClosed') is True and number(cleaned.get('floor')) == floor
        and number(cleaned.get('generation')) == generation, 'cleanup binding/counters differ')
    browser = object_value(value.get('browser'))
    require(browser.get('probe') == 'application-http-write' and browser.get('mode') == 'activate'
        and browser.get('profile') == 'seq8-to9' and browser.get('result') == 'passed'
        and browser.get('selectedArchiveVerified') is True, 'browser activation absent')
    for key, expected in (('uploadRequests', 1), ('activationRequests', 1), ('loginRequests', 1), ('blockedMutations', 0), ('pageErrors', 0)):
        require(number(browser.get(key)) == expected, 'browser request/error counts differ')
    ui = object_value(browser.get('ui'))
    require(ui.get('generationReviewed') == str(binding.baseline_generation)
        and ui.get('stagingDisposition') == 'published' and ui.get('uploadedAlreadyPresent') is False
        and number(ui.get('activationStatus')) == 202 and ui.get('activationReceiptValidated') is True,
        'browser staging/activation receipt incomplete')
    require(binding.baseline_slot == contract.temporary.TARGET_SLOT and slot == TARGET_SLOT
            and binding.baseline_floor == 8 and binding.baseline_generation == 40 and floor == 9 and generation == 43,
            'receipt outside reviewed seq8-to9 producer profile')
    return Contents(raw, hashlib.sha256(raw).hexdigest(), binding, slot, floor, generation)
