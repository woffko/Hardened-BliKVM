#!/usr/bin/env python3
"""Prepare exactly one bound unconfirmed candidate for a software reboot test."""

from __future__ import annotations

import argparse
import grp
import hashlib
import json
import os
from pathlib import Path
import re
import stat
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
BUNDLE_SHA = "9b8fa7781e9b453fa96cef385aa135e7e35d127276dbe22fe87364e4ead37bee"
CONFIRMED = "app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53"
CANDIDATE = "app-00000000000000000003-4bef35dff43b8c06b7a2c8965acb0c9b106e605881fcc955fe24bbc5c3c03d6a"
DROPIN_ROOT = Path("/run/systemd/system/hardened-blikvm-server-candidate.service.d")
DROPIN = DROPIN_ROOT / "90-interrupted-health.conf"
DROPIN_BYTES = b"[Service]\nExecStartPost=\nExecStartPost=/usr/bin/sleep 90\n"


def validate_assets(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(r"hardened-blikvm-reboot-probe-[a-f0-9]{24}", root.name) is not None,
            "invalid reboot probe namespace")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700,
            "unsafe reboot probe root")


def source_root(root: Path) -> Path:
    return Path("/tmp") / (root.name + "-input")


def cleanup(root: Path) -> None:
    validate_assets(root)
    marker = root / "interruption-owned"
    owns_override = infrastructure.exists(marker)
    if owns_override:
        meta = marker.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) == 0o600
                and marker.read_bytes() == hashlib.sha256(root.name.encode() + DROPIN_BYTES).hexdigest().encode(), "unsafe interruption ownership marker")
        if infrastructure.exists(DROPIN):
            meta = DROPIN.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and DROPIN.read_bytes() == DROPIN_BYTES,
                    "interruption override changed")
            DROPIN.unlink()
        if infrastructure.exists(DROPIN_ROOT):
            require(not DROPIN_ROOT.is_symlink() and DROPIN_ROOT.stat().st_uid == 0 and not list(DROPIN_ROOT.iterdir()), "unrelated candidate override remains")
            DROPIN_ROOT.rmdir()
        checked(["systemctl", "daemon-reload"])
    source = source_root(root)
    if infrastructure.exists(source):
        meta = source.lstat()
        require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o750
                and {p.name for p in source.iterdir()} == {"application.tar"}, "unsafe interruption source")
        archive = source / "application.tar"
        require(not archive.is_symlink() and archive.stat().st_nlink == 1 and u.digest(archive) == BUNDLE_SHA, "interruption source changed")
        archive.unlink()
        source.rmdir()
    # If reboot was rejected or its response was lost, the installed watchdog
    # still owns rollback. Wait for it; never force a journal/pointer rewrite.
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        status = infrastructure.current_status()
        if status.get("pending") is None:
            require(status.get("floorSequence") == 2 and status.get("activeSlotId") == status.get("confirmedSlotId") == CONFIRMED,
                    "unexpected authority after interruption")
            if owns_override:
                marker.unlink()
            return
        time.sleep(1)
    raise ValueError("watchdog did not finish recovery after interruption")


def prepare(root: Path) -> dict[str, object]:
    validate_assets(root)
    u.installed(root, "new")
    infrastructure.audit_enabled(check_initial=False)
    require(not infrastructure.exists(DROPIN_ROOT)
            and not infrastructure.exists(Path("/etc/systemd/system/hardened-blikvm-server-candidate.service.d")), "candidate override already exists")
    original = infrastructure.current_status()
    require(original.get("floorSequence") == 2 and original.get("pending") is None
            and original.get("activeSlotId") == original.get("confirmedSlotId") == CONFIRMED, "expected confirmed sequence two")
    archive = root / "application.tar"
    require(u.digest(archive) == BUNDLE_SHA, "interrupted candidate binding mismatch")
    source = source_root(root)
    source.mkdir(mode=0o750)
    source.chmod(0o750)
    group = grp.getgrnam("hardened-blikvm").gr_gid
    os.chown(source, 0, group)
    u.new_file(source / "application.tar", archive.read_bytes(), 0o640)
    os.chown(source / "application.tar", 0, group)
    name = root.name + "-stage.service"
    argv = ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + name,
            "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm", "--property=UMask=0077",
            "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict", "--property=RuntimeMaxSec=30",
            "--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
            "--property=LoadCredential=update-supervisor-config:" + str(s.bootstrap.CONFIG),
            str(s.bootstrap.SUPERVISOR), "--config", "/run/credentials/" + name + "/update-supervisor-config",
            "stage", str(source / "application.tar")]
    staged = cast(dict[str, object], json.loads(checked(argv)))
    require(staged.get("slotId") == CANDIDATE and staged.get("sequence") == 3
            and staged.get("result") in ("published", "already_present"), "unexpected candidate staging result")
    require(infrastructure.current_status() == original, "staging changed active application state")
    retained_tree = u.tree_fingerprint((u.STATE_ROOTS[0] / CONFIRMED,))
    candidate_tree = u.tree_fingerprint((u.STATE_ROOTS[0] / CANDIDATE,))
    u.new_file(root / "interruption-owned", hashlib.sha256(root.name.encode() + DROPIN_BYTES).hexdigest().encode())
    DROPIN_ROOT.mkdir(mode=0o755)
    u.new_file(DROPIN, DROPIN_BYTES, 0o644)
    checked(["systemctl", "daemon-reload"])
    post = s.property_value(s.CANDIDATE_SERVER, "ExecStartPost")
    require("/usr/bin/sleep 90" in post and "bli-kvm-update-health-reporter" not in post, "health delay override not loaded")
    checked(["systemctl", "start", "hardened-blikvm-update-activate@" + CANDIDATE + ".service"])
    status = infrastructure.current_status()
    pending_value = status.get("pending")
    require(isinstance(pending_value, dict), "candidate has no pending transaction")
    pending = cast(dict[str, object], pending_value)
    require(pending.get("targetSlotId") == CANDIDATE
            and pending.get("previousSlotId") == CONFIRMED and pending.get("phase") == "awaiting_health"
            and status.get("floorSequence") == 2, "candidate is not awaiting health")
    remaining = cast(int, pending["deadlineMonotonicMs"]) - time.clock_gettime_ns(time.CLOCK_BOOTTIME) // 1_000_000
    require(remaining >= 30000, "insufficient time to request reboot")
    return {"result": "pending", "status": status, "remainingHealthMs": remaining,
            "bootIdSha256": hashlib.sha256(Path("/proc/sys/kernel/random/boot_id").read_bytes()).hexdigest(),
            "confirmedTree": retained_tree, "candidateTree": candidate_tree, "credentialReadOrEmitted": False}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--cleanup", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0, "root orchestration required")
    if args.cleanup:
        cleanup(args.assets)
        result = {"result": "cleaned"}
    else:
        result = prepare(args.assets)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
