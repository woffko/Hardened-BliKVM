#!/usr/bin/env python3
"""Owned, backed-up upload-only Nginx streaming change; never enable writes."""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
import time

sys.dont_write_bytecode = True
if not __package__:
    import installed_persistent_write as native
else:
    from tools import installed_persistent_write as native

u, s, require = native.u, native.s, native.require
CONFIG = Path("/etc/hardened-blikvm/nginx.conf")
PREFIX = "hardened-blikvm-upload-stream-"
ASSETS = (*native.ASSETS, "installed_persistent_write.py")


def transform(original: bytes) -> bytes:
    require(0 < len(original) <= 65536 and b"\r" not in original, "unsupported proxy configuration")
    require(b"/api/application-update/upload" not in original and b"proxy_request_buffering" not in original,
            "upload streaming configuration already exists")
    matches = list(re.finditer(rb"(?m)^(?P<indent>[ \t]*)location / \{\n(?P<body>[^{}]*?)^(?P=indent)\}", original))
    require(len(matches) == 1, "one simple default proxy location required")
    match = matches[0]
    body, indent = match["body"], match["indent"]
    statements = [line.strip() for line in body.splitlines() if line.strip()]
    allowed = {b"proxy_pass", b"proxy_http_version", b"proxy_set_header", b"proxy_buffering", b"proxy_read_timeout"}
    require(all(line.endswith(b";") and line.split()[0] in allowed for line in statements), "unreviewed proxy location directives")
    require(statements.count(b"proxy_pass http://127.0.0.1:18080;") == 1
            and statements.count(b"proxy_http_version 1.1;") == 1, "unexpected proxy upstream")
    addition = (indent + b"location = /api/application-update/upload {\n" + body
                + indent + b"    proxy_request_buffering off;\n" + indent + b"}\n")
    return original[:match.start()] + addition + original[match.start():]


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def root_path(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + r"[a-f0-9]{24}", root.name) is not None,
            "invalid proxy backup namespace")
    meta = root.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700
            and root.resolve(strict=True) == root, "unsafe proxy backup root")


def config_bytes() -> bytes:
    meta = CONFIG.lstat()
    require(CONFIG.resolve(strict=True) == CONFIG and stat.S_ISREG(meta.st_mode) and meta.st_uid == 0
            and meta.st_gid == 0 and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) in (0o600, 0o640, 0o644)
            and meta.st_size <= 65536, "unsafe Nginx configuration")
    return CONFIG.read_bytes()


def workers(master: str) -> set[str]:
    require(master.isascii() and master.isdecimal() and int(master) > 0, "invalid Nginx master")
    children = (Path("/proc") / master / "task" / master / "children").read_text().split()
    require(len(children) <= 128 and all(pid.isascii() and pid.isdecimal() for pid in children), "invalid Nginx children")
    result: set[str] = set()
    for pid in children:
        try:
            process = Path("/proc") / pid
            if b"nginx: worker process" not in (process / "cmdline").read_bytes()[:4096]:
                continue
            started = (process / "stat").read_text().rsplit(")", 1)[1].split()[19]
            require(started.isdecimal(), "invalid Nginx worker start time")
            result.add(pid + ":" + started)
        except FileNotFoundError:
            continue
    return result


def load(root: Path) -> dict[str, object]:
    root_path(root)
    plan = native.object_value(json.loads(native.private_file(root / "plan.json", limit=2 * 1024 * 1024),
        object_pairs_hook=native.temporary.upload.probe.unique_object))
    require(set(plan) == {"phase", "before", "mode", "originalSha256", "candidateSha256", "files", "workersBefore"}
            and plan["phase"] in ("prepared", "applying", "accepted", "restored")
            and type(plan["mode"]) is int and plan["mode"] in (0o600, 0o640, 0o644), "invalid proxy plan")
    manifests = native.object_value(plan["files"])
    require(isinstance(plan["workersBefore"], list)
            and all(isinstance(item, str) and re.fullmatch(r"[0-9]+:[0-9]+", item) is not None
                    for item in native.cast(list[object], plan["workersBefore"])), "invalid worker baseline")
    require(set(manifests) == {*ASSETS, "controller.py"}, "proxy recovery code manifest differs")
    for name, expected in manifests.items():
        require(digest(native.private_file(root / name)) == expected, "proxy recovery code changed")
    original = native.private_file(root / "original.conf", limit=65536)
    candidate = native.private_file(root / "candidate.conf", limit=65536)
    require(digest(original) == plan["originalSha256"] and digest(candidate) == plan["candidateSha256"]
            and transform(original) == candidate, "proxy backup binding differs")
    return plan


