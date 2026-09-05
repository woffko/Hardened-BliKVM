#!/usr/bin/env python3
"""Guarded idle enablement of the installed updater; never activate a slot."""

from __future__ import annotations

import argparse
import fcntl
import grp
import os
from pathlib import Path
import pwd
import re
import secrets
import stat
import subprocess
import sys
import time
import json
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import upgrade_initialized_updater as upgrade
else:
    from tools import upgrade_initialized_updater as upgrade

s = upgrade.serving
require = upgrade.require
checked = upgrade.checked
PREFIX = "hardened-blikvm-updater-enable-"
TIMER = "hardened-blikvm-update-watchdog.timer"
WATCHDOG = "hardened-blikvm-update-watchdog.service"
SOCKET = "hardened-blikvm-update-confirm.socket"
RUNTIME = Path("/run/hardened-blikvm-update")
SOCKET_ROOT = Path("/run/hardened-blikvm-update-confirm")
RULE = Path("/etc/tmpfiles.d/hardened-blikvm-update.conf")
EXAMPLE = Path("/usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-update-tmpfiles.conf")
RULE_SHA = "93081586b0cd99105f743bed2d4d81c78ef687aa338a6541db417f2dc2a30588"
HELPERS = tuple(dict.fromkeys(("upgrade_initialized_updater.py", *upgrade.HELPERS)))


def exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def current_status() -> dict[str, object]:
    name = "blikvm-enabled-status-" + secrets.token_hex(10) + ".service"
    argv = ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + name,
            "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm", "--property=UMask=0077",
            "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict", "--property=RuntimeMaxSec=20",
            "--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
            "--property=LoadCredential=update-supervisor-config:" + str(s.bootstrap.CONFIG),
            str(s.bootstrap.SUPERVISOR), "--config", "/run/credentials/" + name + "/update-supervisor-config", "status"]
    return cast(dict[str, object], json.loads(checked(argv)))


def initial_status() -> None:
    result = current_status()
    for key, value in {"generation": 1, "floorSequence": 1, "activeSlotId": s.bootstrap.SLOT,
                       "confirmedSlotId": s.bootstrap.SLOT, "previousSlotId": None, "pending": None, "lastRollback": None}.items():
        require(result.get(key) == value, "initial update state changed")


def audit_enabled(*, check_initial: bool = True) -> None:
    for unit in (TIMER, SOCKET):
        require(s.enabled(unit) == "enabled" and s.property_value(unit, "ActiveState") == "active", "updater service not enabled/active")
    require(s.property_value(WATCHDOG, "Result") == "success" and s.property_value(WATCHDOG, "ExecMainStatus") == "0",
            "watchdog invocation failed")
    uid = pwd.getpwnam("hardened-blikvm-update").pw_uid
    gid = grp.getgrnam("hardened-blikvm").gr_gid
    for path, mode, socket_inode in ((RUNTIME, 0o700, False), (SOCKET_ROOT, 0o750, False),
                                     (SOCKET_ROOT / "confirm.sock", 0o620, True)):
        meta = path.lstat()
        require((stat.S_ISSOCK(meta.st_mode) if socket_inode else stat.S_ISDIR(meta.st_mode))
                and meta.st_uid == uid and meta.st_gid == gid and stat.S_IMODE(meta.st_mode) == mode,
                "unsafe updater runtime ownership/mode")
    require(not exists(RUNTIME / "health-token") and not exists(RUNTIME / "confirmed-apply-request.json"), "unexpected pending update")
    for unit in (s.CANDIDATE_SERVER, "hardened-blikvm-update-apply-candidate.service", "hardened-blikvm-update-apply-confirmed.service"):
        s.inactive(unit)
    active = checked(["systemctl", "list-units", "--all", "--plain", "--no-legend", "--state=active,activating,deactivating,reloading",
                      "hardened-blikvm-update-activate@*.service", "hardened-blikvm-update-confirm@*.service"])
    require(not active.strip(), "unexpected active update transaction")
    meta = RULE.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) == 0o644
            and upgrade.digest(RULE) == RULE_SHA, "tmpfiles configuration changed")
    if check_initial:
        initial_status()


