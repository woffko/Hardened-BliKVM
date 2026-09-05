#!/usr/bin/env python3
"""Guarded package-backed to signed-slot serving switch, without candidate activation."""

from __future__ import annotations

import argparse
import fcntl
import hashlib
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
    import initialize_application_state as bootstrap
else:
    from tools import initialize_application_state as bootstrap

require = bootstrap.require
checked = bootstrap.checked
digest = bootstrap.digest
new_file = bootstrap.new_file
sync_directory = bootstrap.sync_directory
OLD_SERVER = "hardened-blikvm-server.service"
CURRENT_SERVER = "hardened-blikvm-server-current.service"
CANDIDATE_SERVER = "hardened-blikvm-server-candidate.service"
NGINX = "hardened-blikvm-nginx.service"
UNIT = Path("/etc/systemd/system") / NGINX
ORIGINAL_UNIT_SHA = "741f705246f9a7abd5ca28e4d4e8ec68ff55bff9300ff9a098a7310b8e805a0b"
PREFIX = "hardened-blikvm-slot-serving-"
CHECKED_FILES = {
    "account": Path("/var/lib/hardened-blikvm/account.json"),
    "nginxConfig": Path("/etc/hardened-blikvm/nginx.conf"),
    "certificate": Path("/etc/kvmd/nginx/ssl/server.crt"),
    "tlsKey": Path("/etc/kvmd/nginx/ssl/server.key"),
    "trust": bootstrap.CONFIG,
}
PEERS = {"broker": "hardened-blikvm-platform-broker.service", "video": "hardened-blikvm-video-proxy.service"}


def slot_nginx_unit(original: bytes) -> bytes:
    required = f"Requires={OLD_SERVER}\n".encode()
    ordered = f"After={OLD_SERVER}\n".encode()
    lines = original.splitlines(keepends=True)
    require(lines.count(required) == 1 and lines.count(ordered) == 1, "unexpected Nginx dependencies")
    return b"".join(f"After={CURRENT_SERVER} {CANDIDATE_SERVER}\n".encode() if line == ordered else line
                    for line in lines if line != required)


def property_value(unit: str, name: str) -> str:
    return checked(["systemctl", "show", "-p", name, "--value", unit]).decode().strip()


def enabled(unit: str) -> str:
    result = subprocess.run(["systemctl", "is-enabled", unit], capture_output=True, timeout=10, check=False)
    return result.stdout.decode().strip()


def inactive(unit: str) -> None:
    require(property_value(unit, "ActiveState") in ("inactive", "failed"), "unexpected active unit: " + unit)


def snapshot() -> dict[str, str]:
    values = {name: digest(path) for name, path in CHECKED_FILES.items()}
    for name, unit in PEERS.items():
        require(property_value(unit, "ActiveState") == "active", "runtime peer inactive")
        values[name + "Pid"] = property_value(unit, "MainPID")
        values[name + "Restarts"] = property_value(unit, "NRestarts")
    values["nginxPid"] = property_value(NGINX, "MainPID")
    return values


def no_previous_guard() -> None:
    require(not list(Path("/var/lib").glob(PREFIX + "*")), "previous serving guard requires inspection")
    require(not list(UNIT.parent.glob(PREFIX + "*")) and not list(UNIT.parent.glob("." + PREFIX + "*.part")),
            "previous serving unit artifacts require inspection")


def updater_inert() -> None:
    for unit in ("hardened-blikvm-update-watchdog.timer", "hardened-blikvm-update-watchdog.service",
                 "hardened-blikvm-update-confirm.socket", "hardened-blikvm-update-confirm@.service",
                 "hardened-blikvm-update-activate@.service", "hardened-blikvm-update-apply-candidate.service",
                 "hardened-blikvm-update-apply-confirmed.service", CANDIDATE_SERVER):
        inactive(unit)
        require(enabled(unit) not in ("enabled", "enabled-runtime", "linked", "linked-runtime"), "update infrastructure enabled")
    require(not Path("/run/hardened-blikvm-update/health-token").exists(), "candidate activation is pending")
    require(digest(Path("/lib/systemd/system") / CURRENT_SERVER) == "dd3d529561a1e3699d1eec9d48b07921067de3242b1f87251945d519b63644d1",
            "current server template changed")


