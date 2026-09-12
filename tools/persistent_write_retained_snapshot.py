"""Read-only retained-policy checks, separate from unchanged recovery snapshots."""

import grp
from pathlib import Path
import pwd
import re
import stat

if not __package__:
    import installed_persistent_write as native
else:
    from tools import installed_persistent_write as native

files, contract = native.files, native.contract
require = native.require


def configuration(root: Path, backup: dict[str, object]) -> None:
    files.validate_backup(root, backup)
    expected = files.files(root)
    for path, data in expected.items():
        if path.name == contract.TRANSACTION_DROPIN_NAME:
            require(not files.exists(path), 'temporary guard override remains')
        else:
            require(path.name == contract.DROPIN_NAME, 'unknown retained policy file')
            files.safe_directory(path.parent)
            files.safe_file(path, data)
    for parent in {path.parent for path in expected}:
        if files.exists(parent):
            files.safe_directory(parent)
            allowed = {p.name for p in (*expected, *files.status_files())
                       if p.parent == parent and p.name != contract.TRANSACTION_DROPIN_NAME}
            require({p.name for p in parent.iterdir()} <= allowed, 'foreign retained override')
    for path, target in files.links().items():
        files.safe_directory(path.parent)
        meta = path.lstat()
        require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == 0 and path.readlink() == target,
                'retained socket enable link differs')


def loaded_units(root: Path) -> None:
    for unit in (*contract.SERVERS, *contract.SOCKETS):
        expected = []
        if unit in contract.SERVERS:
            expected = [str(files.UNIT_ROOT / (unit + '.d') / name)
                        for name in (files.status.DROPIN_NAME, contract.DROPIN_NAME)]
        require(native.s.property_value(unit, 'FragmentPath') == str(files.LIB_ROOT / unit), 'retained fragment differs')
        require(native.s.property_value(unit, 'DropInPaths').split() == sorted(expected), 'retained loaded overrides differ')
        require(root.name + '-boot.service' not in native.s.property_value(unit, 'Requires').split(),
                'retained unit still requires temporary guard')
        # The guard unit's own Before= can imply an After= edge until unloaded.
        # Exact drop-ins and absent Requires prove detachment without mistaking
        # that harmless ordering edge for a retained activation dependency.


def capture(root: Path, backup: dict[str, object], terminal: native.contract.temporary.TerminalState) -> dict[str, object]:
    winner = native.decisions.read(root)
    require(winner is not None and winner.outcome == 'committed', 'durable commit required before retained verification')
    assert winner is not None
    plan = native.load(root)
    require(winner.binding == native.binding(plan) and plan['phase'] in ('wired', 'committed')
            and plan['cleanupStarted'] is False, 'committed decision and plan differ')
    require(type(terminal.floor) is int and type(terminal.generation) is int
        and terminal.floor >= winner.binding.baseline_floor and terminal.generation >= winner.binding.baseline_generation
        and re.fullmatch(r'app-[0-9]{20}-[a-f0-9]{64}', terminal.slot) is not None
        and int(terminal.slot.split('-')[1]) == terminal.floor, 'invalid/regressed retained terminal state')
    native.u.select_profile('dev7-dev9')
    native.u.installed(root, 'new')
    configuration(root, backup)
    loaded_units(root)
    native.snapshot.state.environment(True)
    for unit, name in zip(contract.SOCKETS, ('upload', 'activation'), strict=True):
        require(native.s.enabled(unit) == 'enabled' and native.s.property_value(unit, 'ActiveState') == 'active',
                'retained write socket not active/enabled')
        meta = (Path('/run/hardened-blikvm-update-api') / (name + '.sock')).lstat()
        require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
            and meta.st_uid == pwd.getpwnam('hardened-blikvm-update').pw_uid
            and meta.st_gid == grp.getgrnam('hardened-blikvm').gr_gid, 'unsafe retained write endpoint')
    result = native.snapshot.state.upload.boot.status_api.snapshot(True, expected_version=native.u.VERSIONS['new'],
        expected_supervisor_sha=native.u.SUPERVISOR_HASHES['new'], expected_slot=terminal.slot,
        expected_floor=terminal.floor, expected_generation=terminal.generation)
    require((native.u.STATE_ROOTS[0] / 'current').readlink() == Path(terminal.slot), 'retained selected slot differs')
    result['persistentDropinHashes'] = {str(path): native.u.digest(path) for path in files.status_files()}
    return result
