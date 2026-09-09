"""Exclusive ownership of persistent write-policy files, not service control.

The controller must durably save preflight output and arm/verify its guard
before installing transaction dependencies or policy. No CLI is exposed.
"""

import hashlib
import os
from pathlib import Path
import stat
from typing import cast

if not __package__:
    import persistent_write_contract as contract
    import status_api_wiring as status
else:
    from tools import persistent_write_contract as contract
    from tools import status_api_wiring as status

UNIT_ROOT = Path("/etc/systemd/system")
LIB_ROOT = Path("/lib/systemd/system")
OWNER_UID = 0
require = contract.temporary.require


def exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def safe_directory(path: Path) -> None:
    meta = path.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == OWNER_UID and stat.S_IMODE(meta.st_mode) == 0o755
            and path.resolve(strict=True) == path, "unsafe policy directory")


def safe_file(path: Path, expected: bytes) -> None:
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == OWNER_UID and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o644 and meta.st_size == len(expected)
            and path.read_bytes() == expected, "foreign policy file")


def status_files() -> tuple[Path, ...]:
    return tuple(UNIT_ROOT / (unit + ".d") / status.DROPIN_NAME for unit in contract.SERVERS)


def files(root: Path) -> dict[Path, bytes]:
    transaction = {UNIT_ROOT / path.relative_to("/etc/systemd/system"): data
                   for path, data in contract.transaction_dropins(root).items()}
    policy = {UNIT_ROOT / path.relative_to("/etc/systemd/system"): contract.DROPIN_BYTES for path in contract.dropins()}
    return {**policy, **transaction}


def links() -> dict[Path, Path]:
    return {UNIT_ROOT / "sockets.target.wants" / unit: LIB_ROOT / unit for unit in contract.SOCKETS}


def preflight(root: Path) -> dict[str, object]:
    expected = files(root)
    safe_directory(UNIT_ROOT)
    for path in status_files():
        safe_directory(path.parent)
        require({entry.name for entry in path.parent.iterdir()} == {status.DROPIN_NAME}, "unapproved server overrides")
        safe_file(path, status.DROPIN_BYTES)
    require(all(not exists(path) for path in (*expected, *links())), "write policy already exists")
    created = sorted({str(path.parent) for path in (*expected, *links()) if not exists(path.parent)})
    for path in (*expected, *links()):
        if exists(path.parent):
            safe_directory(path.parent)
            if path.parent.name.endswith(".socket.d"):
                require(not list(path.parent.iterdir()), "foreign socket overrides")
    return {"schemaVersion": 1, "guardRoot": str(root), "originallyAbsent": [str(path) for path in (*expected, *links())],
            "createdDirectories": created,
            "preservedStatus": {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in status_files()}}


def validate_backup(root: Path, backup: dict[str, object]) -> None:
    expected = files(root)
    require(type(backup.get("schemaVersion")) is int and backup["schemaVersion"] == 1 and backup.get("guardRoot") == str(root),
            "wrong policy backup")
    require(backup.get("originallyAbsent") == [str(path) for path in (*expected, *links())], "policy ownership backup differs")
    created = backup.get("createdDirectories")
    require(isinstance(created, list), "invalid directory backup")
    entries = cast(list[object], created)
    require(all(isinstance(value, str) for value in entries), "invalid directory entry")
    allowed = {str(path.parent) for path in (*expected, *links())} - {str(path.parent) for path in status_files()}
    require(set(cast(list[str], entries)) <= allowed and len(set(cast(list[str], entries))) == len(entries), "foreign backup directories")
    for path in status_files():
        safe_directory(path.parent)
        safe_file(path, status.DROPIN_BYTES)
    require(backup.get("preservedStatus") == {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in status_files()},
            "preserved status file changed")