def verify(before: dict[str, str], current: bool, *, preserve_nginx_pid: bool = True) -> dict[str, object]:
    bootstrap.verify(secrets.token_hex(12))
    updater_inert()
    actual = snapshot()
    for name, value in before.items():
        if name != "nginxPid" or preserve_nginx_pid:
            require(actual.get(name) == value, "preserved runtime fact changed: " + name)
    server = CURRENT_SERVER if current else OLD_SERVER
    other = OLD_SERVER if current else CURRENT_SERVER
    require(property_value(server, "ActiveState") == "active" and enabled(server) == "enabled", "serving unit is not active/enabled")
    inactive(other)
    require(enabled(other) == "disabled", "other server must be disabled")
    require(property_value(NGINX, "ActiveState") == "active" and enabled(NGINX) == "enabled", "Nginx unavailable")
    pid = property_value(server, "MainPID")
    require(pid.isdecimal() and int(pid) > 0, "invalid server PID")
    executable = Path("/proc") / pid / "exe"
    expected_path = (Path("/var/lib/hardened-blikvm-slots") / bootstrap.SLOT / "bin/bli-kvm-server") if current else Path("/usr/libexec/hardened-blikvm/bli-kvm-server")
    require(executable.resolve(strict=True) == expected_path, "server is not executing the expected artifact")
    require(digest(executable) == digest(expected_path), "server executable hash mismatch")
    health = checked(["curl", "--noproxy", "*", "--insecure", "--silent", "--fail", "--max-time", "5", "https://127.0.0.1/api/health"])
    require(bool(health), "empty HTTPS health response")
    return {"result": "verified", "server": server, "serverPid": pid, "executableSha256": digest(executable),
            "nginxPidUnchanged": actual["nginxPid"] == before["nginxPid"], "accountTlsTrustPeersUnchanged": True}


def guard_path(value: str) -> tuple[Path, str]:
    path = Path(value)
    require(path.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", path.name) is not None,
            "invalid serving guard path")
    require(path.resolve(strict=True) == path, "noncanonical serving guard")
    meta = path.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe serving guard")
    return path, path.name.removeprefix(PREFIX)


def marker(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and 0 < meta.st_size < 16384, "unsafe serving marker")
    decoded = cast(object, json.loads(path.read_bytes()))
    require(isinstance(decoded, dict), "invalid serving marker")
    plan = cast(dict[str, object], decoded)
    require(set(plan) == {"schemaVersion", "phase", "before", "oldUnitSha256", "newUnitSha256"}
            and plan.get("schemaVersion") == 1 and plan.get("phase") in ("prepared", "switched", "committed", "rolled_back"), "invalid serving marker fields")
    require(plan.get("oldUnitSha256") == ORIGINAL_UNIT_SHA, "unexpected original Nginx unit")
    require(digest(root / "nginx.old") == plan["oldUnitSha256"] and digest(root / "nginx.new") == plan["newUnitSha256"], "backup unit changed")
    require((root / "nginx.new").read_bytes() == slot_nginx_unit((root / "nginx.old").read_bytes()), "invalid slot unit transformation")
    return plan


def save(root: Path, plan: dict[str, object]) -> None:
    part = root / "plan.json.part"
    if part.exists() or part.is_symlink():
        marker(root)
        meta = part.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and meta.st_size < 16384, "unsafe incomplete marker")
        part.unlink()
    new_file(part, json.dumps(plan, sort_keys=True).encode())
    os.replace(part, root / "plan.json")
    sync_directory(root)


