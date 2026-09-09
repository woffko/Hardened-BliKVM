"""Preservation checks for the fixed installed sequence-seven to eight gate."""

import grp
from pathlib import Path
import pwd
import shlex
import stat
from typing import cast

if not __package__:
    import application_write_contract as contract
    import application_write_overrides as overrides
    import installed_upload_gate as upload
else:
    from tools import application_write_contract as contract
    from tools import application_write_overrides as overrides
    from tools import installed_upload_gate as upload


def protected_state(state: dict[str, object]) -> dict[str, object]:
    result = dict(state)
    slots, journal = upload.u.STATE_ROOTS
    for path, kind in ((slots / "current", "pointer"), (journal / "application-state.json", "journal")):
        raw_entry = result[str(path)]
        contract.require(isinstance(raw_entry, dict), "missing transition state entry")
        entry = cast(dict[str, object], raw_entry)
        # Only these inodes may be atomically replaced. Ownership, type/mode
        # and filesystem identity remain part of the preservation comparison.
        expected = {"mode", "uid", "gid", "device", "inode", "target" if kind == "pointer" else "sha256"}
        contract.require(set(entry) == expected, "unexpected transition state metadata")
        result[str(path)] = {key: value for key, value in entry.items() if key not in ("inode", "target", "sha256")}
    return result


def persistent_wiring() -> None:
    common = upload.boot.status_api
    for path in upload.boot.persistent.dropins():
        meta = path.parent.lstat()
        contract.require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o755
                         and path.parent.resolve(strict=True) == path.parent
                         and {p.name for p in path.parent.iterdir()} == {common.DROPIN_NAME},
                         "persistent status override directory changed")
        meta = path.lstat()
        contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                         and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == common.DROPIN_BYTES,
                         "persistent status override changed")


def environment(wired: bool) -> None:
    names = set(contract.WRITE_ENVIRONMENT)
    expected = [f"{key}={value}" for key, value in contract.WRITE_ENVIRONMENT.items()] if wired else []
    for unit in (contract.CURRENT, contract.CANDIDATE):
        actual = shlex.split(upload.s.property_value(unit, "Environment"))
        contract.require(sorted(item for item in actual if item.partition("=")[0] in names) == sorted(expected),
                         "loaded HTTP write environment differs")
    pid = upload.s.property_value(contract.CURRENT, "MainPID")
    contract.require(pid.isascii() and pid.isdecimal() and int(pid) > 0, "invalid current server PID")
    data = (Path("/proc") / pid / "environ").read_bytes()
    contract.require(len(data) <= 65536, "oversize current environment")
    encoded = {name.encode() for name in names}
    contract.require(sorted(item for item in data.split(b"\0") if item.partition(b"=")[0] in encoded)
                     == sorted(item.encode() for item in expected), "running HTTP write environment differs")


def snapshot(wired: bool, terminal: contract.TerminalState) -> dict[str, object]:
    persistent_wiring()
    for unit in (contract.CURRENT, contract.CANDIDATE):
        if wired:
            overrides.inspect(unit, allow_absent=False)
        else:
            contract.require(not upload.application.infrastructure.exists(overrides.directory(unit)), "runtime override remains")
    environment(wired)
    for unit, name in ((contract.UPLOAD, "upload"), (contract.ACTIVATION, "activation")):
        contract.require(upload.s.enabled(unit) == "disabled", "write socket persistently enabled")
        if wired:
            contract.require(upload.s.property_value(unit, "ActiveState") == "active", "write socket not active")
            meta = (Path("/run/hardened-blikvm-update-api") / (name + ".sock")).lstat()
            contract.require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
                             and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
                             and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid, "unsafe HTTP write endpoint")
    if not wired:
        upload.u.write_transports_inert()
    result = upload.boot.status_api.snapshot(True, expected_version=upload.u.VERSIONS["new"],
        expected_supervisor_sha=upload.u.SUPERVISOR_HASHES["new"], expected_slot=terminal.slot,
        expected_floor=terminal.floor, expected_generation=terminal.generation)
    contract.require((upload.u.STATE_ROOTS[0] / "current").readlink() == Path(terminal.slot), "selected symlink differs")
    result["persistentDropinHashes"] = {str(path): upload.u.digest(path) for path in upload.boot.persistent.dropins()}
    return result


def preserved(before: dict[str, object], after: dict[str, object], *, allow_transition: bool) -> None:
    contract.require(set(before) == set(after), "snapshot fields changed")
    for key in before:
        if key == "managerStatus" and allow_transition:
            continue  # Caller validates authoritative terminal state separately.
        if key == "state" and allow_transition:
            contract.require(protected_state(cast(dict[str, object], before[key]))
                             == protected_state(cast(dict[str, object], after[key])), "immutable application state changed")
        else:
            contract.require(before[key] == after[key], "protected runtime changed: " + key)
