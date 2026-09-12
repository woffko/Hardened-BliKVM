#!/usr/bin/env python3
"""Move reviewed APT cache files to /mnt only after durable verified copies."""

import argparse
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import fcntl, hashlib, json, os, pathlib, pwd, re, stat, sys

def require(value, message):
    if not value:
        raise ValueError(message)

def sync(path):
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try: os.fsync(fd)
    finally: os.close(fd)

def space(path):
    value = os.statvfs(path)
    return {"availableBytes": value.f_bavail * value.f_frsize,
            "totalBytes": value.f_blocks * value.f_frsize}

def signature(value):
    return [value.st_dev, value.st_ino, value.st_size, value.st_mtime_ns,
            value.st_ctime_ns, value.st_uid, value.st_gid, value.st_mode, value.st_nlink]

def sha(path):
    digest = hashlib.sha256()
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW), "rb") as stream:
        require(stat.S_ISREG(os.fstat(stream.fileno()).st_mode), "not a regular cache file")
        for chunk in iter(lambda: stream.read(1048576), b""): digest.update(chunk)
    return digest.hexdigest()

target = pathlib.Path(sys.argv[1])
require(os.geteuid() == 0 and target.parent == pathlib.Path("/mnt")
        and re.fullmatch(r"blikvm-apt-cache-[a-f0-9]{24}", target.name), "invalid cache backup target")
require(os.path.ismount("/mnt") and pathlib.Path("/mnt").resolve() == pathlib.Path("/mnt")
        and os.stat("/mnt").st_dev != os.stat("/").st_dev, "destination is not a separate mounted filesystem")
require(not target.exists() and not target.is_symlink(), "backup target already exists")
require(not os.statvfs("/var/cache/apt").f_flag & os.ST_RDONLY, "APT cache filesystem is read-only")
locks = []
try:
    for name in ("/var/lib/dpkg/lock-frontend", "/var/lib/dpkg/lock", "/var/lib/apt/lists/lock", "/var/cache/apt/archives/lock"):
        fd = os.open(name, os.O_RDWR | os.O_NOFOLLOW)
        locks.append(fd)
        meta = os.fstat(fd)
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1, "unsafe package lock")
        fcntl.lockf(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    owners = {0}
    try: owners.add(pwd.getpwnam("_apt").pw_uid)
    except KeyError: pass
    archive = pathlib.Path("/var/cache/apt/archives")
    sources = sorted(archive.glob("*.deb"))
    derived = [p for p in (pathlib.Path("/var/cache/apt/pkgcache.bin"), pathlib.Path("/var/cache/apt/srcpkgcache.bin"))
               if p.exists() or p.is_symlink()]
    entries = []
    disposable = []
    for path in [*sources, *derived]:
        meta = path.lstat()
        require(path.resolve(strict=True) == path and stat.S_ISREG(meta.st_mode) and meta.st_nlink == 1
                and meta.st_uid in owners and not meta.st_mode & 0o022 and meta.st_dev == os.stat("/").st_dev,
                "unreviewed cache file metadata")
        item = {"source": str(path), "target": "archives/" + path.name,
                "signature": signature(meta), "bytes": meta.st_size, "allocatedBytes": meta.st_blocks * 512}
        if path.parent == archive:
            entries.append(item)
        else:
            item.pop("target")
            disposable.append(item)
    total = sum(item["bytes"] for item in entries)
    require(len(entries) <= 2048 and total <= 1073741824 and space("/mnt")["availableBytes"] > total + 67108864,
            "cache move exceeds reviewed size or destination capacity")
    before = {"root": space("/"), "data": space("/mnt")}
    package_hash = sha("/var/lib/dpkg/status")
    target.mkdir(mode=0o700)
    (target / "archives").mkdir(mode=0o700)
    sync(target.parent)
    for item in entries:
        source, destination = pathlib.Path(item["source"]), target / item["target"]
        digest = hashlib.sha256()
        with os.fdopen(os.open(source, os.O_RDONLY | os.O_NOFOLLOW), "rb") as incoming:
            require(signature(os.fstat(incoming.fileno())) == item["signature"], "cache changed before copy")
            with os.fdopen(os.open(destination, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "wb") as outgoing:
                for chunk in iter(lambda: incoming.read(1048576), b""):
                    outgoing.write(chunk)
                    digest.update(chunk)
                outgoing.flush()
                os.fsync(outgoing.fileno())
        item["sha256"] = digest.hexdigest()
        require(sha(destination) == item["sha256"] and destination.stat().st_size == item["bytes"], "backup verification failed")
    sync(target / "archives")
    # Verify every source before removing any. A failed copy leaves all sources.
    for item in entries:
        path = pathlib.Path(item["source"])
        require(signature(path.lstat()) == item["signature"] and sha(path) == item["sha256"], "source changed during backup")
    manifest = {"phase": "verified", "entries": entries, "regenerableCaches": disposable,
                "packageStatusSha256": package_hash}
    def save_manifest():
        part = target / "manifest.part"
        with os.fdopen(os.open(part, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600), "w") as output:
            json.dump(manifest, output, sort_keys=True)
            output.flush()
            os.fsync(output.fileno())
        os.replace(part, target / "manifest.json")
        sync(target)
    save_manifest()
    for item in [*entries, *disposable]:
        path = pathlib.Path(item["source"])
        require(signature(path.lstat()) == item["signature"], "source changed before removal")
        path.unlink()
    sync(archive)
    sync(archive.parent)
    require(sha("/var/lib/dpkg/status") == package_hash, "package database changed")
    manifest["phase"] = "completed"
    save_manifest()
    print(json.dumps({"result": "passed", "backupRoot": str(target), "debCount": sum(p.suffix == ".deb" for p in sources),
        "derivedCacheCount": len(disposable), "verifiedFileBytes": total,
        "sourceAllocatedBytes": sum(item["allocatedBytes"] for item in [*entries, *disposable]), "before": before,
        "after": {"root": space("/"), "data": space("/mnt")}, "packageDatabaseUnchanged": True}, sort_keys=True))
finally:
    for fd in reversed(locks): os.close(fd)
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new relocation receipt required")
    backup = "/mnt/blikvm-apt-cache-" + secrets.token_hex(12)
    receipt: dict[str, object] = {"probe": "apt-cache-relocation", "result": "failed", "backupRoot": backup, "submitted": False}
    write_json_0600_atomic(output, receipt)
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root", common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = common.transport.read_secret_stdin()
    try:
        receipt["submitted"] = True
        write_json_0600_atomic(output, receipt)
        result = common.transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", REMOTE, backup]), timeout=360)
        common.require(result.returncode == 0 and len(result.stdout) <= 16384, "cache relocation failed or is ambiguous")
        value = common.native.object_value(json.loads(result.stdout))
        common.require(value.get("result") == "passed" and value.get("backupRoot") == backup
                       and value.get("packageDatabaseUnchanged") is True, "cache relocation unproven")
        receipt["device"] = value
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        receipt["independentInspectionRequired"] = True
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
