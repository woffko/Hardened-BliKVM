"""Durable same-boot ownership and deadline fencing for the HTTP gate."""

from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import secrets
import stat
import time
from collections.abc import Generator
from typing import cast

if not __package__:
    import application_write_contract as contract
    import installed_upload_gate as upload
else:
    from tools import application_write_contract as contract
    from tools import installed_upload_gate as upload

PHASES = ("prepared", "wired", "restoring", "restored")
BOOT_ID = Path("/proc/sys/kernel/random/boot_id")


def root_path(value: str) -> Path:
    root = Path(value)
    contract.guard_units(root)
    meta = root.lstat()
    contract.require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode)
                     and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe HTTP guard root")
    return root


def validate_plan(value: dict[str, object], *, boot_id: str) -> None:
    contract.require(type(value.get("schemaVersion")) is int and value["schemaVersion"] == 1
                     and value.get("phase") in PHASES and value.get("bootId") == boot_id
                     and re.fullmatch(r"[a-f0-9-]{36}", boot_id) is not None,
                     "invalid or previous-boot HTTP guard")
    floor = value.get("observedFloor")
    started = value.get("startedMonotonic")
    contract.require(type(floor) is int and floor in (7, 8), "invalid retained floor")
    contract.require(type(started) in (int, float) and isinstance(started, (int, float))
                     and 0 <= started < float("inf"), "invalid guard start time")
    contract.require(value.get("sourceSlot") == contract.SOURCE_SLOT and value.get("targetSlot") == contract.TARGET_SLOT,
                     "guard transition binding differs")


def load(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                     and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024,
                     "unsafe HTTP guard marker")
    value: object = json.loads(path.read_bytes(), object_pairs_hook=upload.probe.unique_object)
    contract.require(isinstance(value, dict), "invalid HTTP guard marker")
    result = cast(dict[str, object], value)
    validate_plan(result, boot_id=BOOT_ID.read_text().strip())
    return result


def save(root: Path, plan: dict[str, object]) -> None:
    validate_plan(plan, boot_id=BOOT_ID.read_text().strip())
    part = root / ("plan-" + secrets.token_hex(12) + ".part")
    upload.u.new_file(part, json.dumps(plan, sort_keys=True, allow_nan=False).encode())
    os.replace(part, root / "plan.json")
    upload.u.sync_directory(root)


def mark_deadline(root: Path) -> None:
    # Independent of the controller lock. Its presence permanently revokes
    # admission for this guard; prepare must recheck it after starting sockets.
    path = root / "deadline-fired"
    try:
        upload.u.new_file(path, b"closed\n")
    except FileExistsError:
        meta = path.lstat()
        contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                         and stat.S_IMODE(meta.st_mode) == 0o600 and path.read_bytes() == b"closed\n",
                         "foreign deadline marker")


def require_open_budget(root: Path, plan: dict[str, object], *, now: float | None = None) -> None:
    validate_plan(plan, boot_id=BOOT_ID.read_text().strip())
    current = time.monotonic() if now is None else now
    started = cast(float, plan["startedMonotonic"])
    contract.require(plan["phase"] == "prepared" and 0 <= current - started < 420
                     and not upload.application.infrastructure.exists(root / "deadline-fired"),
                     "HTTP admission lease expired or revoked")


@contextmanager
def locked(root: Path, *, timeout: float = 20) -> Generator[None, None, None]:
    contract.require(0 < timeout <= 30, "invalid HTTP guard lock timeout")
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    deadline = time.monotonic() + timeout
    try:
        while True:
            try:
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise TimeoutError("HTTP guard lock busy; admission must remain closed")
                time.sleep(0.1)
        yield
    finally:
        os.close(descriptor)