def replace_unit(root: Path, plan: dict[str, object], current: bool) -> None:
    meta = UNIT.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) == 0o644,
            "unsafe local Nginx unit")
    require(digest(UNIT) in (plan["oldUnitSha256"], plan["newUnitSha256"]), "local Nginx unit changed independently")
    content = (root / ("nginx.new" if current else "nginx.old")).read_bytes()
    part = UNIT.parent / ("." + root.name + ".part")
    if part.exists() or part.is_symlink():
        partial_metadata = part.lstat()
        require(stat.S_ISREG(partial_metadata.st_mode) and partial_metadata.st_uid == 0
                and partial_metadata.st_nlink == 1 and stat.S_IMODE(partial_metadata.st_mode) == 0o644,
                "unsafe incomplete Nginx unit")
        partial = part.read_bytes()
        require(any((root / name).read_bytes().startswith(partial) for name in ("nginx.old", "nginx.new")),
                "incomplete Nginx unit is not a known transition")
        part.unlink()
        sync_directory(UNIT.parent)
    new_file(part, content, 0o644)
    os.replace(part, UNIT)
    sync_directory(UNIT.parent)
    checked(["systemctl", "daemon-reload"])


def guard_units(root: Path, token: str) -> dict[str, str]:
    base = PREFIX + token
    service = "[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=120\nNoNewPrivileges=yes\nPrivateDevices=yes\nProtectHome=yes\n"
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py rollback --guard-root {root}"
    return {
        base + "-boot.service": "[Unit]\nDescription=Signed-slot serving early recovery\nDefaultDependencies=no\nAfter=local-fs.target\nBefore=multi-user.target\n" + service + command + " --boot\n[Install]\nWantedBy=sysinit.target\n",
        base + "-rollback.service": "[Unit]\nDescription=Signed-slot serving deadline recovery\n" + service + command + "\n",
        base + "-deadline.timer": f"[Unit]\nDescription=Signed-slot serving deadline\n[Timer]\nOnActiveSec=600\nUnit={base}-rollback.service\n[Install]\nWantedBy=timers.target\n",
    }


def arm(root: Path, token: str) -> None:
    for name, content in guard_units(root, token).items():
        new_file(UNIT.parent / name, content.encode(), 0o644)
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "enable", PREFIX + token + "-boot.service"])
    checked(["systemctl", "enable", "--now", PREFIX + token + "-deadline.timer"])
    checked(["systemctl", "is-active", "--quiet", PREFIX + token + "-deadline.timer"])
    os.sync()


def disarm(root: Path, token: str, boot: bool = False) -> None:
    files = guard_units(root, token)
    for name, content in files.items():
        path = UNIT.parent / name
        if path.exists() or path.is_symlink():
            require(not path.is_symlink() and path.read_bytes() == content.encode(), "serving guard unit changed")
    timer = PREFIX + token + "-deadline.timer"
    if (UNIT.parent / timer).exists():
        checked(["systemctl", "stop", "--no-block", timer] if boot else ["systemctl", "stop", timer])
        checked(["systemctl", "disable", timer])
    early = PREFIX + token + "-boot.service"
    if (UNIT.parent / early).exists():
        checked(["systemctl", "disable", early])
    for name in files:
        (UNIT.parent / name).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    sync_directory(UNIT.parent)


def rollback(root: Path, token: str, plan: dict[str, object], boot: bool) -> dict[str, object]:
    if plan["phase"] == "committed":
        disarm(root, token, boot)
        return {"result": "retained"}
    replace_unit(root, plan, False)
    checked(["systemctl", "disable", CURRENT_SERVER])
    checked(["systemctl", "stop", "--no-block", CURRENT_SERVER] if boot else ["systemctl", "stop", CURRENT_SERVER])
    checked(["systemctl", "enable", OLD_SERVER])
    checked(["systemctl", "start", "--no-block", OLD_SERVER] if boot else ["systemctl", "start", OLD_SERVER])
    checked(["systemctl", "start", "--no-block", NGINX] if boot else ["systemctl", "start", NGINX])
    if not boot:
        verify(cast(dict[str, str], plan["before"]), False, preserve_nginx_pid=False)
    plan["phase"] = "rolled_back"
    save(root, plan)
    disarm(root, token, boot)
    return {"result": "rolled_back", "guardRoot": str(root), "bootRecovery": boot}


