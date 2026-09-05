#!/usr/bin/env python3
"""Root orchestration of the first signed application slot; no activation."""

from __future__ import annotations

import argparse
import fcntl
import grp
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import stat
import subprocess
import sys
import tarfile
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    from application_bootstrap_roots import BootstrapRoots, sync_directory, rename_owned, identity
else:
    from tools.application_bootstrap_roots import BootstrapRoots, sync_directory, rename_owned, identity


CONFIG = Path("/etc/hardened-blikvm/update-supervisor.json")
SUPERVISOR = Path("/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor")
PARENT = Path("/var/lib")
PREFIX = "hardened-blikvm-state-init-"
CURRENT_BUNDLE_SHA = "d8c144158cc97b0acfd43ebbf7660d0fc1d0a2e8dcc6d378295240564d4ef028"
CURRENT_CONFIG_SHA = "93b2ae14c5dd12d831de01f4a4ba491667a5eee34512e161ef34e7cd29590c5e"
CURRENT_SUPERVISOR_SHA = "a6d7c99e1269371a2b1c62aebc2e31ead9ca30b5a1a54356afc367d63804dc59"
SLOT = "app-00000000000000000001-7a088d20f639fabf5ed4f59b7223cd34b8f39cb9fe2c5764acff6df1789fbc6f"


class BootstrapFailure(ValueError):
    def __init__(self, root: Path, message: str):
        super().__init__(message)
        self.guard_root = str(root)


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def checked(command: list[str], *, timeout: int = 30) -> bytes:
    result = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout, check=False)
    require(result.returncode == 0, "command failed: " + command[0] + ": " + result.stderr.decode(errors="replace")[:1024])
    return result.stdout


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def new_file(path: Path, data: bytes, mode: int = 0o600) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, mode)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)
        stream.flush()
        os.fchmod(stream.fileno(), mode)
        os.fsync(stream.fileno())
    sync_directory(path.parent)


def trusted_code_and_config(*, expected_supervisor_sha: str = CURRENT_SUPERVISOR_SHA) -> None:
    require(re.fullmatch(r"[a-f0-9]{64}", expected_supervisor_sha) is not None, "invalid supervisor binding")
    for path, expected in ((CONFIG, CURRENT_CONFIG_SHA), (SUPERVISOR, expected_supervisor_sha)):
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1, "unsafe installed input")
        require(path.resolve(strict=True) == path and stat.S_IMODE(meta.st_mode) == (0o600 if path == CONFIG else 0o755),
                "installed input path or permissions changed")
        require(digest(path) == expected, "installed input hash mismatch")
    require(stat.S_IMODE(CONFIG.stat().st_mode) == 0o600, "configuration permissions changed")


def units(token: str) -> dict[str, str]:
    return {name: PREFIX + token + "-" + name + suffix for name, suffix in (
        ("boot", ".service"), ("rollback", ".service"), ("timer", ".timer"),
        ("stage", ".service"), ("initialize", ".service"), ("status", ".service"),
    )}


def render_guards(root: Path, token: str) -> dict[str, str]:
    names = units(token)
    run = f"ExecStart=/usr/bin/python3 -B {root}/controller.py rollback --guard-root {root}\n"
    common = ("[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=90\n"
              "NoNewPrivileges=yes\nPrivateDevices=yes\nProtectHome=yes\nRestrictNamespaces=yes\n"
              "RestrictAddressFamilies=AF_UNIX\n" + run)
    return {
        names["boot"]: ("[Unit]\nDescription=Initial application state recovery\nDefaultDependencies=no\n"
                        "After=local-fs.target\nBefore=multi-user.target\n" + common + "[Install]\nWantedBy=sysinit.target\n"),
        names["rollback"]: "[Unit]\nDescription=Initial application state deadline recovery\n" + common,
        names["timer"]: ("[Unit]\nDescription=Initial application state deadline\n[Timer]\nOnActiveSec=600\n"
                         f"Unit={names['rollback']}\n[Install]\nWantedBy=timers.target\n"),
    }


def arm(root: Path, token: str) -> None:
    for name, content in render_guards(root, token).items():
        new_file(Path("/etc/systemd/system") / name, content.encode(), 0o644)
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "enable", units(token)["boot"]])
    checked(["systemctl", "enable", "--now", units(token)["timer"]])
    checked(["systemctl", "is-active", "--quiet", units(token)["timer"]])
    os.sync()


