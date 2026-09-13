"""Parse the installed negative browser rehearsal receipt for three fresh gates."""

from collections.abc import Mapping
from dataclasses import dataclass
import hashlib
import json
from typing import cast

if not __package__:
    import persistent_write_activation_receipt as common
    import persistent_write_contract as contract
else:
    from tools import persistent_write_activation_receipt as common
    from tools import persistent_write_contract as contract

require = common.require
NEGATIVE_BROWSER_SOURCES = frozenset({'application_response_capture.mjs', 'application_write_acceptance.mjs',
    'application_write_profiles.mjs', 'device_application_upload_failures.mjs', 'status_api_acceptance.mjs',
    'upload_termination.mjs'})
RETRY_PROFILES = {9: ('seq9', 'seq9-retry', 'signed-seq9'), 10: ('seq10', 'seq10-retry', 'signed-seq10')}
GATE_PROFILES = {9: 'seq9-retry', 10: 'seq10-retry'}
TERMINATION = {'aborted': True, 'afterLogoutRequested': True, 'kind': 'failed'}
TRANSPORT_FAILURES = ['net::ERR_ABORTED', 'net::ERR_ABORTED']


@dataclass(frozen=True)
class Contents:
    original_bytes: bytes
    receipt_sha256: str
    binding: contract.EvidenceBinding
    browser: dict[str, object]