def save(root: Path, plan: dict[str, object]) -> None:
    part = root / ("plan-" + secrets.token_hex(12) + ".part")
    u.new_file(part, json.dumps(plan, sort_keys=True, allow_nan=False).encode())
    os.replace(part, root / "plan.json")
    u.sync_directory(root)


def replace_config(root: Path, plan: dict[str, object], data: bytes) -> None:
    require(digest(config_bytes()) in (plan["originalSha256"], plan["candidateSha256"]), "foreign proxy change; not overwriting")
    require(stat.S_IMODE(CONFIG.stat().st_mode) == plan["mode"], "proxy file mode changed")
    part = CONFIG.parent / ("." + root.name + "-" + secrets.token_hex(12) + ".part")
    u.new_file(part, data, mode=native.cast(int, plan["mode"]))
    os.replace(part, CONFIG)
    u.sync_directory(CONFIG.parent)


def verify(root: Path, plan: dict[str, object], *, changed: bool) -> None:
    expected = copy.deepcopy(native.object_value(plan["before"]))
    native.object_value(expected["runtime"])["nginxConfig"] = plan["candidateSha256" if changed else "originalSha256"]
    end = time.monotonic() + 20
    seen: set[str] = set()
    while time.monotonic() < end:
        try:
            u.installed(root, "new")
            require(native.temporary.upload.boot.seq8_snapshot() == expected, "proxy change disturbed protected runtime/state")
            if changed:
                master = native.cast(str, native.object_value(expected["runtime"])["nginxPid"])
                fresh = workers(master) - set(native.cast(list[str], plan["workersBefore"]))
                require(bool(fresh), "new Nginx worker generation not observed")
                if not fresh & seen:
                    seen = fresh
                    time.sleep(0.5)
                    continue
            return
        except (OSError, ValueError, subprocess.SubprocessError):
            time.sleep(0.5)
    raise TimeoutError("proxy runtime verification failed")


def recover(root: Path) -> None:
    with native.temporary.guard.locked(root):
        plan = load(root)
        if plan["phase"] == "accepted":
            return
        if plan["phase"] == "restored":
            verify(root, plan, changed=False)
            return
        if plan["phase"] != "prepared" or digest(config_bytes()) != plan["originalSha256"]:
            replace_config(root, plan, native.private_file(root / "original.conf"))
            u.checked(["/usr/sbin/nginx", "-t", "-c", str(CONFIG)], timeout=10)
            u.checked(["systemctl", "reload", s.NGINX], timeout=20)
        verify(root, plan, changed=False)
        plan["phase"] = "restored"
        save(root, plan)


def apply(root: Path) -> None:
    require(any(line.endswith("/" + root.name + ".service") for line in Path("/proc/self/cgroup").read_text().splitlines()),
            "proxy apply requires owned systemd service")
    with native.temporary.guard.locked(root):
        plan = load(root)
        require(plan["phase"] == "prepared" and digest(config_bytes()) == plan["originalSha256"], "proxy apply is not fresh")
        verify(root, plan, changed=False)
        u.checked(["/usr/sbin/nginx", "-t", "-c", str(root / "candidate.conf")], timeout=10)
        plan["phase"] = "applying"
        save(root, plan)
        replace_config(root, plan, native.private_file(root / "candidate.conf"))
        u.checked(["/usr/sbin/nginx", "-t", "-c", str(CONFIG)], timeout=10)
        u.checked(["systemctl", "reload", s.NGINX], timeout=20)
        verify(root, plan, changed=True)
        plan["phase"] = "accepted"
        save(root, plan)


def service_command(root: Path, *, proof: str | None = None) -> list[str]:
    name = root.name + ("-proof-" + proof if proof is not None else "")
    stop = ("guard-proof --proof " + proof) if proof is not None else "recover"
    program = (["/usr/bin/false"] if proof == "failure" else ["/usr/bin/sleep", "10"]) if proof is not None else [
        "/usr/bin/python3", "-B", str(root / "controller.py"), "apply", "--root", str(root)]
    return ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + name,
        "--property=Type=oneshot", "--property=User=root", "--property=UMask=0077", "--property=NoNewPrivileges=yes",
        "--property=TimeoutStartSec=" + ("1" if proof == "timeout" else "120"),
        "--property=TimeoutStopSec=90", "--property=KillMode=control-group",
        "--property=ExecStopPost=/usr/bin/python3 -B " + str(root / "controller.py") + " " + stop + " --root " + str(root),
        *program]


