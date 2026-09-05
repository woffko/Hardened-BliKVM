#!/usr/bin/env python3
"""Guarded persistent status wiring; no package, slot or journal mutation."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import persistent_status_contract as contract
    import status_api_wiring as common
else:
    from tools import persistent_status_contract as contract
    from tools import status_api_wiring as common

u = common.u
s = common.s
require = common.require
checked = common.checked
exists = common.infrastructure.exists
UNIT_ROOT = Path("/etc/systemd/system")
PREFIX = contract.PREFIX
SLOT = common.SLOT
HELPERS = tuple(dict.fromkeys((*common.HELPERS, "persistent_status_contract.py", "persistent_status_wiring.py")))


def dropins() -> tuple[Path, ...]:
    return tuple(UNIT_ROOT / (unit + ".d") / common.DROPIN_NAME for unit in common.SERVERS)


def guard_root(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(contract.PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid persistent status guard")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0
            and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe persistent status guard")
    return root


def load(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe persistent status marker")
    value = cast(dict[str, object], json.loads(path.read_bytes()))
    require(set(value) == {"schemaVersion", "slotId", "phase", "before"}
            and value.get("schemaVersion") == 1 and value.get("slotId") == common.SLOT, "persistent status marker mismatch")
    contract.Phase(value["phase"])
    require(isinstance(value["before"], dict), "missing baseline")
    return value


def validate_dropins(wired: bool) -> None:
    require(all(not exists(path.parent) for path in common.dropins()), "runtime overrides remain")
    for path in dropins():
        if not wired:
            require(not exists(path.parent), "persistent override remains")
            continue
        meta = path.parent.lstat()
        require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o755
                and {child.name for child in path.parent.iterdir()} == {common.DROPIN_NAME}, "foreign persistent override directory")
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == common.DROPIN_BYTES, "persistent override changed")


def verify(plan: dict[str, object], wired: bool, *, boot: bool = False) -> None:
    validate_dropins(wired)
    before = cast(dict[str, object], plan["before"])
    if not boot:
        common.await_snapshot(wired, before)
        return
    # Do not wait for services deliberately ordered after this boot guard.
    require(u.tree_fingerprint(u.STATE_ROOTS) == before["state"], "persistent application state changed")
    runtime = cast(dict[str, str], before["runtime"])
    require(all(u.digest(path) == runtime[name] for name, path in s.CHECKED_FILES.items()), "protected configuration changed")
    require(u.digest(s.UNIT) == before["nginxUnit"], "Nginx unit changed")


def arm(root: Path) -> None:
    for name, content in contract.guard_units(root).items():
        u.new_file(UNIT_ROOT / name, content.encode(), 0o644)
    u.sync_directory(UNIT_ROOT)
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "enable", root.name + "-boot.service"])
    checked(["systemctl", "enable", "--now", root.name + "-deadline.timer"])
    require(s.enabled(root.name + "-boot.service") == "enabled", "boot recovery not enabled")
    checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"])
    os.sync()


def disarm(root: Path, *, boot: bool) -> None:
    for name, content in contract.guard_units(root).items():
        path = UNIT_ROOT / name
        if exists(path):
            require(not path.is_symlink() and path.read_text() == content, "foreign recovery unit")
    for suffix in ("-deadline.timer", "-boot.service"):
        name = root.name + suffix
        if exists(UNIT_ROOT / name):
            if suffix.endswith("timer"):
                checked(["systemctl", "stop", "--no-block", name] if boot else ["systemctl", "stop", name])
            checked(["systemctl", "disable", name])
    for name in contract.guard_units(root):
        (UNIT_ROOT / name).unlink(missing_ok=True)
    u.sync_directory(UNIT_ROOT)
    checked(["systemctl", "daemon-reload"])


def preflight() -> dict[str, object]:
    require(not list(Path("/var/lib").glob(contract.PREFIX + "*"))
            and not list(UNIT_ROOT.glob(contract.PREFIX + "*"))
            and not list(Path("/var/lib").glob(common.PREFIX + "*"))
            and not list(Path("/var/lib").glob("hardened-blikvm-application-update-*")), "unfinished transaction")
    validate_dropins(False)
    return common.snapshot(False)


def prepare(assets: Path) -> dict[str, object]:
    before = preflight()
    root = Path("/var/lib") / (contract.PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in HELPERS:
        u.new_file(root / name, (assets / name).read_bytes())
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "slotId": common.SLOT, "phase": "prepared", "before": before}
    common.save(root, plan)
    u.sync_directory(root.parent)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        arm(root)
        for path in dropins():
            path.parent.mkdir(mode=0o755)
            path.parent.chmod(0o755)
            u.new_file(path, common.DROPIN_BYTES, 0o644)
            u.sync_directory(path.parent)
        u.sync_directory(UNIT_ROOT)
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", common.SOCKET])
        checked(["systemctl", "restart", s.CURRENT_SERVER])
        verify(plan, True)
        plan["phase"] = "wired"
        common.save(root, plan)
        return {"result": "prepared", "guardRoot": str(root), "statePreserved": True}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)


def transition(action: str, root: Path, *, boot: bool = False, browser_verified: bool = False) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load(root)
        phase = contract.Phase(plan["phase"])
        if action == "restore":
            decision = contract.recovery_decision(phase, boot=boot)
            if decision.remove_dropins:
                common.remove_matching_dropins(dropins())
                u.sync_directory(UNIT_ROOT)
                checked(["systemctl", "daemon-reload"])
            if decision.restart_server:
                checked(["systemctl", "restart", s.CURRENT_SERVER])
            if decision.stop_socket:
                checked(["systemctl", "stop", common.SOCKET])
            phase = decision.phase
            verify(plan, phase is contract.Phase.COMMITTED, boot=boot)
            plan["phase"] = phase.value
            common.save(root, plan)
            disarm(root, boot=boot)
        else:
            verify(plan, phase in (contract.Phase.WIRED, contract.Phase.COMMITTED))
            if action == "commit":
                phase = contract.require_commit(phase, native_verified=True, browser_verified=browser_verified)
                plan["phase"] = phase.value
                common.save(root, plan)
                disarm(root, boot=False)
            elif action == "cleanup":
                require(phase in (contract.Phase.RESTORED, contract.Phase.COMMITTED), "persistent transaction still pending")
                disarm(root, boot=False)
                expected = {"controller.py", "plan.json", *HELPERS}
                require({path.name for path in root.iterdir()} == expected, "unknown persistent guard files")
                for path in root.iterdir():
                    meta = path.lstat()
                    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1, "unsafe persistent guard file")
                for name in expected:
                    (root / name).unlink()
                root.rmdir()
                u.sync_directory(root.parent)
        return {"result": action, "phase": phase.value, "guardRoot": str(root),
                "runtimeVerified": not boot, "bootRecoveryPendingVerification": boot}
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "restore", "commit", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    parser.add_argument("--boot", action="store_true")
    parser.add_argument("--browser-verified", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0 and (not args.boot or args.action == "restore"), "invalid persistent execution mode")
    if args.action == "preflight":
        preflight()
        result: dict[str, object] = {"result": "preflight"}
    elif args.action == "prepare":
        result = prepare(args.assets)
    else:
        result = transition(args.action, guard_root(args.guard_root), boot=args.boot, browser_verified=args.browser_verified)
    print(json.dumps(result, sort_keys=True))
    if result["result"] == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