def prepare() -> dict[str, object]:
    no_previous_guard()
    bootstrap.verify(secrets.token_hex(12))
    require(digest(UNIT) == ORIGINAL_UNIT_SHA, "unexpected initial Nginx unit")
    before = snapshot()
    verify(before, False)
    token = secrets.token_hex(12)
    root = Path("/var/lib") / (PREFIX + token)
    root.mkdir(mode=0o700)
    original = UNIT.read_bytes()
    replacement = slot_nginx_unit(original)
    new_file(root / "nginx.old", original)
    new_file(root / "nginx.new", replacement)
    for name, path in (("controller.py", Path(__file__)), ("initialize_application_state.py", Path(bootstrap.__file__)),
                       ("application_bootstrap_roots.py", Path(__file__).with_name("application_bootstrap_roots.py"))):
        new_file(root / name, path.read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "phase": "prepared", "before": before,
                               "oldUnitSha256": ORIGINAL_UNIT_SHA, "newUnitSha256": hashlib.sha256(replacement).hexdigest()}
    save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        arm(root, token)
        replace_unit(root, plan, True)
        require(OLD_SERVER not in property_value(NGINX, "Requires").split(), "Nginx still requires package server")
        checked(["systemctl", "disable", OLD_SERVER])
        checked(["systemctl", "enable", "--now", CURRENT_SERVER])
        checked(["systemctl", "start", NGINX])
        verified = verify(before, True)
        plan["phase"] = "switched"
        save(root, plan)
        return {"result": "prepared", "guardRoot": str(root), "verification": verified, "newNginxUnitSha256": plan["newUnitSha256"]}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        try:
            rollback(root, token, plan, False)
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
        raise bootstrap.BootstrapFailure(root, str(error)) from None
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "commit", "rollback", "cleanup"))
    parser.add_argument("--guard-root")
    parser.add_argument("--boot", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0, "root orchestration required")
    if args.action == "preflight":
        no_previous_guard()
        require(digest(UNIT) == ORIGINAL_UNIT_SHA, "unexpected initial Nginx unit")
        result = verify(snapshot(), False)
        result["persistentChanges"] = False
    elif args.action == "prepare":
        result = prepare()
    else:
        require(isinstance(args.guard_root, str), "guard root required")
        root, token = guard_path(cast(str, args.guard_root))
        descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            plan = marker(root)
            if args.action == "rollback":
                result = rollback(root, token, plan, bool(args.boot))
            else:
                current = plan["phase"] != "rolled_back"
                require(digest(UNIT) == plan["newUnitSha256" if current else "oldUnitSha256"], "serving unit hash mismatch")
                result = verify(cast(dict[str, str], plan["before"]), current, preserve_nginx_pid=current)
                if args.action == "commit":
                    require(plan["phase"] == "switched", "serving switch is not prepared")
                    plan["phase"] = "committed"
                    save(root, plan)
                    disarm(root, token)
                    result["result"] = "committed"
                elif args.action == "cleanup":
                    require(plan["phase"] in ("committed", "rolled_back"), "serving guard still needed")
                    disarm(root, token)
                    allowed = {"plan.json", "nginx.old", "nginx.new", "controller.py", "initialize_application_state.py", "application_bootstrap_roots.py"}
                    require({path.name for path in root.iterdir()} == allowed, "unexpected guard contents")
                    for name in allowed:
                        (root / name).unlink()
                    root.rmdir()
                    sync_directory(root.parent)
                    result["result"] = "cleaned"
        finally:
            os.close(descriptor)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        failure = {"result": "failed", "errorType": type(error).__name__, "error": str(error)[:1024]}
        if isinstance(error, bootstrap.BootstrapFailure):
            failure["guardRoot"] = error.guard_root
        print(json.dumps(failure))
        raise SystemExit(1) from None
