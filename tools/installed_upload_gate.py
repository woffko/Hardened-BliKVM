#!/usr/bin/env python3
"""Guard a retained-slot upload IPC rehearsal without enabling HTTP writes."""

from __future__ import annotations

import argparse
import fcntl
import grp
import json
import os
from pathlib import Path
import pwd
import re
import select
import shlex
import secrets
import stat
import subprocess
import sys
import time
import tarfile
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import capture_operational_boot as boot
    import probe_installed_upload as probe
    import installed_application_update as application
    from upload_unit_reference import UploadUnitReference
else:
    from tools import capture_operational_boot as boot
    from tools import probe_installed_upload as probe
    from tools import installed_application_update as application
    from tools.upload_unit_reference import UploadUnitReference

u, s = boot.u, boot.s
require, checked = u.require, u.checked
PREFIX = "hardened-blikvm-upload-gate-"
UNITS = Path("/run/systemd/system")
SOCKET = "hardened-blikvm-update-upload.socket"
SPOOL = Path("/var/lib/hardened-blikvm-upload")
HELPERS = tuple(dict.fromkeys(("capture_operational_boot.py", "probe_installed_upload.py", "upload_unit_reference.py",
                              "installed_application_update.py", *u.HELPERS)))
MODES = ("retry", "invalid-archive", "short", "surplus", "malformed", "unauthorized", "idle", "lost-reply", "retry")


def modes(profile: probe.UploadProfile) -> tuple[str, ...]:
    return ("publish", *MODES) if profile.publish else MODES


def plan_profile(plan: dict[str, object]) -> probe.UploadProfile:
    name = plan.get("profile", probe.DEFAULT_PROFILE.name)
    require(isinstance(name, str) and name in probe.PROFILES, "unknown upload profile")
    return probe.PROFILES[cast(str, name)]


def publication_preflight(before: dict[str, object], profile: probe.UploadProfile) -> None:
    if not profile.publish:
        return
    state = cast(dict[str, object], before["state"])
    root = u.STATE_ROOTS[0]
    entries = {Path(key).name for key in state if Path(key).parent == root}
    require(profile.slot not in entries, "publication target already exists; inspect previous attempt")
    slots = {name for name in entries if re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", name)}
    require(len(slots) == 7 and entries == slots | {"current"}, "unexpected pre-publication slot inventory")


def verify_publication(archive: Path, profile: probe.UploadProfile, added: dict[str, object]) -> None:
    require(archive.stat().st_size == probe.BUNDLE_BYTES and u.digest(archive) == profile.bundle_sha, "publication archive binding differs")
    slot = u.STATE_ROOTS[0] / profile.slot
    uid = pwd.getpwnam("hardened-blikvm-update").pw_uid
    gid = grp.getgrnam("hardened-blikvm").gr_gid
    expected: dict[str, int] = {".": 0o555, ".hardened-blikvm": 0o500,
        **{".hardened-blikvm/" + name: 0o400 for name in ("receipt.json", "manifest.json", "manifest.sig")}}
    proofs: dict[str, bytes] = {}
    with tarfile.open(archive, mode="r:") as source:
        for member in source:
            if member.name.startswith("payload/") and member.isfile():
                relative = Path(member.name).relative_to("payload")
                require(".." not in relative.parts and not relative.is_absolute(), "unsafe archive path")
                expected[str(relative)] = member.mode & ~0o222
                for parent in relative.parents:
                    if parent != Path("."):
                        expected[str(parent)] = 0o555
        for name in ("manifest.json", "manifest.sig"):
            stream = source.extractfile(name)
            require(stream is not None, "archive proof missing")
            assert stream is not None
            proofs[name] = stream.read()
    require(set(added) == {str(slot / name) for name in expected}, "unexpected published tree entries")
    for relative, mode in expected.items():
        entry = cast(dict[str, object], added[str(slot / relative)])
        actual_mode = cast(int, entry["mode"])
        directory = relative in (".", ".hardened-blikvm") or any(name.startswith(relative + "/") for name in expected)
        require((stat.S_ISDIR(actual_mode) if directory else stat.S_ISREG(actual_mode))
                and stat.S_IMODE(actual_mode) == mode and entry["uid"] == uid and entry["gid"] == gid,
                "published inode identity or mode differs")
    for name, data in proofs.items():
        require((slot / ".hardened-blikvm" / name).read_bytes() == data, "published proof differs")
    receipt = json.loads((slot / ".hardened-blikvm/receipt.json").read_bytes(), object_pairs_hook=probe.unique_object)
    expected_receipt = {"schemaVersion": 1, "slotId": profile.slot,
                        "manifestSha256": profile.manifest_sha, "payloadBytes": 8683648}
    require(json.dumps(receipt, sort_keys=True) == json.dumps(expected_receipt, sort_keys=True), "published receipt differs")
    application.verify_staged_payload(archive, slot, expected_bundle_sha=profile.bundle_sha,
                                      expected_bundle_bytes=probe.BUNDLE_BYTES)


def preserved(plan: dict[str, object], after: dict[str, object], archive: Path) -> bool:
    before = cast(dict[str, object], plan["before"])
    profile = plan_profile(plan)
    if not profile.publish or not plan.get("publicationAttempted", False):
        require(after == before, "retained application or serving state changed")
        return False
    original = cast(dict[str, object], before["state"])
    current = cast(dict[str, object], after["state"])
    require({key: value for key, value in before.items() if key != "state"}
            == {key: value for key, value in after.items() if key != "state"}, "serving or authoritative state changed")
    require(all(key in current and current[key] == value for key, value in original.items()), "existing persistent entry changed")
    added = {key: value for key, value in current.items() if key not in original}
    if not added:
        require(not plan.get("newSlotPublished", False), "published slot disappeared")
        return False
    slot = str(u.STATE_ROOTS[0] / profile.slot)
    require(all(key == slot or key.startswith(slot + "/") for key in added), "publication changed another path")
    verify_publication(archive, profile, added)
    previous = plan.get("publishedTree")
    require(previous is None or previous == added, "published slot changed after verification")
    plan["publishedTree"] = added
    plan["newSlotPublished"] = True
    return True


def source_root(root: Path) -> Path:
    return Path("/tmp") / (root.name + "-source")


def guard_root(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None, "invalid guard path")
    meta = root.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700
            and root.resolve(strict=True) == root, "unsafe guard root")
    return root


