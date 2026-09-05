#!/usr/bin/env python3
"""Bound installed sequence-two activation with authoritative-state recovery."""

from __future__ import annotations

import argparse
import fcntl
import grp
import json
import os
from pathlib import Path
import re
import secrets
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
PREFIX = "hardened-blikvm-application-update-"
BUNDLE_SHA = "fe3680eb1607b2ec5c957916b0b343c55390171e873ad978f0f521b0c2d42778"
SLOT_ONE = s.bootstrap.SLOT
SLOT_TWO = "app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53"
SLOTS = {1: SLOT_ONE, 2: SLOT_TWO}
DROPIN_ROOT = Path("/run/systemd/system/hardened-blikvm-server-candidate.service.d")
DROPIN = DROPIN_ROOT / "90-health-timeout-rehearsal.conf"
DROPIN_BYTES = b"[Service]\nExecStartPost=\nExecStartPost=/usr/bin/false\n"
HELPERS = tuple(dict.fromkeys(("enable_update_services.py", *infrastructure.HELPERS)))


def plan_path(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None, "invalid update guard")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0
            and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe update guard")
    return root


def load(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe update marker")
    value = cast(dict[str, object], json.loads(path.read_bytes()))
    require(value.get("schemaVersion") == 1 and value.get("bundleSha256") == BUNDLE_SHA
            and value.get("phase") in ("prepared", "staged", "rehearsed", "confirmed", "recovered", "committed-rehearsal", "committed-update"), "invalid update marker")
    return value


def save(root: Path, plan: dict[str, object]) -> None:
    part = root / "plan.json.part"
    if infrastructure.exists(part):
        meta = part.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe interrupted marker")
        load(root)
        partial = part.read_bytes()
        require(any(json.dumps({**plan, "phase": phase}, sort_keys=True).encode().startswith(partial)
                    for phase in ("prepared", "staged", "rehearsed", "confirmed", "recovered", "committed-rehearsal", "committed-update")),
                "unknown interrupted marker")
        part.unlink()
        u.sync_directory(root)
    u.new_file(part, json.dumps(plan, sort_keys=True).encode())
    os.replace(part, root / "plan.json")
    u.sync_directory(root)


def source_root(root: Path) -> Path:
    return Path("/tmp") / (root.name + "-source")


def remove_source(root: Path) -> None:
    source = source_root(root)
    if not infrastructure.exists(source):
        return
    meta = source.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o750
            and {p.name for p in source.iterdir()} == {"application.tar"}, "unsafe staged source")
    archive = source / "application.tar"
    require(not archive.is_symlink() and archive.stat().st_nlink == 1 and u.digest(archive) == BUNDLE_SHA, "staged source changed")
    archive.unlink()
    source.rmdir()


def remove_override() -> None:
    if infrastructure.exists(DROPIN):
        meta = DROPIN.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                and DROPIN.read_bytes() == DROPIN_BYTES, "candidate override changed")
        DROPIN.unlink()
        u.sync_directory(DROPIN_ROOT)
    if infrastructure.exists(DROPIN_ROOT):
        require(not DROPIN_ROOT.is_symlink() and DROPIN_ROOT.stat().st_uid == 0 and not list(DROPIN_ROOT.iterdir()), "unrelated candidate override present")
        DROPIN_ROOT.rmdir()
    checked(["systemctl", "daemon-reload"])


def preserved(plan: dict[str, object]) -> None:
    require(s.snapshot() == plan["runtime"], "accounts, trust, TLS, Nginx or peers changed")
    require(u.digest(s.UNIT) == plan["nginxUnitSha256"], "local Nginx unit changed")
    original = u.STATE_ROOTS[0] / SLOT_ONE
    require(u.tree_fingerprint((original,)) == plan["originalSlot"], "original signed slot changed")


def stable(root: Path, plan: dict[str, object], expected_floor: int | None = None) -> dict[str, object]:
    u.installed(root, "new")
    infrastructure.audit_enabled(check_initial=False)
    status = infrastructure.current_status()
    floor = status.get("floorSequence")
    require(isinstance(floor, int) and floor in SLOTS and (expected_floor is None or floor == expected_floor), "unexpected confirmed floor")
    slot = SLOTS[cast(int, floor)]
    require(status.get("pending") is None and status.get("activeSlotId") == slot and status.get("confirmedSlotId") == slot,
            "update has not converged")
    require(os.readlink(u.STATE_ROOTS[0] / "current") == slot, "authoritative pointer mismatch")
    require(s.property_value(s.CURRENT_SERVER, "ActiveState") == "active" and s.enabled(s.CURRENT_SERVER) == "enabled", "confirmed service unavailable")
    s.inactive(s.OLD_SERVER)
    require(s.enabled(s.OLD_SERVER) == "disabled", "old server unexpectedly enabled")
    pid = s.property_value(s.CURRENT_SERVER, "MainPID")
    require(pid.isdecimal() and int(pid) > 0 and (Path("/proc") / pid / "exe").resolve(strict=True) == u.STATE_ROOTS[0] / slot / "bin/bli-kvm-server",
            "wrong serving executable")
    require(bool(checked(["curl", "--noproxy", "*", "--insecure", "--silent", "--fail", "--max-time", "5", "https://127.0.0.1/api/health"])), "HTTPS health unavailable")
    preserved(plan)
    return {"status": status, "serverPid": pid, "accountTlsTrustPeersPreserved": True}


