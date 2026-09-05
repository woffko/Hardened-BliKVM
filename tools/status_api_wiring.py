#!/usr/bin/env python3
"""Reversible runtime-only wiring of the installed read-only status API."""

from __future__ import annotations

import argparse
import fcntl
import grp
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import shlex
import stat
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import enable_update_services as infrastructure
else:
    from tools import enable_update_services as infrastructure

u = infrastructure.upgrade
s = u.serving
require = u.require
checked = u.checked
PREFIX = "hardened-blikvm-status-api-"
SLOT = "app-00000000000000000004-768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c"
SOCKET = "hardened-blikvm-update-status.socket"
ENDPOINT = Path("/run/hardened-blikvm-update-api/status.sock")
ENVIRONMENT = "BLI_KVM_UPDATE_STATUS_SOCKET=" + str(ENDPOINT)
UNIT_ROOT = Path("/run/systemd/system")
DROPIN_NAME = "85-status-api-rehearsal.conf"
DROPIN_BYTES = (f"[Unit]\nWants={SOCKET}\nAfter={SOCKET}\n[Service]\nEnvironment={ENVIRONMENT}\n").encode()
SERVERS = (s.CURRENT_SERVER, s.CANDIDATE_SERVER)
HELPERS = tuple(dict.fromkeys((*u.HELPERS, "status_api_wiring.py")))
UNIT_HASHES = {
    SOCKET: "8ca4e41fd6ce265c91bbf0b4adc5f86ac79bd643e46394778f2b834db6ed92cb",
    "hardened-blikvm-update-status@.service": "f846555a10aa35a99092cc9d21e5358081dd4f210a124da1d5aa8fbd414485f2",
}


def dropins() -> tuple[Path, ...]:
    return tuple(UNIT_ROOT / (unit + ".d") / DROPIN_NAME for unit in SERVERS)


def runtime_environment(wired: bool) -> None:
    for unit in SERVERS:
        environment = shlex.split(s.property_value(unit, "Environment"))
        found = [item for item in environment if item.startswith("BLI_KVM_UPDATE_STATUS_SOCKET=")]
        require(found == ([ENVIRONMENT] if wired else []), "loaded status environment mismatch")
    pid = s.property_value(s.CURRENT_SERVER, "MainPID")
    require(pid.isdecimal() and int(pid) > 0, "invalid confirmed server PID")
    environment = (Path("/proc") / pid / "environ").read_bytes().split(b"\0")
    found = [item for item in environment if item.startswith(b"BLI_KVM_UPDATE_STATUS_SOCKET=")]
    require(found == ([ENVIRONMENT.encode()] if wired else []), "running status environment mismatch")


def snapshot(wired: bool) -> dict[str, object]:
    require(checked(["dpkg-query", "-W", "-f=${db:Status-Abbrev}:${Version}", "hardened-blikvm-update-runtime"]).decode()
            == "ii :0.1.0~dev6", "unexpected installed updater version")
    for unit, digest in UNIT_HASHES.items():
        require(u.digest(Path("/lib/systemd/system") / unit) == digest, "installed status unit changed")
    runtime_environment(wired)
    s.inactive(s.CANDIDATE_SERVER)
    require(s.enabled(SOCKET) == "disabled", "status socket was enabled")
    if wired:
        require(s.property_value(SOCKET, "ActiveState") == "active", "status socket not active")
        meta = ENDPOINT.lstat()
        require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
                and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
                and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid, "unsafe status socket")
    else:
        u.status_transport_inert()
    value = infrastructure.enabled_snapshot(expected_slot=SLOT, expected_floor=4, check_initial=False)
    # Restarting only the confirmed server is the intended runtime change.
    serving = cast(dict[str, object], value["serving"])
    serving.pop("serverPid")
    status = infrastructure.current_status()
    require(status.get("generation") == 16, "application journal generation changed")
    value["managerStatus"] = status
    return value


def await_snapshot(wired: bool, expected: object) -> None:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            require(snapshot(wired) == expected, "status wiring changed protected state")
            return
        except (OSError, ValueError, subprocess.SubprocessError):
            time.sleep(1)
    raise ValueError("status runtime did not converge")


