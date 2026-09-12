#!/usr/bin/env python3
"""Read early-boot recovery evidence before any runtime finalize command."""

import argparse
import json
import os
from pathlib import Path
from typing import cast

if not __package__:
    import installed_persistent_write as native
    import persistent_write_boot_proof as proof
else:
    from tools import installed_persistent_write as native
    from tools import persistent_write_boot_proof as proof


def observe(root: Path) -> dict[str, object]:
    require = native.require
    require(os.geteuid() == 0, 'root observer required')
    native.u.select_profile('dev7-dev9')
    transaction = native.decisions.validate_root(root)
    require(native.u.digest(root / 'controller.py') == native.u.digest(Path(native.__file__)),
            'observer and retained controller versions differ')
    plan = native.load(root)
    native.validate_guards(root, enabled=True)
    boot = native.boot_identity()
    require(boot != plan['boot'], 'still in prepared boot')
    winner = native.decisions.read(root)
    require(winner is not None and winner.binding == native.binding(plan), 'bound recovery decision missing')
    assert winner is not None
    require(plan['phase'] == 'restored' and plan['terminal'] is None and winner.outcome == 'revoked',
            'early-boot restoration not independently observable')
    native.systemd.Admission(root).require_masked_and_closed()
    backup = native.object_value(plan['backup'])
    native.snapshot.configuration(root, backup, wired=False)
    for unit in (*native.contract.SERVERS, *native.contract.SOCKETS):
        require(root.name + '-boot.service' not in native.s.property_value(unit, 'Requires').split(),
                'temporary guard dependency remains')
    pipeline = native.temporary.observer.observe()
    terminal = native.pipeline.terminal(pipeline, native.pipeline.Counters(8, 40))
    require(terminal is not None, 'pipeline not settled after boot')
    assert terminal is not None
    after = native.snapshot.capture(root, backup, terminal, wired=False, masked=True)
    native.snapshot.preserve(native.object_value(plan['before']), after, new_boot=True, allow_transition=False)
    fields = ('ActiveState', 'SubState', 'Result', 'ExecMainCode', 'ExecMainStatus', 'FragmentPath', 'DropInPaths',
              'ExecMainStartTimestampMonotonic', 'ExecMainExitTimestampMonotonic')
    guard = native.temporary.upload.properties(root.name + '-boot.service', fields)
    require(guard['FragmentPath'] == str(native.files.UNIT_ROOT / (root.name + '-boot.service'))
            and guard['DropInPaths'] == '', 'loaded boot guard differs')
    server = native.temporary.upload.properties(native.contract.temporary.CURRENT, ('ExecMainStartTimestampMonotonic',))
    deadline = native.temporary.upload.properties(root.name + '-restore.service', ('ExecMainStartTimestampMonotonic',))
    def number(value: str) -> int:
        require(value.isascii() and value.isdecimal(), 'invalid monotonic execution field')
        return int(value)
    value: dict[str, object] = {'transaction': transaction, 'bootIdSha256': boot,
        'decision': winner.outcome, 'phase': plan['phase'], 'terminal': plan['terminal'],
        'guardActiveState': guard['ActiveState'], 'guardSubState': guard['SubState'], 'guardResult': guard['Result'],
        'guardExecCode': number(guard['ExecMainCode']), 'guardExecStatus': number(guard['ExecMainStatus']),
        'guardStartUsec': number(guard['ExecMainStartTimestampMonotonic']),
        'guardExitUsec': number(guard['ExecMainExitTimestampMonotonic']),
        'serverStartUsec': number(server['ExecMainStartTimestampMonotonic']),
        'deadlineStartUsec': number(deadline['ExecMainStartTimestampMonotonic']),
        'liveWorkers': pipeline.live_workers, 'queuedJobs': pipeline.queued_jobs,
        'ownedFenceVerified': True, 'writePolicyAbsent': True, 'writeEnableLinksAbsent': True,
        'guardDependenciesAbsent': True, 'protectedConfigurationUnchanged': True, 'serverWritesDisabled': True,
        **{key: pipeline.status.get(key) for key in ('activeSlotId', 'confirmedSlotId', 'floorSequence', 'generation', 'pending')}}
    require(native.boot_identity() == boot, 'boot changed during observation')
    # Do not let a concurrent late deadline convert this into runtime evidence.
    current = native.load(root)
    require(current['phase'] == 'restored' and current['terminal'] is None, 'runtime finalize raced observer')
    return {'facts': value, 'qualification': proof.validate(value,
        prepared_boot=cast(str, plan['boot']), transaction=transaction), 'readOnly': True}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--guard-root', type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(observe(args.guard_root), sort_keys=True))


if __name__ == '__main__':
    main()