def await_stable(root: Path, plan: dict[str, object], expected_floor: int | None = None) -> dict[str, object]:
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        try:
            return stable(root, plan, expected_floor)
        except (OSError, ValueError, subprocess.SubprocessError):
            time.sleep(1)
    raise ValueError("installed update did not converge within recovery window")


def guard_units(root: Path) -> dict[str, str]:
    service = "[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=150\nNoNewPrivileges=yes\nProtectHome=yes\n"
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py recover --guard-root {root}"
    return {
        root.name + "-boot.service": ("[Unit]\nDescription=Application transaction boot handoff\nDefaultDependencies=no\n"
            f"Wants={infrastructure.WATCHDOG}\nAfter=basic.target {infrastructure.WATCHDOG}\n"
            f"Before={s.CURRENT_SERVER} {s.CANDIDATE_SERVER}\n" + service + command + " --boot\n[Install]\nWantedBy=multi-user.target\n"),
        root.name + "-recovery.service": "[Unit]\nDescription=Application transaction deadline recovery\n" + service + command + "\n",
        root.name + "-deadline.timer": f"[Timer]\nOnActiveSec=600\nUnit={root.name}-recovery.service\n[Install]\nWantedBy=timers.target\n",
    }


def arm(root: Path) -> None:
    for name, content in guard_units(root).items():
        u.new_file(u.UNIT_ROOT / name, content.encode(), 0o644)
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "enable", root.name + "-boot.service"])
    checked(["systemctl", "enable", "--now", root.name + "-deadline.timer"])
    checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"])
    os.sync()


def disarm(root: Path, boot: bool = False) -> None:
    for name, content in guard_units(root).items():
        path = u.UNIT_ROOT / name
        if infrastructure.exists(path):
            require(not path.is_symlink() and path.read_bytes() == content.encode(), "transaction guard unit changed")
    timer = root.name + "-deadline.timer"
    if (u.UNIT_ROOT / timer).exists():
        checked(["systemctl", "stop", "--no-block", timer] if boot else ["systemctl", "stop", timer])
        checked(["systemctl", "disable", timer])
    if (u.UNIT_ROOT / (root.name + "-boot.service")).exists():
        checked(["systemctl", "disable", root.name + "-boot.service"])
    for name in guard_units(root):
        (u.UNIT_ROOT / name).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    u.sync_directory(u.UNIT_ROOT)


def stage(root: Path) -> None:
    source = source_root(root)
    name = "blikvm-production-stage-" + secrets.token_hex(10) + ".service"
    command = ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + name,
               "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm", "--property=UMask=0077",
               "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict", "--property=RuntimeMaxSec=30",
               "--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
               "--property=LoadCredential=update-supervisor-config:" + str(s.bootstrap.CONFIG),
               str(s.bootstrap.SUPERVISOR), "--config", "/run/credentials/" + name + "/update-supervisor-config",
               "stage", str(source / "application.tar")]
    result = cast(dict[str, object], json.loads(checked(command)))
    require(result.get("slotId") == SLOT_TWO and result.get("sequence") == 2
            and result.get("result") in ("published", "already_present"), "unexpected staged application")
    original = u.STATE_ROOTS[0] / SLOT_ONE
    new = u.STATE_ROOTS[0] / SLOT_TWO
    for path in original.rglob("*"):
        if path.is_file() and path.name not in ("manifest.json", "manifest.sig"):
            relative = path.relative_to(original)
            # Manifest/signature storage names are checked by the Rust manager;
            # compare only actual executable/web payload roots here.
            if relative.parts[0] in ("bin", "libexec", "share"):
                require(u.digest(path) == u.digest(new / relative), "sequence-two payload differs from serving application")
    remove_source(root)


def activate_slot() -> None:
    unit = "hardened-blikvm-update-activate@" + SLOT_TWO + ".service"
    checked(["systemctl", "start", unit])