def disarm(root: Path, token: str) -> None:
    # Only this guard's exact unit files may be removed; recovery is replayable.
    for name, content in render_guards(root, token).items():
        path = Path("/etc/systemd/system") / name
        if path.exists() or path.is_symlink():
            require(not path.is_symlink() and path.read_bytes() == content.encode(), "guard unit changed")
    if (Path("/etc/systemd/system") / units(token)["timer"]).exists():
        checked(["systemctl", "disable", "--now", units(token)["timer"]])
    if (Path("/etc/systemd/system") / units(token)["boot"]).exists():
        checked(["systemctl", "disable", units(token)["boot"]])
    for name, target in (("sysinit.target.wants", units(token)["boot"]),
                         ("timers.target.wants", units(token)["timer"])):
        link = Path("/etc/systemd/system") / name / target
        if link.exists() or link.is_symlink():
            require(link.is_symlink() and link.resolve(strict=False) == Path("/etc/systemd/system") / target,
                    "unexpected guard unit link")
            link.unlink()
    for name in render_guards(root, token):
        (Path("/etc/systemd/system") / name).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    sync_directory(Path("/etc/systemd/system"))


def supervisor(token: str, action: str, *arguments: str) -> dict[str, object]:
    unit = units(token)[action]
    command = [
        "systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + unit,
        "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm",
        "--property=UMask=0077", "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict",
        "--property=ProtectHome=yes", "--property=PrivateDevices=yes", "--property=CapabilityBoundingSet=",
        "--property=RestrictNamespaces=yes", "--property=SystemCallFilter=@system-service",
        "--property=RestrictAddressFamilies=AF_UNIX", "--property=RuntimeMaxSec=30",
        "--property=RuntimeDirectory=hardened-blikvm-update", "--property=RuntimeDirectoryMode=0700",
        "--property=ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
        "--property=LoadCredential=update-supervisor-config:" + str(CONFIG),
        str(SUPERVISOR), "--config", "/run/credentials/" + unit + "/update-supervisor-config", action, *arguments,
    ]
    result = cast(dict[str, object], json.loads(checked(command, timeout=45)))
    require(not Path("/run/credentials", unit).exists(), "credential mount remains")
    require(not Path("/run/hardened-blikvm-update").exists(), "transient runtime remains")
    return result


def verify(token: str, *, expected_supervisor_sha: str = CURRENT_SUPERVISOR_SHA) -> dict[str, object]:
    trusted_code_and_config(expected_supervisor_sha=expected_supervisor_sha)
    status = supervisor(token, "status")
    for name, expected in {"generation": 1, "floorSequence": 1, "activeSlotId": SLOT,
                           "confirmedSlotId": SLOT, "previousSlotId": None, "pending": None}.items():
        require(status.get(name) == expected, "initial status mismatch: " + name)
    require(os.readlink(PARENT / "hardened-blikvm-slots/current") == SLOT, "pointer mismatch")
    return status


def guard_root(value: str) -> tuple[Path, str]:
    root = Path(value)
    require(root.parent == PARENT and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "guard path is outside fixed namespace")
    require(root.resolve(strict=True) == root, "guard path is not canonical")
    return root, root.name.removeprefix(PREFIX)


def source_path(token: str) -> Path:
    return Path("/tmp") / (PREFIX + token + "-source")


def remove_source(token: str) -> None:
    root = source_path(token)
    if not root.exists() and not root.is_symlink():
        return
    metadata = root.lstat()
    require(stat.S_ISDIR(metadata.st_mode) and metadata.st_uid == 0 and stat.S_IMODE(metadata.st_mode) == 0o750,
            "source directory changed")
    require({path.name for path in root.iterdir()} == {"application.tar"}, "unexpected source contents")
    source = root / "application.tar"
    require(not source.is_symlink() and source.stat().st_uid == 0 and source.stat().st_nlink == 1
            and digest(source) == CURRENT_BUNDLE_SHA, "source archive changed")
    source.unlink()
    root.rmdir()
    sync_directory(Path("/tmp"))


def binding(token: str) -> str:
    return hashlib.sha256((token + CURRENT_BUNDLE_SHA + CURRENT_CONFIG_SHA).encode()).hexdigest()


def rollback(root: Path, token: str, roots: BootstrapRoots) -> dict[str, object]:
    if roots.load()["phase"] != "committed":
        for name in ("stage", "initialize", "status"):
            subprocess.run(["systemctl", "stop", units(token)[name]], capture_output=True, timeout=40, check=False)
            state = checked(["systemctl", "show", "-p", "ActiveState", "--value", units(token)[name]]).strip()
            require(state in (b"inactive", b"failed"), "bootstrap worker did not stop")
        roots.quarantine()
        require(not (PARENT / "hardened-blikvm-slots").exists() and not (PARENT / "hardened-blikvm-update").exists(),
                "production roots remain after rollback")
    disarm(root, token)
    remove_source(token)
    if roots.load()["phase"] == "quarantined":
        quarantine = PARENT / ("hardened-blikvm-state-quarantine-" + token)
        rename_owned(root, quarantine, identity(root))
        return {"result": "rolled_back", "quarantineRoot": str(quarantine), "quarantinePreserved": True}
    return {"result": "retained", "guardRoot": str(root)}


