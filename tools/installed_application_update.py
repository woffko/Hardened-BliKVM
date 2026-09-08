#!/usr/bin/env python3
"""Bound installed sequence-two activation with authoritative-state recovery."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import fcntl
import grp
import hashlib
import json
import os
import pwd
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
import tarfile
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


@dataclass(frozen=True)
class UpdateProfile:
    name: str
    bundle_sha: str
    bundle_bytes: int
    source_sequence: int
    source_slot: str
    target_sequence: int
    target_slot: str
    persistent_status: bool = False


PROFILES = {
    "seq1-to-seq2": UpdateProfile("seq1-to-seq2", BUNDLE_SHA, 8499200, 1, SLOT_ONE, 2, SLOT_TWO),
    "seq2-to-seq4-status": UpdateProfile(
        "seq2-to-seq4-status", "4ae41c49d2a4dcc0cab041516e3591d89d2da4342484123467f70e19d19a7b12",
        8591360, 2, SLOT_TWO, 4,
        "app-00000000000000000004-768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c"),
    "seq4-to-seq5-notification": UpdateProfile(
        "seq4-to-seq5-notification", "618b1068086a04aa75f44804b58bcf0dac05d62c11553af74ebfe5cfdf893093",
        8591360, 4, "app-00000000000000000004-768b33ac3a022f4a216ae1db4a08e7926802eabb4842b0de1dc80c74aa70095c", 5,
        "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc"),
    "seq5-to-seq6-mjpeg": UpdateProfile(
        "seq5-to-seq6-mjpeg", "4f5a307e2d1014de360dc2cdb75cb5186a60511cfd772f3be0b98b8aff9a6036",
        8601600, 5, "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc", 6,
        "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac", True),
}
active_profile = PROFILES["seq1-to-seq2"]


def select_profile(name: str) -> None:
    global active_profile
    require(name in PROFILES, "unknown application transition profile")
    active_profile = PROFILES[name]


def profile_slots() -> dict[int, str]:
    return {active_profile.source_sequence: active_profile.source_slot, active_profile.target_sequence: active_profile.target_slot}


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
    require(value.get("schemaVersion") == 1 and value.get("bundleSha256") == active_profile.bundle_sha
            and value.get("profile", "seq1-to-seq2") == active_profile.name
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
    require(not archive.is_symlink() and archive.stat().st_nlink == 1 and u.digest(archive) == active_profile.bundle_sha, "staged source changed")
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
    original = u.STATE_ROOTS[0] / active_profile.source_slot
    require(u.tree_fingerprint((original,)) == plan["originalSlot"], "original signed slot changed")


def validate_status_wiring(*, allow_rehearsal: bool = False) -> None:
    """Preserve the accepted status wiring independently of application floor.

    The old status snapshot binds sequence five and cannot validate a new slot.
    Here only its exact transport/override contract is reused; stable() checks
    the authoritative application slot against the selected transition profile.
    """
    if __package__:
        from tools import status_api_wiring as common
    else:
        import status_api_wiring as common
    for unit in common.SERVERS:
        directory = Path("/etc/systemd/system") / (unit + ".d")
        meta = directory.lstat()
        require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o755
                and {child.name for child in directory.iterdir()} == {common.DROPIN_NAME}, "unexpected persistent status override directory")
        path = directory / common.DROPIN_NAME
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == common.DROPIN_BYTES,
                "persistent status override changed")
        runtime = Path("/run/systemd/system") / (unit + ".d")
        if infrastructure.exists(runtime):
            meta = runtime.lstat()
            require(allow_rehearsal and runtime == DROPIN_ROOT and stat.S_ISDIR(meta.st_mode)
                    and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o755
                    and {child.name for child in runtime.iterdir()} == {DROPIN.name}, "unapproved runtime override")
            meta = DROPIN.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                    and stat.S_IMODE(meta.st_mode) == 0o644 and DROPIN.read_bytes() == DROPIN_BYTES,
                    "rehearsal override changed")
    common.runtime_environment(True)
    for unit, digest in common.UNIT_HASHES.items():
        require(u.digest(Path("/lib/systemd/system") / unit) == digest, "status transport unit changed")
    require(s.enabled(common.SOCKET) == "disabled" and s.property_value(common.SOCKET, "ActiveState") == "active",
            "persistent status socket changed")
    meta = common.ENDPOINT.lstat()
    require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
            and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
            and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid, "unsafe status socket metadata")


def stable(root: Path, plan: dict[str, object], expected_floor: int | None = None) -> dict[str, object]:
    u.installed(root, "new")
    infrastructure.audit_enabled(check_initial=False)
    if active_profile.persistent_status:
        validate_status_wiring(allow_rehearsal=plan.get("phase") == "staged")
    status = infrastructure.current_status()
    floor = status.get("floorSequence")
    require(isinstance(floor, int) and floor in profile_slots() and (expected_floor is None or floor == expected_floor), "unexpected confirmed floor")
    require(cast(int, floor) >= minimum_recovery_floor(plan), "confirmed floor is below previous observation")
    slot = profile_slots()[cast(int, floor)]
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
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py recover --guard-root {root} --profile {active_profile.name}"
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


def verify_staged_payload(archive_path: Path, slot: Path) -> None:
    require(archive_path.stat().st_size == active_profile.bundle_bytes and u.digest(archive_path) == active_profile.bundle_sha,
            "staged archive binding changed")
    expected: set[str] = set()
    with tarfile.open(archive_path, mode="r:") as archive:
        for member in archive:
            if not member.name.startswith("payload/") or member.isdir():
                continue
            relative = Path(member.name).relative_to("payload")
            require(member.isfile() and relative.parts[0] in ("bin", "libexec", "share")
                    and ".." not in relative.parts and str(relative) not in expected, "unsafe payload entry")
            expected.add(str(relative))
            stream = archive.extractfile(member)
            require(stream is not None and 0 < member.size <= 32 * 1024 * 1024, "invalid payload length")
            if stream is None:
                raise ValueError("missing payload stream")
            digest = hashlib.sha256(stream.read()).hexdigest()
            target = slot / relative
            meta = target.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1 and meta.st_size == member.size
                    and stat.S_IMODE(meta.st_mode) == member.mode & ~0o222
                    and u.digest(target) == digest, "staged payload differs from signed archive")
    actual = {str(path.relative_to(slot)) for name in ("bin", "libexec", "share")
              for path in (slot / name).rglob("*") if not path.is_dir()}
    require(bool(expected) and actual == expected, "staged payload file set differs")


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
    require(result.get("slotId") == active_profile.target_slot and result.get("sequence") == active_profile.target_sequence
            and result.get("result") in ("published", "already_present"), "unexpected staged application")
    verify_staged_payload(source / "application.tar", u.STATE_ROOTS[0] / active_profile.target_slot)
    remove_source(root)


def activate_slot() -> None:
    unit = "hardened-blikvm-update-activate@" + active_profile.target_slot + ".service"
    checked(["systemctl", "start", unit])


def prepare(assets: Path) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")) and not list(u.UNIT_ROOT.glob(PREFIX + "*")), "previous application guard remains")
    require(not infrastructure.exists(DROPIN_ROOT), "candidate runtime override already exists")
    if active_profile.persistent_status:
        validate_status_wiring()
    else:
        require(not infrastructure.exists(Path("/etc/systemd/system/hardened-blikvm-server-candidate.service.d")), "candidate override already exists")
    u.installed(assets, "new")
    s.bootstrap.trusted_code_and_config(expected_supervisor_sha=u.SUPERVISOR_HASHES["new"])
    infrastructure.audit_enabled(check_initial=False)
    require(u.digest(assets / "application.tar") == active_profile.bundle_sha, "application bundle hash mismatch")
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in ("new.deb", "bindings.json", *HELPERS):
        u.new_file(root / name, (assets / name).read_bytes())
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "bundleSha256": active_profile.bundle_sha, "profile": active_profile.name, "phase": "prepared",
                               "runtime": s.snapshot(), "nginxUnitSha256": u.digest(s.UNIT),
                               "originalSlot": u.tree_fingerprint((u.STATE_ROOTS[0] / active_profile.source_slot,))}
    save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        stable(root, plan, active_profile.source_sequence)
        arm(root)
        source = source_root(root)
        source.mkdir(mode=0o750)
        source.chmod(0o750)
        group = grp.getgrnam("hardened-blikvm").gr_gid
        os.chown(source, 0, group)
        u.new_file(source / "application.tar", (assets / "application.tar").read_bytes(), 0o640)
        os.chown(source / "application.tar", 0, group)
        stage(root)
        stable(root, plan, active_profile.source_sequence)
        plan["phase"] = "staged"
        save(root, plan)
        DROPIN_ROOT.mkdir(mode=0o755)
        u.new_file(DROPIN, DROPIN_BYTES, 0o644)
        checked(["systemctl", "daemon-reload"])
        activate_slot()
        verified = await_stable(root, plan, active_profile.source_sequence)
        require(cast(dict[str, object], verified["status"]).get("lastRollback") == "health_timeout", "timeout rehearsal failed")
        remove_override()
        plan["phase"] = "rehearsed"
        save(root, plan)
        return {"result": "rehearsed", "guardRoot": str(root), "verification": verified, "browserVerificationRequired": True}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)


def minimum_recovery_floor(plan: dict[str, object]) -> int:
    observed = plan.get("observedFloor", active_profile.source_sequence)
    require(type(observed) is int and observed in profile_slots(), "invalid observed recovery floor")
    confirmed = active_profile.target_sequence if plan["phase"] in ("confirmed", "committed-update") else active_profile.source_sequence
    return max(cast(int, observed), confirmed)


def recover(root: Path, plan: dict[str, object], boot: bool) -> dict[str, object]:
    minimum = minimum_recovery_floor(plan)
    remove_override()
    remove_source(root)
    if boot:
        # Ordered after the installed watchdog. Do not wait for GUI/apply jobs
        # here: they are deliberately ordered after this boot handoff.
        status = infrastructure.current_status()
        require(status.get("pending") is None and status.get("floorSequence") in profile_slots(), "boot manager recovery incomplete")
        require(status.get("activeSlotId") == status.get("confirmedSlotId") == profile_slots()[cast(int, status["floorSequence"])], "boot pointer authority mismatch")
        verified: dict[str, object] = {"status": status, "browserVerificationRequired": True}
    else:
        checked(["systemctl", "start", infrastructure.WATCHDOG])
        if infrastructure.current_status().get("pending") is None:
            checked(["systemctl", "start", s.CURRENT_SERVER])
        verified = await_stable(root, plan)
    floor = cast(dict[str, object], verified["status"]).get("floorSequence")
    require(type(floor) is int and floor in profile_slots() and floor >= minimum,
            "recovery regressed an already observed confirmed floor")
    plan["observedFloor"] = floor
    plan["phase"] = "recovered"
    save(root, plan)
    disarm(root, boot)
    return {"result": "recovered", "guardRoot": str(root), "verification": verified, "journalRestoredFromBackup": False}


def transition(action: str, root: Path, boot: bool, browser_phase: str | None = None) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load(root)
        if action == "recover":
            return recover(root, plan, boot)
        if action == "activate":
            require(plan["phase"] == "rehearsed", "timeout rehearsal has not passed")
            stable(root, plan, active_profile.source_sequence)
            require(not infrastructure.exists(DROPIN_ROOT), "candidate override remains")
            activate_slot()
            verified = await_stable(root, plan, active_profile.target_sequence)
            plan["phase"] = "confirmed"
            save(root, plan)
        else:
            expected = active_profile.source_sequence if plan["phase"] in ("rehearsed", "committed-rehearsal") else active_profile.target_sequence if plan["phase"] in ("confirmed", "committed-update") else None
            verified = stable(root, plan, expected)
            if action == "commit":
                require(plan["phase"] in ("rehearsed", "confirmed", "committed-rehearsal", "committed-update"), "transaction not ready")
                required_browser = "rehearsal" if plan["phase"] in ("rehearsed", "committed-rehearsal") else "confirmed"
                require(browser_phase == required_browser, "browser receipt phase differs from device transaction")
                plan["phase"] = "committed-rehearsal" if cast(dict[str, object], verified["status"])["floorSequence"] == active_profile.source_sequence else "committed-update"
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
    parser.add_argument("--profile", choices=tuple(PROFILES), default="seq1-to-seq2")
    parser.add_argument("--browser-phase", choices=("rehearsal", "confirmed"))
    args = parser.parse_args()
    select_profile(args.profile)
    require(os.geteuid() == 0 and (not args.boot or args.action == "recover"), "invalid execution mode")
    result = prepare(args.assets) if args.action == "prepare" else transition(args.action, plan_path(args.guard_root), args.boot, args.browser_phase)
    print(json.dumps(result, sort_keys=True))
    if result.get("result") == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
