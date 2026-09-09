"""Owned runtime socket masks, anchored by inode identity on the same /run FS.

The privileged controller closes sockets, installs these masks, reloads the
manager and closes/cancels starts again before taking its transaction lock.
This module changes files only and never operates services or worker PIDs.
"""

import os
from pathlib import Path
import stat

if not __package__:
    import persistent_write_contract as contract
    import status_api_wiring as status
else:
    from tools import persistent_write_contract as contract
    from tools import status_api_wiring as status

RUNTIME_ROOT = Path("/run")
UNIT_ROOT = RUNTIME_ROOT / "systemd/system"
OWNER_UID = 0
require = contract.temporary.require


def exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def root_for(guard: Path) -> Path:
    contract.guard_units(guard)
    return RUNTIME_ROOT / (guard.name + "-fence")


def validate_directory(path: Path, mode: int) -> os.stat_result:
    meta = path.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == OWNER_UID and stat.S_IMODE(meta.st_mode) == mode
            and path.resolve(strict=True) == path, "unsafe fence directory")
    return meta


def validate_anchor(path: Path) -> os.stat_result:
    meta = path.lstat()
    require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == OWNER_UID and meta.st_nlink in (1, 2)
            and os.readlink(path) == "/dev/null", "unsafe fence anchor")
    return meta


def preflight(guard: Path) -> None:
    root = root_for(guard)
    require(not exists(root) and all(not exists(UNIT_ROOT / name) for name in contract.SOCKETS), "runtime mask namespace occupied")
    validate_directory(UNIT_ROOT, 0o755)
    runtime = RUNTIME_ROOT.lstat()
    require(stat.S_ISDIR(runtime.st_mode) and runtime.st_uid == OWNER_UID
            and not stat.S_IMODE(runtime.st_mode) & 0o022 and RUNTIME_ROOT.resolve(strict=True) == RUNTIME_ROOT,
            "unsafe runtime root")


def validate_owned(guard: Path) -> None:
    root = root_for(guard)
    validate_directory(root, 0o700)
    validate_directory(UNIT_ROOT, 0o755)
    require({path.name for path in root.iterdir()} <= set(contract.SOCKETS), "foreign fence contents")
    for name in contract.SOCKETS:
        anchor, target = root / name, UNIT_ROOT / name
        if exists(anchor):
            source = validate_anchor(anchor)
            if exists(target):
                public = target.lstat()
                require(stat.S_ISLNK(public.st_mode) and (source.st_dev, source.st_ino) == (public.st_dev, public.st_ino),
                        "runtime mask belongs to another owner")
        else:
            require(not exists(target), "unanchored runtime mask")


def install(guard: Path) -> None:
    root = root_for(guard)
    if not exists(root):
        preflight(guard)
        root.mkdir(mode=0o700)
        root.chmod(0o700)
        status.u.sync_directory(RUNTIME_ROOT)
    validate_owned(guard)
    require(root.stat().st_dev == UNIT_ROOT.stat().st_dev, "runtime mask anchor crosses filesystem")
    for name in contract.SOCKETS:
        anchor, target = root / name, UNIT_ROOT / name
        if not exists(anchor):
            os.symlink("/dev/null", anchor)
            status.u.sync_directory(root)
        if not exists(target):
            os.link(anchor, target, follow_symlinks=False)
            status.u.sync_directory(UNIT_ROOT)
    validate_owned(guard)


def remove(guard: Path) -> None:
    root = root_for(guard)
    if not exists(root):
        require(all(not exists(UNIT_ROOT / name) for name in contract.SOCKETS), "unowned mask remains")
        return
    validate_owned(guard)
    # All public paths were validated before the first unlink. Do not adopt a
    # matching /dev/null symlink created by somebody else: inode must match.
    for name in contract.SOCKETS:
        target = UNIT_ROOT / name
        if exists(target):
            target.unlink()
    status.u.sync_directory(UNIT_ROOT)
    for path in root.iterdir():
        path.unlink()
    root.rmdir()
    status.u.sync_directory(RUNTIME_ROOT)