def enabled_snapshot(*, expected_slot: str = s.bootstrap.SLOT, expected_floor: int = 1,
                     check_initial: bool = True, expected_supervisor_sha: str | None = None) -> dict[str, object]:
    require(re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", expected_slot) is not None
            and int(expected_slot.split("-", 2)[1]) == expected_floor, "invalid expected application binding")
    selected_sha = upgrade.SUPERVISOR_HASHES["new"] if expected_supervisor_sha is None else expected_supervisor_sha
    s.bootstrap.trusted_code_and_config(expected_supervisor_sha=selected_sha)
    audit_enabled(check_initial=check_initial)
    if not check_initial:
        status = current_status()
        require(status.get("floorSequence") == expected_floor and status.get("pending") is None
                and status.get("activeSlotId") == status.get("confirmedSlotId") == expected_slot, "operational application state changed")
    before = s.snapshot()
    for unit in (s.CURRENT_SERVER, s.NGINX):
        require(s.property_value(unit, "ActiveState") == "active" and s.enabled(unit) == "enabled", "serving service unavailable")
    s.inactive(s.OLD_SERVER)
    require(s.enabled(s.OLD_SERVER) == "disabled", "old server enabled")
    pid = s.property_value(s.CURRENT_SERVER, "MainPID")
    require(pid.isdecimal() and int(pid) > 0, "invalid serving PID")
    executable = Path("/proc") / pid / "exe"
    expected = upgrade.STATE_ROOTS[0] / expected_slot / "bin/bli-kvm-server"
    require(executable.resolve(strict=True) == expected and upgrade.digest(executable) == upgrade.digest(expected), "serving executable changed")
    require(bool(checked(["curl", "--noproxy", "*", "--insecure", "--silent", "--fail", "--max-time", "5", "https://127.0.0.1/api/health"])), "HTTPS unavailable")
    verified = {"result": "verified", "server": s.CURRENT_SERVER, "serverPid": pid,
                "executableSha256": upgrade.digest(executable), "nginxPidUnchanged": True, "accountTlsTrustPeersUnchanged": True}
    return {"runtime": before, "serving": verified, "state": upgrade.tree_fingerprint(upgrade.STATE_ROOTS), "nginxUnit": upgrade.digest(s.UNIT)}


def disabled_snapshot() -> dict[str, object]:
    require(not exists(RULE) and not exists(SOCKET_ROOT) and not exists(RUNTIME), "idle updater provisioning already exists")
    # Preserve the original generation-one provisioning contract independently
    # of later package-upgrade observers for an already enabled deployment.
    before = s.snapshot()
    verified = s.verify(before, True, expected_supervisor_sha=upgrade.SUPERVISOR_HASHES["new"])
    return {"runtime": before, "serving": verified, "state": upgrade.tree_fingerprint(upgrade.STATE_ROOTS), "nginxUnit": upgrade.digest(s.UNIT)}


def verify(root: Path, plan: dict[str, object], enabled: bool) -> None:
    upgrade.installed(root, "new")
    actual = enabled_snapshot() if enabled else disabled_snapshot()
    require(actual == plan["before"], "application state or serving runtime changed")


def disable(boot: bool) -> None:
    for unit in (TIMER, SOCKET):
        checked(["systemctl", "disable", unit])
        checked(["systemctl", "stop", "--no-block", unit] if boot else ["systemctl", "stop", unit])
    checked(["systemctl", "stop", "--no-block", WATCHDOG] if boot else ["systemctl", "stop", WATCHDOG])
    if exists(RULE):
        require(not RULE.is_symlink() and upgrade.digest(RULE) == RULE_SHA, "cannot remove changed tmpfiles rule")
        RULE.unlink()
        upgrade.sync_directory(RULE.parent)
    if not boot:
        uid = pwd.getpwnam("hardened-blikvm-update").pw_uid
        gid = grp.getgrnam("hardened-blikvm").gr_gid
        for path in (RUNTIME, SOCKET_ROOT):
            if exists(path):
                meta = path.lstat()
                require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == uid and meta.st_gid == gid
                        and not list(path.iterdir()), "refusing nonempty or unsafe updater runtime removal")
                path.rmdir()