def prepare(assets: Path) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")) and not list(u.UNIT_ROOT.glob(PREFIX + "*")), "previous application guard remains")
    require(not infrastructure.exists(DROPIN_ROOT)
            and not infrastructure.exists(Path("/etc/systemd/system/hardened-blikvm-server-candidate.service.d")), "candidate override already exists")
    u.installed(assets, "new")
    s.bootstrap.trusted_code_and_config(expected_supervisor_sha=u.SUPERVISOR_HASHES["new"])
    infrastructure.audit_enabled(check_initial=False)
    require(u.digest(assets / "application.tar") == BUNDLE_SHA, "application bundle hash mismatch")
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in ("new.deb", "bindings.json", *HELPERS):
        u.new_file(root / name, (assets / name).read_bytes())
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "bundleSha256": BUNDLE_SHA, "phase": "prepared",
                               "runtime": s.snapshot(), "nginxUnitSha256": u.digest(s.UNIT),
                               "originalSlot": u.tree_fingerprint((u.STATE_ROOTS[0] / SLOT_ONE,))}
    save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        stable(root, plan, 1)
        arm(root)
        source = source_root(root)
        source.mkdir(mode=0o750)
        source.chmod(0o750)
        group = grp.getgrnam("hardened-blikvm").gr_gid
        os.chown(source, 0, group)
        u.new_file(source / "application.tar", (assets / "application.tar").read_bytes(), 0o640)
        os.chown(source / "application.tar", 0, group)
        stage(root)
        stable(root, plan, 1)
        plan["phase"] = "staged"
        save(root, plan)
        DROPIN_ROOT.mkdir(mode=0o755)
        u.new_file(DROPIN, DROPIN_BYTES, 0o644)
        checked(["systemctl", "daemon-reload"])
        activate_slot()
        verified = await_stable(root, plan, 1)
        require(cast(dict[str, object], verified["status"]).get("lastRollback") == "health_timeout", "timeout rehearsal failed")
        remove_override()
        plan["phase"] = "rehearsed"
        save(root, plan)
        return {"result": "rehearsed", "guardRoot": str(root), "verification": verified, "browserVerificationRequired": True}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)


def recover(root: Path, plan: dict[str, object], boot: bool) -> dict[str, object]:
    remove_override()
    remove_source(root)
    if boot:
        # Ordered after the installed watchdog. Do not wait for GUI/apply jobs
        # here: they are deliberately ordered after this boot handoff.
        status = infrastructure.current_status()
        require(status.get("pending") is None and status.get("floorSequence") in SLOTS, "boot manager recovery incomplete")
        require(status.get("activeSlotId") == status.get("confirmedSlotId") == SLOTS[cast(int, status["floorSequence"])], "boot pointer authority mismatch")
        verified: dict[str, object] = {"status": status, "browserVerificationRequired": True}
    else:
        checked(["systemctl", "start", infrastructure.WATCHDOG])
        if infrastructure.current_status().get("pending") is None:
            checked(["systemctl", "start", s.CURRENT_SERVER])
        verified = await_stable(root, plan)
    plan["phase"] = "recovered"
    save(root, plan)
    disarm(root, boot)
    return {"result": "recovered", "guardRoot": str(root), "verification": verified, "journalRestoredFromBackup": False}


def transition(action: str, root: Path, boot: bool) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load(root)
        if action == "recover":
            return recover(root, plan, boot)
        if action == "activate":
            require(plan["phase"] == "rehearsed", "timeout rehearsal has not passed")
            stable(root, plan, 1)
            require(not infrastructure.exists(DROPIN_ROOT), "candidate override remains")
            activate_slot()
            verified = await_stable(root, plan, 2)
            plan["phase"] = "confirmed"
            save(root, plan)
        else:
            expected = 1 if plan["phase"] in ("rehearsed", "committed-rehearsal") else 2 if plan["phase"] in ("confirmed", "committed-update") else None
            verified = stable(root, plan, expected)
            if action == "commit":
                require(plan["phase"] in ("rehearsed", "confirmed", "committed-rehearsal", "committed-update"), "transaction not ready")
                plan["phase"] = "committed-rehearsal" if cast(dict[str, object], verified["status"])["floorSequence"] == 1 else "committed-update"
                save(root, plan)
                disarm(root)
            elif action == "cleanup":
                require(plan["phase"] in ("committed-rehearsal", "committed-update", "recovered"), "transaction still guarded")
                remove_override()
                remove_source(root)
                disarm(root)
                expected_files = {"controller.py", "plan.json", "new.deb", "bindings.json", *HELPERS}
                require({p.name for p in root.iterdir()} == expected_files, "unexpected application guard contents")
                for path in root.iterdir():
                    require(path.is_file() and not path.is_symlink() and path.stat().st_uid == 0, "unsafe application guard file")
                for name in expected_files:
                    (root / name).unlink()
                root.rmdir()
                u.sync_directory(root.parent)
        return {"result": action, "phase": plan["phase"], "guardRoot": str(root), "verification": verified}
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "inspect", "activate", "recover", "commit", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    parser.add_argument("--boot", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0 and (not args.boot or args.action == "recover"), "invalid execution mode")
    result = prepare(args.assets) if args.action == "prepare" else transition(args.action, plan_path(args.guard_root), args.boot)
    print(json.dumps(result, sort_keys=True))
    if result.get("result") == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