def guard_units(root: Path) -> dict[str, bytes]:
    return {
        root.name + "-restore.service": ("[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=120\n"
            f"ExecStart=/usr/bin/python3 -B {root}/controller.py emergency-stop --guard-root {root}\n").encode(),
        root.name + "-deadline.timer": f"[Timer]\nOnActiveSec=600\nUnit={root.name}-restore.service\n".encode(),
    }


def save(root: Path, plan: dict[str, object]) -> None:
    temporary = root / ("plan-" + secrets.token_hex(12) + ".part")
    u.new_file(temporary, json.dumps(plan, sort_keys=True).encode())
    os.replace(temporary, root / "plan.json")
    u.sync_directory(root)


def load(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe guard plan")
    value = cast(dict[str, object], json.loads(path.read_bytes()))
    require(value.get("schemaVersion") == 1 and value.get("bundleSha256") == plan_profile(value).bundle_sha
            and value.get("phase") in ("prepared", "running", "restored")
            and type(value.get("spoolExisted")) is bool, "invalid guard plan")
    return value


def workers() -> list[str]:
    lines = checked(["systemctl", "list-units", "--all", "--plain", "--no-legend",
                     "hardened-blikvm-update-upload@*.service"]).decode().splitlines()
    result: list[str] = []
    for line in lines:
        name = line.split()[0]
        require(re.fullmatch(r"hardened-blikvm-update-upload@[A-Za-z0-9_.:\\-]+\.service", name) is not None,
                "unexpected upload worker name")
        result.append(name)
    return result


def audit_upload_units() -> None:
    expected = Path("/lib/systemd/system") / SOCKET
    require(Path(s.property_value(SOCKET, "FragmentPath")).resolve(strict=True) == expected.resolve(strict=True)
            and not s.property_value(SOCKET, "DropInPaths").strip(), "upload socket has foreign overrides")
    # A template cannot be queried with systemctl show. Cat resolves template
    # fragments/drop-ins without creating a synthetic worker instance.
    template = Path("/lib/systemd/system/hardened-blikvm-update-upload@.service")
    raw = checked(["systemctl", "cat", "--no-pager", template.name], timeout=5)
    require(len(raw) <= 16384, "oversize upload template")
    header, separator, body = raw.partition(b"\n")
    require(bool(separator) and header.startswith(b"# ")
            and Path(header[2:].decode()).resolve(strict=True) == template.resolve(strict=True)
            and body.strip() == template.read_bytes().strip(), "upload template has foreign fragments or overrides")


def properties(unit: str, fields: tuple[str, ...]) -> dict[str, str]:
    raw = checked(["systemctl", "show", unit, "--property=" + ",".join(fields)], timeout=5)
    require(len(raw) <= 16384, "oversize worker properties")
    lines = raw.decode().splitlines()
    value = dict(line.split("=", 1) for line in lines)
    require(set(value) == set(fields) and len(lines) == len(fields), "worker properties missing or duplicated")
    return value


def live_worker(unit: str) -> dict[str, object]:
    value = properties(unit, ("MainPID", "InvocationID", "User", "Group", "NoNewPrivileges", "ProtectSystem", "ReadWritePaths",
                              "FragmentPath", "DropInPaths", "CollectMode"))
    require(Path(value["FragmentPath"]).resolve(strict=True) == Path("/lib/systemd/system/hardened-blikvm-update-upload@.service").resolve(strict=True)
            and not value["DropInPaths"] and value["CollectMode"] == "inactive", "worker instance overrides differ")
    require(value["MainPID"].isdecimal() and int(value["MainPID"]) > 0
            and re.fullmatch("[a-f0-9]{32}", value["InvocationID"]) is not None
            and value["User"] == "hardened-blikvm-update" and value["Group"] == "hardened-blikvm"
            and value["NoNewPrivileges"] == "yes" and value["ProtectSystem"] == "strict"
            and set(value["ReadWritePaths"].split()) == {str(u.STATE_ROOTS[0]), str(SPOOL)}, "worker sandbox differs")
    proc = Path("/proc") / value["MainPID"]
    identity = proc.joinpath("stat").read_text().rpartition(")")[2].split()[19]
    require(proc.joinpath("exe").resolve(strict=True) == s.bootstrap.SUPERVISOR
            and u.digest(proc / "exe") == u.SUPERVISOR_HASHES["new"], "unexpected worker executable")
    uid = pwd.getpwnam("hardened-blikvm-update").pw_uid
    gid = grp.getgrnam("hardened-blikvm").gr_gid
    status = {name: entry.strip() for line in (proc / "status").read_text().splitlines()
              if ":" in line for name, entry in [line.split(":", 1)] if name in {"Uid", "Gid", "CapEff", "NoNewPrivs"}}
    require(status.get("Uid", "").split() == [str(uid)] * 4 and status.get("Gid", "").split() == [str(gid)] * 4
            and status.get("CapEff") == "0000000000000000" and status.get("NoNewPrivs") == "1", "worker process privilege differs")
    found: list[str] = []
    descriptors = list((proc / "fd").iterdir())
    require(len(descriptors) <= 128, "worker descriptor bound exceeded")
    for fd in descriptors:
        try:
            target = os.readlink(fd)
            if not target.startswith(str(SPOOL) + "/#") or not target.endswith(" (deleted)"):
                continue
            meta = fd.stat()
        except FileNotFoundError:
            continue
        require(stat.S_ISREG(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o600
                and meta.st_uid == uid and meta.st_gid == gid and meta.st_nlink == 0
                and meta.st_dev == SPOOL.stat().st_dev and 0 < meta.st_size < probe.BUNDLE_BYTES, "unsafe anonymous spool")
        found.append(fd.name)
    require(len(found) == 1 and proc.joinpath("stat").read_text().rpartition(")")[2].split()[19] == identity
            and s.property_value(unit, "MainPID") == value["MainPID"], "worker or anonymous spool changed")
    return {"invocationId": value["InvocationID"], "pid": int(value["MainPID"]),
            "dedicatedIdentity": True, "emptyCapabilities": True, "anonymousSpool": True, "sandboxVerified": True}


def natural_exit(unit: str, mode: str, invocation: str | None = None) -> dict[str, object]:
    expected = {"publish": {0}, "retry": {0}, "invalid-archive": {0}, "short": {50}, "surplus": {50},
                "malformed": {50}, "unauthorized": {96}, "idle": {50}, "lost-reply": {0, 50}}[mode]
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        value = properties(unit, ("ActiveState", "Result", "ExecMainCode", "ExecMainStatus", "InvocationID", "CollectMode",
                                  "ExecMainStartTimestampMonotonic", "ExecMainExitTimestampMonotonic"))
        if value["ActiveState"] not in ("inactive", "failed"):
            time.sleep(0.1)
            continue
        require(value["CollectMode"] == "inactive" and value["ExecMainCode"] == "1" and value["ExecMainStatus"].isdecimal()
                and int(value["ExecMainStatus"]) in expected, "unexpected natural worker termination")
        code = int(value["ExecMainStatus"])
        require(value["Result"] == ("success" if code == 0 else "exit-code")
                and (invocation is None or value["InvocationID"] == invocation)
                and value["ExecMainStartTimestampMonotonic"].isdecimal()
                and value["ExecMainExitTimestampMonotonic"].isdecimal()
                and 0 < int(value["ExecMainStartTimestampMonotonic"]) <= int(value["ExecMainExitTimestampMonotonic"]),
                "worker terminal proof mismatch")
        return {"naturalExit": True, "exitCode": code, "result": value["Result"]}
    raise TimeoutError("worker failed to exit naturally")


def wait_live(unit: str) -> dict[str, object]:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        try:
            return live_worker(unit)
        except (ValueError, FileNotFoundError):
            time.sleep(0.1)
    raise TimeoutError("worker identity and anonymous spool not observed")


def held_handshake(stream: object) -> None:
    descriptor = cast(int, getattr(stream, "fileno")())
    expected = b"UPLOAD_BODY_HELD\n"
    value = bytearray()
    deadline = time.monotonic() + 5
    while len(value) < len(expected):
        remaining = deadline - time.monotonic()
        require(remaining > 0 and bool(select.select([descriptor], [], [], remaining)[0]), "probe did not hold body")
        part = os.read(descriptor, len(expected) - len(value))
        require(bool(part), "probe closed before handshake")
        value.extend(part)
    require(bytes(value) == expected, "unexpected probe handshake")


def run_vector(source: Path, mode: str, profile: probe.UploadProfile = probe.DEFAULT_PROFILE) -> dict[str, object]:
    previous = set(workers())
    held = mode not in ("malformed", "unauthorized")
    argv = ["python3", "-B", str(source / "probe.py"), mode, "--archive", str(source / "application.tar"), "--profile", profile.name]
    if held:
        argv.append("--hold")
    if mode != "unauthorized":
        argv = ["runuser", "-u", "hardened-blikvm-server", "--", *argv]
    child = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
    reference: UploadUnitReference | None = None
    live: dict[str, object] = {}
    output = b""
    try:
        if held:
            require(child.stdout is not None, "probe stdout missing")
            assert child.stdout is not None
            held_handshake(child.stdout)
        else:
            output, _ = child.communicate(timeout=25)
            require(child.returncode == 0, "early rejection probe failed")
        selected = set(workers()) - previous
        require(len(selected) == 1, "cannot correlate one upload worker")
        unit = selected.pop()
        if held:
            reference = UploadUnitReference(unit)
            live = wait_live(unit)
        if held:
            output, _ = child.communicate(input=b"G", timeout=25)
        require(child.returncode == 0 and len(output) <= 1024, "native upload probe failed")
        expected = {"mode": mode, "result": "reply-unobserved" if mode == "lost-reply" else "passed"}
        require(json.loads(output) == expected, "unexpected native probe receipt")
        terminal = natural_exit(unit, mode, cast(str | None, live.get("invocationId")))
        # Record natural termination before resetting only this owned instance.
        checked(["systemctl", "reset-failed", unit])
        empty_spool()
        return {**expected, "worker": terminal, "live": live}
    finally:
        if child.poll() is None:
            child.kill()
            child.wait(timeout=5)
        for pipe in (child.stdin, child.stdout):
            if pipe is not None:
                pipe.close()
        if reference is not None:
            reference.close()


def snapshot(active: bool) -> dict[str, object]:
    audit_upload_units()
    boot.persistent.validate_dropins(True)
    s.inactive("hardened-blikvm-update-activation.socket")
    require(s.enabled("hardened-blikvm-update-activation.socket") == "disabled", "activation socket enabled")
    require(not Path("/run/hardened-blikvm-update-api/activation.sock").exists(), "activation endpoint present")
    require(s.enabled(SOCKET) == "disabled", "upload socket enabled persistently")
    if active:
        require(s.property_value(SOCKET, "ActiveState") == "active", "upload socket inactive")
        meta = Path(probe.ENDPOINT).lstat()
        require(stat.S_ISSOCK(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o620
                and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
                and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid, "unsafe upload socket")
    else:
        u.write_transports_inert()
    value = boot.status_api.snapshot(True, expected_version=u.VERSIONS["new"],
        expected_supervisor_sha=u.SUPERVISOR_HASHES["new"], expected_slot=boot.SEQ7_SLOT,
        expected_floor=7, expected_generation=37)
    require(cast(dict[str, object], value["managerStatus"]).get("previousSlotId") == u.SEQ6_SLOT, "rollback anchor changed")
    pid = s.property_value(s.CURRENT_SERVER, "MainPID")
    require(pid.isdecimal() and int(pid) > 0, "invalid serving PID")
    cast(dict[str, object], value["serving"])["serverPid"] = pid
    value["persistentDropinHashes"] = {str(path): u.digest(path) for path in boot.persistent.dropins()}
    forbidden = {"BLI_KVM_UPDATE_UPLOAD_SOCKET", "BLI_KVM_UPDATE_ACTIVATION_SOCKET", "BLI_KVM_UPDATE_WRITES_ENABLED"}
    for unit in (s.CURRENT_SERVER, s.CANDIDATE_SERVER):
        require(not any(item.partition("=")[0] in forbidden for item in shlex.split(s.property_value(unit, "Environment"))),
                "HTTP write configuration present")
    environment = (Path("/proc") / pid / "environ").read_bytes()
    require(len(environment) <= 65536 and not any(item.partition(b"=")[0] in {name.encode() for name in forbidden}
                                                for item in environment.split(b"\0")), "running HTTP write configuration present")
    return value


def empty_spool() -> None:
    if not SPOOL.exists() and not SPOOL.is_symlink():
        return
    meta = SPOOL.lstat()
    require(stat.S_ISDIR(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o700
            and meta.st_uid == pwd.getpwnam("hardened-blikvm-update").pw_uid
            and meta.st_gid == grp.getgrnam("hardened-blikvm").gr_gid and not list(SPOOL.iterdir()), "unsafe or nonempty upload spool")


def drain() -> None:
    for name in workers():
        try:
            checked(["systemctl", "stop", name])
            checked(["systemctl", "reset-failed", name])
        except subprocess.SubprocessError:
            # A naturally finished instance can be collected between listing
            # and stop/reset. Absence is acceptable only after a fresh query.
            require(name not in workers(), "upload worker cleanup failed")
        if name in workers():
            s.inactive(name)
    empty_spool()


def emergency_stop(root: Path) -> None:
    # Deliberately independent of the controller lock: a stalled main process
    # must never prevent the deadline from closing admission and workers.
    load(root)
    checked(["systemctl", "stop", SOCKET])
    drain()


def restore(root: Path, plan: dict[str, object]) -> None:
    checked(["systemctl", "stop", SOCKET])
    drain()
    preserved(plan, snapshot(False), source_root(root) / "application.tar")
    if not plan["spoolExisted"] and SPOOL.exists():
        empty_spool()
        SPOOL.rmdir()
    plan["phase"] = "restored"
    save(root, plan)
    for name, data in guard_units(root).items():
        path = UNITS / name
        if path.exists() or path.is_symlink():
            meta = path.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                    and path.read_bytes() == data, "foreign guard unit")
    if (UNITS / (root.name + "-deadline.timer")).exists():
        checked(["systemctl", "stop", root.name + "-deadline.timer"])
    if (UNITS / (root.name + "-restore.service")).exists():
        checked(["systemctl", "stop", root.name + "-restore.service"])
    for name in guard_units(root):
        (UNITS / name).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])


def cleanup(root: Path, profile: probe.UploadProfile = probe.DEFAULT_PROFILE) -> None:
    source = source_root(root)
    if source.exists() or source.is_symlink():
        meta = source.lstat()
        require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o755
                and {path.name for path in source.iterdir()} <= {"probe.py", "application.tar"}, "foreign probe source")
        for path in source.iterdir():
            meta = path.lstat()
            require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                    and not meta.st_mode & 0o022, "unsafe probe source file")
            expected_hash = profile.bundle_sha if path.name == "application.tar" else u.digest(root / "probe_installed_upload.py")
            require(u.digest(path) == expected_hash, "probe source contents changed")
        for path in source.iterdir():
            path.unlink()
        source.rmdir()
    expected = {"controller.py", "plan.json", "bindings.json", "new.deb", *HELPERS}
    require({path.name for path in root.iterdir()} <= expected, "unexpected guard contents")
    for path in root.iterdir():
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1, "unsafe guard file")
    for name in expected:
        (root / name).unlink(missing_ok=True)
    root.rmdir()
    u.sync_directory(root.parent)