def guard_root(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None, "invalid enablement guard")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700,
            "unsafe enablement guard")
    return root


def prepare(assets: Path) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")) and not list(upgrade.UNIT_ROOT.glob(PREFIX + "*")), "previous enablement guard remains")
    upgrade.installed(assets, "new")
    before = disabled_snapshot()
    require(upgrade.digest(EXAMPLE) == RULE_SHA, "packaged tmpfiles example changed")
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in ("new.deb", "bindings.json", *HELPERS):
        upgrade.new_file(root / name, (assets / name).read_bytes())
    upgrade.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "phase": "prepared", "before": before}
    upgrade.save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        upgrade.arm(root)
        upgrade.new_file(RULE, EXAMPLE.read_bytes(), 0o644)
        checked(["systemd-tmpfiles", "--create", str(RULE)])
        checked(["systemctl", "enable", "--now", SOCKET, TIMER])
        timestamps: set[str] = set()
        for _ in range(6):
            time.sleep(2.1)
            verify(root, plan, True)
            timestamps.add(s.property_value(WATCHDOG, "ExecMainStartTimestampMonotonic"))
        require(len(timestamps) >= 4 and all(value.isdecimal() and int(value) > 0 for value in timestamps), "too few real watchdog invocations")
        plan["phase"] = "applied"
        upgrade.save(root, plan)
        return {"result": "prepared", "guardRoot": str(root), "stateAndServingPreserved": True, "watchdogInvocations": len(timestamps)}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)


def transition(action: str, root: Path, boot: bool) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = upgrade.load_plan(root)
        if action == "rollback":
            if plan["phase"] != "committed":
                disable(boot)
                if not boot:
                    verify(root, plan, False)
                else:
                    before = cast(dict[str, object], plan["before"])
                    require(upgrade.tree_fingerprint(upgrade.STATE_ROOTS) == before["state"], "persistent state changed during boot recovery")
                plan["phase"] = "rolled_back"
                upgrade.save(root, plan)
            elif not boot:
                verify(root, plan, True)
            upgrade.disarm(root, boot)
        elif action == "commit":
            require(plan["phase"] in ("applied", "committed"), "enablement not ready")
            verify(root, plan, True)
            plan["phase"] = "committed"
            upgrade.save(root, plan)
            upgrade.disarm(root, False)
        else:
            if plan["phase"] == "rolled_back":
                disable(False)
            verify(root, plan, plan["phase"] in ("applied", "committed"))
            if action == "cleanup":
                require(plan["phase"] in ("committed", "rolled_back"), "enablement guard is pending")
                upgrade.disarm(root, False)
                expected = {"new.deb", "bindings.json", "controller.py", "plan.json", *HELPERS}
                require({p.name for p in root.iterdir()} == expected, "unexpected enablement guard contents")
                for path in root.iterdir():
                    require(path.is_file() and not path.is_symlink() and path.stat().st_uid == 0, "unsafe enablement artifact")
                for name in expected:
                    (root / name).unlink()
                root.rmdir()
                upgrade.sync_directory(root.parent)
        return {"result": action, "phase": plan["phase"], "guardRoot": str(root), "stateAndServingPreserved": not boot,
                "bootRecovery": boot}
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "rollback", "commit", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    parser.add_argument("--variant", choices=("old", "new"), default="old")
    parser.add_argument("--boot", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0 and (not args.boot or args.action == "rollback"), "invalid execution mode")
    if args.action == "preflight":
        upgrade.installed(args.assets, "new")
        result = {"result": "preflight", "before": enabled_snapshot() if args.variant == "new" else disabled_snapshot()}
    elif args.action == "prepare":
        result = prepare(args.assets)
    else:
        result = transition(args.action, guard_root(args.guard_root), args.boot)
    print(json.dumps(result, sort_keys=True))
    if result.get("result") == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
