"""Validate observed early-boot recovery before runtime finalization.

No reboot or device I/O. Timestamp fields must come from the current systemd
manager's monotonic execution properties, not wall time or host estimates.
"""

import re

if not __package__:
    import persistent_write_contract as contract
else:
    from tools import persistent_write_contract as contract

require = contract.temporary.require


def validate(value: dict[str, object], *, prepared_boot: str, transaction: str,
             baseline: contract.EvidenceBinding | None = None) -> dict[str, object]:
    if baseline is not None:
        baseline.validate()
        require(baseline.transaction_id == transaction, 'baseline transaction differs')
    require(re.fullmatch(r'[a-f0-9]{64}', prepared_boot) is not None
            and re.fullmatch(r'[a-f0-9]{24}', transaction) is not None, 'invalid prepared binding')
    require(value.get('transaction') == transaction, 'boot observation belongs to another transaction')
    boot = value.get('bootIdSha256')
    require(isinstance(boot, str) and re.fullmatch(r'[a-f0-9]{64}', boot) is not None
            and boot != prepared_boot, 'new boot not proved')
    require(value.get('decision') == 'revoked' and value.get('phase') == 'restored'
            and 'terminal' in value and value['terminal'] is None,
            'early recovery absent or runtime finalization already occurred')
    require(value.get('guardActiveState') == 'active' and value.get('guardSubState') == 'exited'
            and value.get('guardResult') == 'success', 'boot guard not successfully completed')
    for name in ('guardExecCode', 'guardExecStatus', 'guardStartUsec', 'guardExitUsec',
                 'serverStartUsec', 'deadlineStartUsec', 'liveWorkers', 'queuedJobs'):
        require(type(value.get(name)) is int, 'invalid execution evidence type')
    require(value['guardExecCode'] == 1 and value['guardExecStatus'] == 0, 'boot guard did not exit normally')
    start, end, serving = value['guardStartUsec'], value['guardExitUsec'], value['serverStartUsec']
    assert isinstance(start, int) and isinstance(end, int) and isinstance(serving, int)
    require(0 < start <= end < serving, 'application started before boot guard completed')
    require(value['deadlineStartUsec'] == 0, 'deadline recovery may have contributed')
    require(value['liveWorkers'] == 0 and value['queuedJobs'] == 0, 'pending pipeline at boot proof')
    for name in ('ownedFenceVerified', 'writePolicyAbsent', 'writeEnableLinksAbsent',
                 'guardDependenciesAbsent', 'protectedConfigurationUnchanged', 'serverWritesDisabled'):
        require(value.get(name) is True, 'closed boot policy evidence missing')
    slot = baseline.baseline_slot if baseline is not None else contract.temporary.TARGET_SLOT
    floor = baseline.baseline_floor if baseline is not None else 8
    generation = baseline.baseline_generation if baseline is not None else 40
    require(value.get('activeSlotId') == slot and value.get('confirmedSlotId') == slot
            and type(value.get('floorSequence')) is int and value['floorSequence'] == floor
            and type(value.get('generation')) is int and value['generation'] == generation
            and 'pending' in value and value['pending'] is None, 'boot baseline changed')
    return {'result': 'passed', 'transaction': transaction, 'bootIdSha256': boot,
            'guardCompletedBeforeServing': True, 'deadlineNotExecuted': True,
            'runtimeFinalizationStillRequired': True}