def prove_recovery_hook(root: Path) -> None:
    for kind in ("failure", "timeout"):
        result = subprocess.run(service_command(root, proof=kind), capture_output=True, timeout=110, check=False)
        require(result.returncode != 0, "recovery proof main process unexpectedly succeeded")
        require(native.private_file(root / ("proof-" + kind), limit=64) == b"stop-post-ran\n", "systemd recovery hook unproven")


def guard_proof(root: Path, kind: str) -> None:
    load(root)
    require(kind in ("failure", "timeout") and any(line.endswith("/" + root.name + "-proof-" + kind + ".service")
            for line in Path("/proc/self/cgroup").read_text().splitlines()), "wrong recovery proof context")
    u.new_file(root / ("proof-" + kind), b"stop-post-ran\n")


def prepare(assets: Path, root: Path) -> dict[str, object]:
    # Serialize preparations even though the apply child uses its own root
    # lock. An orphaned unaccepted transaction blocks a later preparation.
    with native.temporary.guard.locked(CONFIG.parent):
        return prepare_locked(assets, root)


def prepare_locked(assets: Path, root: Path) -> dict[str, object]:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + r"[a-f0-9]{24}", root.name) is not None
            and not root.exists() and not root.is_symlink(), "new proxy backup root required")
    for previous in root.parent.glob(PREFIX + "*"):
        prior = load(previous)
        require(prior["phase"] in ("accepted", "restored"), "unfinished proxy transaction requires recovery")
    require(not os.statvfs(CONFIG.parent).f_flag & os.ST_RDONLY, "configuration filesystem must already be writable")
    asset_meta = assets.lstat()
    require(assets.resolve(strict=True) == assets and stat.S_ISDIR(asset_meta.st_mode)
            and asset_meta.st_uid == 0 and stat.S_IMODE(asset_meta.st_mode) == 0o700, "unsafe proxy staging")
    for name in ASSETS:
        native.private_file(assets / name)
    u.installed(assets, "new")
    before = native.temporary.upload.boot.seq8_snapshot()
    original = config_bytes()
    require(digest(original) == native.object_value(before["runtime"])["nginxConfig"], "proxy changed during preflight")
    candidate = transform(original)
    worker_set = workers(native.cast(str, native.object_value(before["runtime"])["nginxPid"]))
    require(bool(worker_set), "Nginx workers unavailable")
    u.checked(["/usr/sbin/nginx", "-t", "-c", str(CONFIG)], timeout=10)
    require(s.property_value(s.NGINX, "CanReload") == "yes", "Nginx reload unavailable")
    root.mkdir(mode=0o700)
    root.chmod(0o700)
    u.sync_directory(root.parent)
    for name in ASSETS:
        u.new_file(root / name, native.private_file(assets / name))
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    u.new_file(root / "original.conf", original)
    u.new_file(root / "candidate.conf", candidate)
    plan: dict[str, object] = {"phase": "prepared", "before": before, "mode": stat.S_IMODE(CONFIG.stat().st_mode),
        "originalSha256": digest(original), "candidateSha256": digest(candidate), "workersBefore": sorted(worker_set),
        "files": {name: u.digest(root / name) for name in (*ASSETS, "controller.py")}}
    save(root, plan)
    load(root)
    prove_recovery_hook(root)
    # systemd owns both the bounded apply process and its failure restoration.
    # ExecStopPost also runs on timeout/disconnect; accepted state is a no-op.
    u.checked(service_command(root), timeout=240)
    plan = load(root)
    require(plan["phase"] == "accepted", "proxy change not accepted")
    return {"result": "passed", "backupRoot": str(root), "originalSha256": plan["originalSha256"],
        "candidateSha256": plan["candidateSha256"], "runtimePreserved": True, "runtimeWritesClosed": True,
        "recoveryHooksVerified": True, "freshWorkersVerified": True, "oldWorkerDrainRequired": False}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "apply", "recover", "guard-proof"))
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--proof", choices=("failure", "timeout"))
    args = parser.parse_args()
    require(os.geteuid() == 0 and (args.assets is not None) == (args.action == "prepare"), "invalid root controller invocation")
    require((args.proof is not None) == (args.action == "guard-proof"), "invalid proof argument")
    u.select_profile("dev7-dev9")
    if args.action == "prepare":
        result = prepare(args.assets, args.root)
    else:
        root_path(args.root)
        if args.action == "guard-proof":
            guard_proof(args.root, args.proof)
        else:
            (apply if args.action == "apply" else recover)(args.root)
        result = {"result": args.action, "backupRoot": str(args.root)}
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
