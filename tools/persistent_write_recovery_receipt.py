"""Recovery receipt content checks; custody and current compatibility are separate."""

from collections.abc import Mapping
import hashlib
import json

if not __package__:
    import persistent_write_activation_receipt as common
    import persistent_write_boot_proof as boot
    import persistent_write_pending as pending
else:
    from tools import persistent_write_activation_receipt as common
    from tools import persistent_write_boot_proof as boot
    from tools import persistent_write_pending as pending

PROBES = {
    'pending': frozenset({'installed_pending_write_gate.py', 'persistent_write_pending.py'}),
    'boot': frozenset({'observe_persistent_write_boot.py', 'persistent_write_boot_proof.py'}),
}


def parse(raw: bytes, *, action: str, expected_runner: str,
          expected_probe_sources: Mapping[str, str]) -> common.Contents:
    require, obj, number = common.require, common.object_value, common.number
    require(action in PROBES and 0 < len(raw) <= 1048576, 'invalid recovery action/size')
    common.digest(expected_runner)
    require(set(expected_probe_sources) == set(PROBES[action]), 'incomplete reviewed probe source set')
    for digest in expected_probe_sources.values(): common.digest(digest)
    try:
        value = obj(json.loads(raw, object_pairs_hook=common.unique, parse_constant=common.reject_nonfinite))
    except (UnicodeDecodeError, RecursionError) as error:
        raise ValueError('invalid recovery receipt encoding/depth') from error
    require(value.get('probe') == 'persistent-write-wiring-rehearsal' and value.get('action') == action
        and value.get('result') == 'passed'
        and not ({'browser', 'browserSubmitted', 'browserReceipt', 'browserSourceHashes'} & value.keys()),
        'not a completed recovery-only gate')
    hashes = obj(value.get('sourceHashes'))
    require(value.get('runnerSha256') == expected_runner and
        all(hashes.get(name) == digest for name, digest in expected_probe_sources.items()), 'unreviewed recovery producer')
    for name in ('prepareSubmitted', 'cleanupSubmitted', 'runtimeWritesClosed', 'controllerStagingRemoved'):
        require(value.get(name) is True, 'incomplete recovery lifecycle')
    for name in ('guardRetained', 'retentionApproved', 'targetPowerControl'):
        require(value.get(name) is False, 'unexpected retained policy/physical action')
    require(value.get('rebootRequested') is (action == 'boot') and number(value.get('applicationActivations')) == 0
        and number(value.get('applicationUploads')) == (1 if action == 'pending' else 0), 'unexpected recovery operations')
    prepared = obj(value.get('prepared'))
    binding = common.read_binding(prepared.get('binding'))
    root = '/var/lib/' + common.contract.PREFIX + binding.transaction_id
    require(value.get('guardRoot') == root and prepared.get('guardRoot') == root and prepared.get('result') == 'prepared'
        and prepared.get('phase') == 'wired' and prepared.get('deadlineArmed') is True
        and prepared.get('retentionApproved') is False, 'unbound recovery preparation')
    before, after = obj(value.get('before')), obj(value.get('after'))
    old_boot, new_boot = common.digest(before.get('bootIdSha256')), common.digest(after.get('bootIdSha256'))
    require((old_boot != new_boot) == (action == 'boot'), 'recovery boot identity mismatch')
    require(before.get('runtimeWritesClosed') is True and after.get('runtimeWritesClosed') is True,
            'closed recovery boundaries missing')
    initial, terminal = obj(before.get('status')), obj(after.get('status'))
    require(initial == terminal and terminal.get('activeSlotId') == terminal.get('confirmedSlotId') == binding.baseline_slot
        and number(terminal.get('floorSequence')) == binding.baseline_floor
        and number(terminal.get('generation')) == binding.baseline_generation
        and 'pending' in terminal and terminal['pending'] is None
        and 'lastRollback' in terminal and terminal['lastRollback'] is None, 'recovery changed baseline')
    restored, cleaned = obj(value.get('restored')), obj(value.get('cleaned'))
    require(restored.get('guardRoot') == root and restored.get('result') == 'restored'
        and restored.get('runtimeVerified') is True and restored.get('boot') is False
        and restored.get('retentionApproved') is False, 'runtime restoration missing')
    require(cleaned.get('guardRoot') == root and cleaned.get('result') == 'cleaned'
        and cleaned.get('archiveRoot') == '/var/lib/hardened-blikvm-write-restored-' + binding.transaction_id
        and cleaned.get('runtimeWritesClosed') is True and number(cleaned.get('floor')) == binding.baseline_floor
        and number(cleaned.get('generation')) == binding.baseline_generation, 'recovery cleanup mismatch')
    if action == 'pending':
        require(value.get('pendingSubmissionAttempted') is True, 'pending fixture submission missing')
        fixture = obj(value.get('pending'))
        require(fixture.get('result') == 'passed' and fixture.get('fixture') == 'held-upload-idle-timeout'
            and fixture.get('guardRoot') == root and fixture.get('completeArchiveSent') is False
            and fixture.get('restored') == restored, 'pending fixture not bound')
        for name in ('restoreSubmissionAttempted', 'fencedWhileWorkerLive', 'probeSourceRemoved', 'terminalPreserved'):
            require(fixture.get(name) is True, 'missing live-fence/settlement proof')
        worker = obj(fixture.get('worker'))
        require(isinstance(worker.get('unit'), str) and isinstance(worker.get('invocation'), str), 'invalid worker identity')
        pending.Worker(str(worker['unit']), str(worker['invocation']), number(worker.get('pid'))).validate()
        exited = obj(fixture.get('workerNaturalExit'))
        require(number(exited.get('code')) == 1 and number(exited.get('status')) == 50, 'worker was not naturally timed out')
    else:
        require(value.get('bootSubmissionAttempted') is True, 'boot gate submission missing')
        early = obj(value.get('earlyBoot'))
        facts = obj(early.get('facts'))
        qualified = boot.validate(facts, prepared_boot=old_boot, transaction=binding.transaction_id, baseline=binding)
        require(early.get('readOnly') is True and early.get('qualification') == qualified
            and facts.get('bootIdSha256') == new_boot, 'early-boot proof does not match final boot')
    return common.Contents(raw, hashlib.sha256(raw).hexdigest(), binding, binding.baseline_slot,
                           binding.baseline_floor, binding.baseline_generation)