def prepare(bundle: Path) -> dict[str, object]:
    trusted_code_and_config()
    require(bundle.is_file() and not bundle.is_symlink() and bundle.stat().st_size <= 128 * 1024 * 1024,
            "invalid bundle input")
    data = bundle.read_bytes()
    require(hashlib.sha256(data).hexdigest() == CURRENT_BUNDLE_SHA, "bundle input hash mismatch")
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
        stream = archive.extractfile("manifest.json")
        assert stream is not None
        manifest = cast(dict[str, object], json.loads(stream.read()))
    # Compare the reviewed bootstrap payload with installed files before mutation.
    for item in cast(list[dict[str, object]], manifest["files"]):
        name = cast(str, item["path"])
        path = (Path("/usr/share/hardened-blikvm/web") / name.removeprefix("share/web/")) if name.startswith("share/web/") else Path("/usr/libexec/hardened-blikvm") / Path(name).name
        require(digest(path) == item["sha256"], "installed payload differs from bootstrap")
    token = secrets.token_hex(12)
    root = PARENT / (PREFIX + token)
    root.mkdir(mode=0o700)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        roots = BootstrapRoots(root, PARENT, binding(token))
        account = pwd.getpwnam("hardened-blikvm-update")
        shared_gid = grp.getgrnam("hardened-blikvm").gr_gid
        require(account.pw_uid != 0, "nonroot updater required")
        roots.prepare(account.pw_uid, shared_gid)
        new_file(root / "controller.py", Path(__file__).read_bytes())
        new_file(root / "application_bootstrap_roots.py", Path(__file__).with_name("application_bootstrap_roots.py").read_bytes())
        source = source_path(token)
        source.mkdir(mode=0o750)
        source.chmod(0o750)
        os.chown(source, 0, shared_gid)
        new_file(source / "application.tar", data, 0o640)
        os.chown(source / "application.tar", 0, shared_gid)
        try:
            arm(root, token)
            roots.publish()
            staged = supervisor(token, "stage", str(source / "application.tar"))
            require(staged.get("slotId") == SLOT and staged.get("result") == "published", "unexpected staged slot")
            supervisor(token, "initialize", SLOT)
            status = verify(token)
            return {"result": "prepared", "guardRoot": str(root), "status": status,
                    "rootIdentities": roots.load()["roots"], "guardArmed": True}
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            try:
                recovery = rollback(root, token, roots)
            except (OSError, ValueError, subprocess.SubprocessError) as recovery_error:
                raise BootstrapFailure(root, f"{error}; recovery incomplete: {recovery_error}") from None
            raise BootstrapFailure(Path(cast(str, recovery.get("quarantineRoot", str(root)))),
                                   f"{error}; created roots quarantined") from None
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        if isinstance(error, BootstrapFailure):
            raise
        raise BootstrapFailure(root, str(error)) from None
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "inspect", "commit", "rollback", "cleanup", "status"))
    parser.add_argument("--bundle")
    parser.add_argument("--guard-root")
    args = parser.parse_args()
    require(os.geteuid() == 0, "root orchestration required")
    action = cast(str, args.action)
    if action == "prepare":
        require(args.bundle is not None, "bundle required")
        result = prepare(Path(cast(str, args.bundle)))
    elif action == "status":
        result = {"result": "verified", "status": verify(secrets.token_hex(12))}
    else:
        require(isinstance(args.guard_root, str), "guard root required")
        root, token = guard_root(cast(str, args.guard_root))
        roots = BootstrapRoots(root, PARENT, binding(token))
        descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            if action == "rollback":
                result = rollback(root, token, roots)
            elif action == "cleanup":
                require(roots.load()["phase"] == "committed", "uncommitted roots cannot be discarded")
                roots.locations(roots.load())
                disarm(root, token)
                remove_source(token)
                require({path.name for path in root.iterdir()} <= {"controller.py", "application_bootstrap_roots.py", "roots.json", "__pycache__"}, "unexpected guard content")
                # Python bytecode caching is disabled by the caller and boot unit.
                require(not (root / "__pycache__").exists(), "unexpected bytecode cache")
                for name in ("controller.py", "application_bootstrap_roots.py", "roots.json"):
                    (root / name).unlink()
                root.rmdir()
                sync_directory(PARENT)
                result = {"result": "cleaned"}
            else:
                roots.locations(roots.load())
                status = verify(token)
                if action == "commit":
                    roots.commit()
                    disarm(root, token)
                result = {"result": "committed" if action == "commit" else "verified", "status": status}
        finally:
            os.close(descriptor)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        failure = {"result": "failed", "errorType": type(error).__name__, "error": str(error)[:1024]}
        if isinstance(error, BootstrapFailure):
            failure["guardRoot"] = error.guard_root
        print(json.dumps(failure))
        raise SystemExit(1) from None