def guard_root(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid status guard namespace")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0
            and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe status guard root")
    return root


def units(root: Path) -> dict[str, str]:
    return {
        root.name + "-restore.service": ("[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=120\n"
            f"ExecStart=/usr/bin/python3 -B {root}/controller.py restore --guard-root {root}\n"),
        root.name + "-deadline.timer": f"[Timer]\nOnActiveSec=600\nUnit={root.name}-restore.service\n",
    }


def load(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe status marker")
    value = cast(dict[str, object], json.loads(path.read_bytes()))
    require(value.get("schemaVersion") == 1 and value.get("slotId") == SLOT
            and value.get("phase") in ("prepared", "wired", "restored"), "invalid status marker")
    return value


def save(root: Path, plan: dict[str, object]) -> None:
    # Unique temporary names avoid overwriting an unrecognized interrupted file.
    temporary = root / ("plan-" + secrets.token_hex(12) + ".part")
    u.new_file(temporary, json.dumps(plan, sort_keys=True).encode())
    os.replace(temporary, root / "plan.json")
    u.sync_directory(root)


def prepare(assets: Path) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")), "unfinished status guard")
    require(not list(Path("/var/lib").glob("hardened-blikvm-application-update-*")), "unfinished application transaction")
    for unit, path in zip(SERVERS, dropins(), strict=True):
        require(not infrastructure.exists(path.parent)
                and not infrastructure.exists(Path("/etc/systemd/system") / (unit + ".d")), "server override already exists")
    before = snapshot(False)
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in HELPERS:
        u.new_file(root / name, (assets / name).read_bytes())
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "slotId": SLOT, "phase": "prepared", "before": before}
    save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        for name, content in units(root).items():
            u.new_file(UNIT_ROOT / name, content.encode(), 0o644)
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", root.name + "-deadline.timer"])
        checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"])
        for path in dropins():
            path.parent.mkdir(mode=0o755)
            path.parent.chmod(0o755)
            u.new_file(path, DROPIN_BYTES, 0o644)
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", SOCKET])
        checked(["systemctl", "restart", s.CURRENT_SERVER])
        await_snapshot(True, before)
        plan["phase"] = "wired"
        save(root, plan)
        return {"result": "prepared", "guardRoot": str(root), "statePreserved": True}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)


def restore(root: Path, plan: dict[str, object]) -> None:
    # Validate all parents and files before removing any override.
    for path in dropins():
        if infrastructure.exists(path.parent):
            meta = path.parent.lstat()
            require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == os.geteuid()
                    and stat.S_IMODE(meta.st_mode) == 0o755
                    and {item.name for item in path.parent.iterdir()} <= {DROPIN_NAME}, "foreign server override directory")
        if infrastructure.exists(path):
            meta = path.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == os.geteuid() and meta.st_nlink == 1
                    and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == DROPIN_BYTES, "foreign status override")
    for path in dropins():
        if infrastructure.exists(path):
            require(not path.is_symlink() and path.read_bytes() == DROPIN_BYTES, "foreign status override")
            path.unlink()
        if path.parent.exists():
            require(not path.parent.is_symlink() and not list(path.parent.iterdir()), "foreign server override directory")
            path.parent.rmdir()
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "restart", s.CURRENT_SERVER])
    checked(["systemctl", "stop", SOCKET])
    await_snapshot(False, plan["before"])
    plan["phase"] = "restored"
    save(root, plan)


def transition(action: str, root: Path) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load(root)
        if action == "restore":
            restore(root, plan)
        else:
            require(snapshot(plan["phase"] == "wired") == plan["before"], "protected status state changed")
        if action in ("restore", "cleanup"):
            require(plan["phase"] == "restored", "status wiring still present")
            for name, content in units(root).items():
                path = UNIT_ROOT / name
                if infrastructure.exists(path):
                    require(not path.is_symlink() and path.read_text() == content, "foreign status deadline unit")
            if (UNIT_ROOT / (root.name + "-deadline.timer")).exists():
                checked(["systemctl", "stop", root.name + "-deadline.timer"])
            for name in units(root):
                (UNIT_ROOT / name).unlink(missing_ok=True)
            checked(["systemctl", "daemon-reload"])
        if action == "cleanup":
            expected = {"controller.py", "plan.json", *HELPERS}
            require({path.name for path in root.iterdir()} == expected, "unexpected status guard contents")
            for path in root.iterdir():
                meta = path.lstat()
                require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1, "unsafe status cleanup file")
            for name in expected:
                (root / name).unlink()
            root.rmdir()
        return {"result": action, "guardRoot": str(root), "phase": plan["phase"], "statePreserved": True}
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "inspect", "restore", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    args = parser.parse_args()
    require(os.geteuid() == 0, "root orchestration required")
    result = prepare(args.assets) if args.action == "prepare" else transition(args.action, guard_root(args.guard_root))
    print(json.dumps(result, sort_keys=True))
    if result["result"] == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