def restore_and_cleanup(root: Path) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load(root)
        restore(root, plan)
        cleanup(root, plan_profile(plan))
        return {"result": "restored", "guardRoot": str(root), "guardMayRemain": False,
                "newSlotPublished": plan.get("newSlotPublished", False)}
    finally:
        os.close(descriptor)


def run(assets: Path, profile: probe.UploadProfile = probe.DEFAULT_PROFILE) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")), "unfinished upload gate")
    require(not list(Path("/var/lib").glob("hardened-blikvm-application-update-*")), "unfinished application transaction")
    u.installed(assets, "new")
    require(u.digest(assets / "application.tar") == profile.bundle_sha, "unexpected candidate input")
    empty_spool()
    before = snapshot(False)
    publication_preflight(before, profile)
    require(not workers(), "old upload instances remain")
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    plan: dict[str, object] = {"schemaVersion": 1, "bundleSha256": profile.bundle_sha, "profile": profile.name, "phase": "prepared",
                              "publicationAttempted": False, "newSlotPublished": False,
                              "before": before, "spoolExisted": SPOOL.exists()}
    result: dict[str, object] = {"result": "failed", "profile": profile.name, "guardRoot": str(root), "guardMayRemain": True, "vectors": []}
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    prepared = False
    deadline = time.monotonic() + 450
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        for name in ("bindings.json", "new.deb", *HELPERS):
            u.new_file(root / name, (assets / name).read_bytes())
        u.new_file(root / "controller.py", Path(__file__).read_bytes())
        save(root, plan)
        prepared = True
        for name, data in guard_units(root).items():
            u.new_file(UNITS / name, data, 0o644)
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", root.name + "-deadline.timer"])
        checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"])
        source = source_root(root)
        source.mkdir(mode=0o755)
        u.new_file(source / "probe.py", (assets / "probe_installed_upload.py").read_bytes(), 0o644)
        u.new_file(source / "application.tar", (assets / "application.tar").read_bytes(), 0o644)
        plan["phase"] = "running"
        save(root, plan)
        require(time.monotonic() < deadline, "preparation exceeded guard budget")
        checked(["systemctl", "start", SOCKET])
        require(snapshot(True) == before, "starting upload changed retained state")
        for mode in modes(profile):
            require(time.monotonic() + 90 < deadline, "insufficient guard budget for next vector")
            if mode == "publish":
                plan["publicationAttempted"] = True
                save(root, plan)
            value = run_vector(source, mode, profile)
            published = preserved(plan, snapshot(True), source / "application.tar")
            require(published == profile.publish, "expected publication not observed")
            save(root, plan)
            cast(list[object], result["vectors"]).append(value)
        require(time.monotonic() < deadline, "native gate exceeded its deadline")
        result["result"] = "passed"
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
        result["errorType"] = type(error).__name__
    finally:
        try:
            if prepared:
                restore(root, plan)
            else:
                require(snapshot(False) == before, "state changed during partial preparation")
            cleanup(root, profile)
            result["guardMayRemain"] = False
            result["stateAndServingPreserved"] = True
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            result.update({"result": "failed", "restoreErrorType": type(error).__name__})
        os.close(descriptor)
    result["publicationAttempted"] = plan["publicationAttempted"]
    result["newSlotPublished"] = plan["newSlotPublished"]
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("run", "restore", "emergency-stop"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    parser.add_argument("--profile", choices=tuple(probe.PROFILES), default=probe.DEFAULT_PROFILE.name)
    args = parser.parse_args()
    require(os.geteuid() == 0, "root gate controller required")
    u.select_profile("dev7-dev9")
    if args.action == "run":
        result = run(args.assets, probe.PROFILES[args.profile])
    elif args.action == "emergency-stop":
        root = guard_root(args.guard_root)
        emergency_stop(root)
        result = {"result": "stopped-unverified", "guardRoot": str(root)}
    else:
        result = restore_and_cleanup(guard_root(args.guard_root))
    print(json.dumps(result, sort_keys=True))
    if result["result"] == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
