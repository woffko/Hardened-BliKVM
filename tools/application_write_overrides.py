"""Exact-owned runtime overrides; persistent status configuration is untouched."""

import os
from pathlib import Path
import stat

if not __package__:
    import application_write_contract as contract
    import installed_upload_gate as upload
else:
    from tools import application_write_contract as contract
    from tools import installed_upload_gate as upload

UNIT_ROOT = Path("/run/systemd/system")
OWNER_UID = 0


def directory(unit: str) -> Path:
    contract.require(unit in (contract.CURRENT, contract.CANDIDATE), "unapproved write override unit")
    return UNIT_ROOT / (unit + ".d")


def inspect(unit: str, *, allow_absent: bool) -> Path | None:
    parent = directory(unit)
    if not upload.application.infrastructure.exists(parent):
        contract.require(allow_absent, "write override directory absent")
        return None
    meta = parent.lstat()
    contract.require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == OWNER_UID and stat.S_IMODE(meta.st_mode) == 0o755
                     and parent.resolve(strict=True) == parent, "unsafe runtime override directory")
    entries = {child.name for child in parent.iterdir()}
    # An empty directory is recoverable after interrupted creation/removal.
    contract.require(entries in (set(), {contract.WRITE_DROPIN}), "foreign runtime override present")
    path = parent / contract.WRITE_DROPIN
    if not entries:
        contract.require(allow_absent, "write override file absent")
        return None
    meta = path.lstat()
    contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == OWNER_UID and meta.st_nlink == 1
                     and stat.S_IMODE(meta.st_mode) == 0o644 and meta.st_size == len(contract.WRITE_DROPIN_BYTES)
                     and path.read_bytes() == contract.WRITE_DROPIN_BYTES, "write override ownership/content differs")
    return path


def install() -> None:
    # Controller has recorded both paths as originally absent and armed its
    # independent deadline before calling this. Never adopt existing content.
    parents = tuple(directory(unit) for unit in (contract.CURRENT, contract.CANDIDATE))
    contract.require(all(not upload.application.infrastructure.exists(parent) for parent in parents),
                     "runtime override namespace already occupied")
    for parent in parents:
        parent.mkdir(mode=0o755)
        os.chmod(parent, 0o755)
        upload.u.sync_directory(UNIT_ROOT)
        upload.u.new_file(parent / contract.WRITE_DROPIN, contract.WRITE_DROPIN_BYTES, 0o644)


def remove() -> None:
    # Validate every target before the first deletion; unrelated files remain
    # untouched. An interrupted partial file fails closed for manual inspection.
    units = (contract.CURRENT, contract.CANDIDATE)
    paths = tuple(inspect(unit, allow_absent=True) for unit in units)
    for unit, path in zip(units, paths, strict=True):
        parent = directory(unit)
        if path is not None:
            path.unlink()
            upload.u.sync_directory(parent)
        if upload.application.infrastructure.exists(parent):
            parent.rmdir()
            upload.u.sync_directory(UNIT_ROOT)
