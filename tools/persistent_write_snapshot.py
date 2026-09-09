"""Installed-state checks for persistent write preparation and recovery."""

import grp
from pathlib import Path
import pwd
import re
import stat
from typing import cast

if not __package__:
    import application_write_state as state
    import persistent_write_contract as contract
    import persistent_write_files as files
else:
    from tools import application_write_state as state
    from tools import persistent_write_contract as contract
    from tools import persistent_write_files as files

u, s = state.upload.u, state.upload.s
require = contract.temporary.require


def configuration(root: Path, backup: dict[str, object], *, wired: bool) -> None:
    files.validate_backup(root, backup)
    expected = files.files(root)
    for path, data in expected.items():
        if wired:
            files.safe_directory(path.parent)
            files.safe_file(path, data)
        else:
            require(not files.exists(path), "owned write override remains")
    for parent in {path.parent for path in expected}:
        if files.exists(parent):
            files.safe_directory(parent)
            allowed = {path.name for path in files.status_files() if path.parent == parent}
            if wired:
                allowed |= {path.name for path in expected if path.parent == parent}
            require({path.name for path in parent.iterdir()} <= allowed, "foreign write override")
    for path, target in files.links().items():
        if wired:
            files.safe_directory(path.parent)
            meta = path.lstat()
            require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == 0 and path.readlink() == target,
                    "socket enable link changed")
        else:
            require(not files.exists(path), "write socket enable link remains")


def loaded_units(root: Path, *, wired: bool, masked: bool) -> None:
    for unit in (*contract.SERVERS, *contract.SOCKETS):
        expected: list[str] = []
        if unit in contract.SERVERS:
            expected.append(str(files.UNIT_ROOT / (unit + ".d") / files.status.DROPIN_NAME))
        if wired:
            expected.append(str(files.UNIT_ROOT / (unit + ".d") / contract.TRANSACTION_DROPIN_NAME))
            if unit in contract.SERVERS:
                expected.append(str(files.UNIT_ROOT / (unit + ".d") / contract.DROPIN_NAME))
        # A loaded mask suppresses a unit's drop-ins altogether. On unmask the
        # caller repeats this check against the installed package fragment.
        if masked and unit in contract.SOCKETS:
            require(s.property_value(unit, "LoadState") == "masked", "write mask not loaded")
            continue
        require(s.property_value(unit, "FragmentPath") == str(files.LIB_ROOT / unit), "unit fragment differs")
        require(s.property_value(unit, "DropInPaths").split() == sorted(expected), "loaded overrides differ")
        if wired:
            guard = root.name + "-boot.service"
            require(all(guard in s.property_value(unit, field).split() for field in ("Requires", "After")),
                    "loaded transaction dependency missing")


def capture(root: Path, backup: dict[str, object], terminal: contract.temporary.TerminalState, *,
            wired: bool, masked: bool) -> dict[str, object]:
    configuration(root, backup, wired=wired)
    loaded_units(root, wired=wired, masked=masked)
    state.environment(wired)
    for unit, name in zip(contract.SOCKETS, ("upload", "activation"), strict=True):
        if wired:
            require(s.enabled(unit) == "enabled" and s.property_value(unit, "ActiveState") == "active", "write socket not enabled/active")
            meta = (Path("/run/hardened-blikvm-update-api") / (name + ".sock")).lstat()
            require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
                    and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
                    and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid, "unsafe write endpoint")
    if not wired and not masked:
        u.write_transports_inert()
    result = state.upload.boot.status_api.snapshot(True, expected_version=u.VERSIONS["new"],
        expected_supervisor_sha=u.SUPERVISOR_HASHES["new"], expected_slot=terminal.slot,
        expected_floor=terminal.floor, expected_generation=terminal.generation)
    require((u.STATE_ROOTS[0] / "current").readlink() == Path(terminal.slot), "selected slot differs")
    result["persistentDropinHashes"] = {str(path): u.digest(path) for path in files.status_files()}
    return result


def preserve(before: dict[str, object], after: dict[str, object], *, new_boot: bool, allow_transition: bool) -> None:
    require(set(before) == set(after), "preservation snapshot fields differ")
    for key in before:
        if key == "managerStatus" and allow_transition:
            continue  # Caller independently validates monotonic terminal state.
        if key == "serving" and allow_transition:
            old, new = cast(dict[str, object], before[key]), cast(dict[str, object], after[key])
            require({k: v for k, v in old.items() if k != "executableSha256"}
                    == {k: v for k, v in new.items() if k != "executableSha256"}, "serving contract changed")
        elif key == "runtime" and new_boot:
            old, new = cast(dict[str, object], before[key]), cast(dict[str, object], after[key])
            require(set(old) == set(new) and all(new[name] == old[name] for name in s.CHECKED_FILES), "protected configuration changed")
            require(all(new[name + "Restarts"] == "0" for name in s.PEERS), "peer restarted after boot")
        elif key == "state" and allow_transition:
            old = state.protected_state(cast(dict[str, object], before[key]))
            new = state.protected_state(cast(dict[str, object], after[key]))
            require(all(new.get(path) == value for path, value in old.items()), "retained immutable state changed")
            for name in new.keys() - old.keys():
                path = Path(name)
                require(u.STATE_ROOTS[0] in path.parents, "unexpected new journal state")
                relative = path.relative_to(u.STATE_ROOTS[0])
                require(re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", relative.parts[0]) is not None,
                        "unexpected new slot state")
        else:
            require(before[key] == after[key], "protected snapshot changed: " + key)