def parse(raw: bytes, *, expected_runner: str, expected_browser_sources: Mapping[str, str]) -> Contents:
    """Validate one completed negative rehearsal and return its binding.

    Producer hashes must come from reviewed code, never from the receipt. The
    caller separately establishes root-owned custody before using the bytes.
    """
    require(0 < len(raw) <= 1048576, 'receipt size outside bounds')
    common.digest(expected_runner)
    require(set(expected_browser_sources) == set(NEGATIVE_BROWSER_SOURCES), 'incomplete reviewed browser producer')
    for value in expected_browser_sources.values():
        common.digest(value)
    try:
        value = common.object_value(json.loads(raw, object_pairs_hook=common.unique,
                                               parse_constant=common.reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid receipt encoding/depth') from error
    require(value.get('probe') == 'persistent-write-wiring-rehearsal' and value.get('action') == 'browser'
            and value.get('result') == 'passed', 'not a completed negative browser rehearsal')
    require(value.get('runnerSha256') == expected_runner
            and value.get('browserSourceHashes') == dict(expected_browser_sources), 'unreviewed receipt producer')
    for key in ('prepareSubmitted', 'cleanupSubmitted', 'runtimeWritesClosed', 'controllerStagingRemoved',
                'browserSubmitted'):
        require(value.get(key) is True, 'missing completed lifecycle evidence')
    for key in ('guardRetained', 'retentionApproved', 'targetPowerControl', 'rebootRequested'):
        require(value.get(key) is False, 'unexpected retained policy or physical/reboot action')
    require(common.number(value.get('applicationUploads')) == 4
            and common.number(value.get('applicationActivations')) == 0, 'negative lifecycle counts differ')
    prepared = common.object_value(value.get('prepared'))
    binding = common.read_binding(prepared.get('binding'))
    root = '/var/lib/' + contract.PREFIX + binding.transaction_id
    require(value.get('guardRoot') == root and prepared.get('guardRoot') == root
            and prepared.get('result') == 'prepared' and prepared.get('phase') == 'wired'
            and prepared.get('deadlineArmed') is True and prepared.get('retentionApproved') is False,
            'preparation not bound')
    before, after = common.object_value(value.get('before')), common.object_value(value.get('after'))
    require(common.digest(before.get('bootIdSha256')) == common.digest(after.get('bootIdSha256')),
            'negative gate crossed boot identities')
    require(before.get('runtimeWritesClosed') is True and after.get('runtimeWritesClosed') is True,
            'closed boundary not established')
    initial, terminal = common.object_value(before.get('status')), common.object_value(after.get('status'))
    require(initial == terminal and terminal.get('activeSlotId') == terminal.get('confirmedSlotId') == binding.baseline_slot
            and common.number(terminal.get('floorSequence')) == binding.baseline_floor
            and common.number(terminal.get('generation')) == binding.baseline_generation
            and 'pending' in terminal and terminal['pending'] is None
            and 'lastRollback' in terminal and terminal['lastRollback'] is None, 'negative gate changed baseline')
    restored, cleaned = common.object_value(value.get('restored')), common.object_value(value.get('cleaned'))
    require(restored.get('result') == 'restored' and restored.get('guardRoot') == root
            and restored.get('runtimeVerified') is True and restored.get('boot') is False
            and restored.get('retentionApproved') is False, 'restoration missing')
    require(cleaned.get('result') == 'cleaned' and cleaned.get('guardRoot') == root
            and cleaned.get('archiveRoot') == '/var/lib/hardened-blikvm-write-restored-' + binding.transaction_id
            and cleaned.get('runtimeWritesClosed') is True
            and common.number(cleaned.get('floor')) == binding.baseline_floor
            and common.number(cleaned.get('generation')) == binding.baseline_generation,
            'cleanup binding/counters differ')
    browser = common.object_value(value.get('browser'))
    require(browser.get('probe') == 'application-upload-failures' and browser.get('result') == 'passed',
            'negative browser result absent')
    expected = RETRY_PROFILES.get(binding.baseline_floor)
    require(expected is not None, 'negative browser gate has no reviewed baseline profile')
    assert expected is not None
    application_profile, browser_profile, selection = expected
    require(value.get('applicationProfile') == application_profile and browser.get('profile') == browser_profile
            and browser.get('selectionVerified') == selection, 'negative browser profile differs')
    for key, expected_number in (('uploadRequests', 4), ('activationRequests', 0), ('loginRequests', 1),
                                 ('pageErrors', 0), ('blockedMutations', 0), ('invalidHttpStatus', 200),
                                 ('cancelProgress', 1), ('logoutRequests', 1), ('logoutHttpStatus', 200),
                                 ('logoutProgress', 1)):
        require(common.number(browser.get(key)) == expected_number, 'negative browser counter differs')
    require(type(browser.get('invalidCode')) is int and browser.get('invalidCode') == -1,
            'invalid archive code differs')
    require(common.number(browser.get('unauthenticatedStatus')) == 401, 'unauthenticated status differs')
    for key in ('invalidArchiveRejected', 'cancelled', 'retryAlreadyPresent', 'throttleReleasedAfterLogout',
                'twoPagesOneSession'):
        require(browser.get(key) is True, 'negative browser assertion missing')
    require(browser.get('uploadHttpStatuses') == [200, 200], 'negative upload statuses differ')
    require(browser.get('uploadTransportFailures') == TRANSPORT_FAILURES, 'request-specific abort missing')
    require(common.object_value(browser.get('uploadTermination')) == TERMINATION, 'logout upload termination differs')
    return Contents(raw, hashlib.sha256(raw).hexdigest(), binding, browser)


def validate_gate(gate: str, contents: Contents) -> None:
    """Assert the per-gate facts already parsed from one shared receipt."""
    require(gate in ('gui_failure_retry', 'gui_cancel', 'gui_logout'), 'unknown negative browser gate')
    browser = contents.browser
    if gate == 'gui_failure_retry':
        require(browser.get('invalidArchiveRejected') is True and browser.get('retryAlreadyPresent') is True
                and common.number(browser.get('uploadRequests')) == 4, 'gui failure/retry evidence missing')
    elif gate == 'gui_cancel':
        require(browser.get('cancelled') is True and common.number(browser.get('cancelProgress')) == 1,
                'gui cancel evidence missing')
    else:
        require(common.number(browser.get('logoutRequests')) == 1
                and common.number(browser.get('logoutHttpStatus')) == 200
                and browser.get('uploadTransportFailures') == TRANSPORT_FAILURES
                and browser.get('throttleReleasedAfterLogout') is True, 'gui logout evidence missing')


def runner_pin(pins: Mapping[str, object]) -> str:
    value = pins.get('runner')
    require(isinstance(value, str), 'negative browser runner pin missing')
    return cast(str, value)


def source_pins(pins: Mapping[str, object]) -> dict[str, str]:
    value = pins.get('failure_browser_sources')
    require(isinstance(value, dict), 'negative browser source pins missing')
    result: dict[str, str] = {}
    for name, digest in cast(dict[object, object], value).items():
        require(isinstance(name, str) and isinstance(digest, str), 'invalid negative browser pin')
        result[cast(str, name)] = cast(str, digest)
    return result