def install(root: Path, backup: dict[str, object], *, transaction_only: bool) -> None:
    validate_backup(root, backup)
    all_files = files(root)
    for parent in {path.parent for path in all_files}:
        if exists(parent):
            safe_directory(parent)
            allowed = {path.name for path in status_files() if path.parent == parent}
            if not transaction_only:
                allowed.add(contract.TRANSACTION_DROPIN_NAME)
            require({path.name for path in parent.iterdir()} <= allowed, "unapproved pre-install overrides")
    expected = {path: data for path, data in files(root).items()
                if (path.name == contract.TRANSACTION_DROPIN_NAME) == transaction_only}
    require(all(not exists(path) for path in expected), "policy install is not a fresh operation")
    if not transaction_only:
        for path, data in files(root).items():
            if path.name == contract.TRANSACTION_DROPIN_NAME:
                safe_file(path, data)
        require(all(not exists(path) for path in links()), "socket enable link already exists")
    for path, data in expected.items():
        if not exists(path.parent):
            require(str(path.parent) in cast(list[str], backup["createdDirectories"]), "unowned directory creation")
            path.parent.mkdir(mode=0o755)
            path.parent.chmod(0o755)
            status.u.sync_directory(path.parent.parent)
        safe_directory(path.parent)
        status.u.new_file(path, data, 0o644)
    if not transaction_only:
        for path, target in links().items():
            if not exists(path.parent):
                require(str(path.parent) in cast(list[str], backup["createdDirectories"]), "unowned wants directory")
                path.parent.mkdir(mode=0o755)
                path.parent.chmod(0o755)
                status.u.sync_directory(path.parent.parent)
            safe_directory(path.parent)
            os.symlink(target, path)
            status.u.sync_directory(path.parent)


def detach_guard(root: Path, backup: dict[str, object]) -> None:
    """Only after a durable accepted commit; keep policy and enable links."""
    validate_backup(root, backup)
    expected = files(root)
    for parent in {path.parent for path in expected}:
        if exists(parent):
            safe_directory(parent)
            allowed = {path.name for path in (*expected, *status_files()) if path.parent == parent}
            require({path.name for path in parent.iterdir()} <= allowed, "foreign override before commit")
    for path, data in expected.items():
        if path.name == contract.DROPIN_NAME or exists(path):
            safe_directory(path.parent)
            safe_file(path, data)
    for path, target in links().items():
        safe_directory(path.parent)
        meta = path.lstat()
        require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == OWNER_UID and os.readlink(path) == str(target), "commit enable link differs")
    for path in expected:
        if path.name == contract.TRANSACTION_DROPIN_NAME and exists(path):
            path.unlink()
            status.u.sync_directory(path.parent)


def remove(root: Path, backup: dict[str, object]) -> None:
    validate_backup(root, backup)
    expected = files(root)
    # Validate ALL targets before deleting any. Policy bytes and enable links
    # are removed before the transaction guard dependencies.
    for path, data in expected.items():
        if exists(path.parent):
            safe_directory(path.parent)
            allowed = {candidate.name for candidate in expected if candidate.parent == path.parent}
            if path.parent in {entry.parent for entry in status_files()}:
                allowed.add(status.DROPIN_NAME)
            require({child.name for child in path.parent.iterdir()} <= allowed, "foreign override appeared")
        if exists(path):
            safe_file(path, data)
    for path, target in links().items():
        if exists(path):
            safe_directory(path.parent)
            meta = path.lstat()
            require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == OWNER_UID and os.readlink(path) == str(target), "foreign enable link")
    ordered = [path for path in expected if path.name == contract.DROPIN_NAME] + list(links())
    ordered += [path for path in expected if path.name == contract.TRANSACTION_DROPIN_NAME]
    for path in ordered:
        if exists(path):
            path.unlink()
            status.u.sync_directory(path.parent)
    for value in cast(list[str], backup["createdDirectories"]):
        path = Path(value)
        if exists(path):
            safe_directory(path)
            require(not list(path.iterdir()), "owned directory is not empty")
            path.rmdir()
            status.u.sync_directory(path.parent)
