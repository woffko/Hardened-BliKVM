#!/usr/bin/env python3
"""Read-only OS/package audit. No apt update, installation, reload or reboot."""

import argparse
import json
import shlex
import subprocess
import sys
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

REMOTE = r'''
import json, os, pathlib, re, shutil, subprocess, time
from urllib.parse import urlsplit

def run(argv, timeout=15):
    result = subprocess.run(argv, capture_output=True, timeout=timeout, check=False)
    if len(result.stdout) + len(result.stderr) > 2097152:
        raise ValueError("oversize audit result")
    return result.returncode, result.stdout.decode(errors="replace")

def release(path, allowed):
    source = pathlib.Path(path)
    if not source.exists():
        return {}
    raw = source.read_text()
    if len(raw) > 16384:
        raise ValueError("oversize release metadata")
    result = {}
    for line in raw.splitlines():
        key, sep, value = line.partition("=")
        value = value.strip().strip('"').strip("'")
        if sep and key in allowed and re.fullmatch(r"[A-Za-z0-9 ._()/-]{1,120}", value):
            result[key] = value
    return result

facts = {"cacheOnly": True, "packagesInstalledByProbe": False,
    "os": release("/etc/os-release", {"ID", "NAME", "VERSION_ID", "VERSION_CODENAME", "PRETTY_NAME"}),
    "armbian": release("/etc/armbian-release", {"BOARD", "BOARD_NAME", "VERSION", "LINUXFAMILY", "BRANCH", "ARCH"}),
    "kernel": os.uname().release, "architecture": os.uname().machine}
facts["filesystems"] = {}
for path in ("/", "/boot", "/var"):
    disk = shutil.disk_usage(path)
    facts["filesystems"][path] = {"totalBytes": disk.total, "freeBytes": disk.free,
        "readOnly": bool(os.statvfs(path).f_flag & os.ST_RDONLY)}
code, packages = run(["dpkg-query", "-W", "-f=${binary:Package}\t${Version}\t${db:Status-Abbrev}\n"])
if code:
    raise ValueError("package inventory unavailable")
selected = {}
for line in packages.splitlines():
    fields = line.split("\t")
    if len(fields) != 3 or len(fields[2]) < 2 or fields[2][1] != "i":
        continue
    name, version = fields[:2]
    if re.match(r"^(nginx|kvmd|ustreamer|linux-(image|dtb|headers|u-boot)|armbian|u-boot|systemd|openssl|libssl|libc6|python3($|[.:-])|hardened-blikvm)", name):
        selected[name] = version
facts["installed"] = selected
code, layout = run(["lsblk", "--json", "--bytes", "--output", "NAME,TYPE,SIZE,FSTYPE,MOUNTPOINT"])
facts["blockLayout"] = json.loads(layout) if code == 0 else {"queryExit": code}
code, holds = run(["apt-mark", "showhold"])
facts["holds"] = [name for name in holds.splitlines() if re.fullmatch(r"[A-Za-z0-9+.:_-]+", name)]
facts["holdsQueryExit"] = code
code, audit = run(["dpkg", "--audit"])
facts["dpkgAudit"] = {"exitCode": code, "clean": code == 0 and not audit.strip(), "lines": len(audit.splitlines())}
files = [p for p in pathlib.Path("/var/lib/apt/lists").glob("*") if p.is_file() and p.name != "lock"]
facts["aptCacheNewestAgeSeconds"] = max(0, int(time.time() - max(p.stat().st_mtime for p in files))) if files else None
origins, suites = set(), set()
sources = [pathlib.Path("/etc/apt/sources.list"), *pathlib.Path("/etc/apt/sources.list.d").glob("*.list"),
           *pathlib.Path("/etc/apt/sources.list.d").glob("*.sources")]
known = {"deb.debian.org", "security.debian.org", "archive.debian.org", "archive.ubuntu.com", "security.ubuntu.com", "ports.ubuntu.com", "apt.armbian.com"}
for source in sources:
    if not source.is_file():
        continue
    raw = source.read_text()
    if len(raw) > 65536:
        raise ValueError("oversize repository metadata")
    for line in raw.splitlines():
        if line.lstrip().startswith("#"):
            continue
        for value in re.findall(r"https?://[^\s]+", line):
            host = urlsplit(value).hostname
            origins.add(host if host in known else "other-repository")
        for value in re.findall(r"\b(?:buster|bullseye|bookworm|trixie|focal|jammy|noble)(?:-updates|-security|-backports)?\b", line):
            suites.add(value)
facts["repositoryClasses"] = sorted(origins)
facts["repositorySuites"] = sorted(suites)
code, simulation = run(["apt-get", "-s", "-o", "Debug::NoLocking=1", "upgrade"], timeout=60)
facts["upgradeSimulation"] = {"exitCode": code, "install": [], "remove": []}
for line in simulation.splitlines():
    match = re.match(r"Inst ([A-Za-z0-9+.:_-]+)(?: \[[^\]]+\])? \(([^\s()]+)", line)
    if match:
        facts["upgradeSimulation"]["install"].append({"package": match[1], "version": match[2]})
    match = re.match(r"Remv ([A-Za-z0-9+.:_-]+)", line)
    if match:
        facts["upgradeSimulation"]["remove"].append(match[1])
facts["services"] = {}
for unit in ("kvmd.service", "kvmd-nginx.service", "hardened-blikvm-nginx.service", "hardened-blikvm-server-current.service", "hardened-blikvm-platform-broker.service", "hardened-blikvm-video-proxy.service"):
    code, raw = run(["systemctl", "show", unit, "--property=LoadState,ActiveState,SubState"])
    facts["services"][unit] = {key: value for line in raw.splitlines() for key, sep, value in [line.partition("=")]
        if sep and key in ("LoadState", "ActiveState", "SubState")}
print(json.dumps(facts, sort_keys=True))
'''

REMOTE_STORAGE = r'''
import json, os, pathlib, subprocess

def run(argv, timeout=15):
    result = subprocess.run(argv, capture_output=True, timeout=timeout, check=False)
    if len(result.stdout) + len(result.stderr) > 262144:
        raise ValueError("oversize storage probe")
    return result.returncode, result.stdout.decode(errors="replace")

facts = {}
block = pathlib.Path("/sys/class/block/mmcblk0")
facts["deviceBytes"] = int((block / "size").read_text()) * 512
kind = (block / "device/type").read_text().strip()
facts["deviceType"] = kind if kind in ("SD", "MMC") else "other"
code, raw = run(["sfdisk", "--json", "/dev/mmcblk0"])
if code:
    raise ValueError("partition table unavailable")
table = json.loads(raw)["partitiontable"]
sector = table.get("sectorsize", 512)
facts["partitions"] = [{"name": pathlib.Path(item["node"]).name,
    "startBytes": item["start"] * sector, "sizeBytes": item["size"] * sector}
    for item in table["partitions"]]
facts["bytesAfterLastPartition"] = facts["deviceBytes"] - max(item["startBytes"] + item["sizeBytes"] for item in facts["partitions"])
facts["filesystems"] = {}
for name in ("/", "/mnt", "/var/lib/kvmd/msd"):
    v = os.statvfs(name)
    facts["filesystems"][name] = {"totalBytes": v.f_blocks * v.f_frsize,
        "usedBytes": (v.f_blocks - v.f_bfree) * v.f_frsize,
        "availableBytes": v.f_bavail * v.f_frsize,
        "reservedFreeBytes": (v.f_bfree - v.f_bavail) * v.f_frsize}
code, raw = run(["du", "-x", "-B1", "--max-depth=1", "/"], timeout=120)
allowed = {"/", "/usr", "/var", "/opt", "/root", "/home", "/tmp", "/boot", "/etc", "/srv", "/bin", "/sbin", "/lib", "/lib64", "/lost+found", "/media", "/mnt", "/run", "/dev", "/sys", "/proc"}
sizes = {}
for line in raw.splitlines():
    count, sep, name = line.partition("\t")
    if sep and count.isdecimal():
        key = name if name in allowed else "otherRootDirectories"
        sizes[key] = sizes.get(key, 0) + int(count)
facts["rootDirectoryBytes"] = sizes
facts["duExitCode"] = code
facts["selectedDirectoryBytes"] = {}
for name in ("/var/cache/apt", "/var/lib/apt/lists", "/var/lib/hardened-blikvm-slots", "/var/lib/hardened-blikvm-update", "/root/.cache", "/root/.cargo"):
    if not pathlib.Path(name).is_dir():
        continue
    code, raw = run(["du", "-x", "-B1", "-s", name], timeout=30)
    count = raw.partition("\t")[0]
    if code == 0 and count.isdecimal():
        facts["selectedDirectoryBytes"][name] = int(count)
print(json.dumps(facts, sort_keys=True))
'''

REMOTE_CLEANUP = r'''
import json, pathlib, stat, subprocess

def size(path):
    if not path.exists():
        return 0
    result = subprocess.run(["du", "-x", "-B1", "-s", str(path)], capture_output=True, timeout=45, check=False)
    if result.returncode or len(result.stdout) > 8192:
        return None
    count = result.stdout.decode().partition("\t")[0]
    return int(count) if count.isdecimal() else None

facts = {"candidates": {}}
for name in ("/var/cache/apt/archives", "/var/cache/apt/pkgcache.bin", "/var/cache/apt/srcpkgcache.bin",
             "/var/lib/apt/lists", "/var/lib/systemd/coredump", "/var/crash", "/tmp", "/var/tmp",
             "/root/.cache", "/root/.npm", "/root/.cargo", "/root/.rustup"):
    facts["candidates"][name] = size(pathlib.Path(name))
archives = pathlib.Path("/var/cache/apt/archives")
debs = [p for p in archives.glob("*.deb") if stat.S_ISREG(p.lstat().st_mode)]
facts["cachedDebCount"] = len(debs)
facts["cachedDebFileBytes"] = sum(p.stat().st_size for p in debs)
facts["homeCaches"] = []
for home in sorted(pathlib.Path("/home").iterdir()):
    if not home.is_dir() or home.is_symlink():
        continue
    facts["homeCaches"].append({name: size(home / name) for name in (".cache", ".npm", ".cargo", ".rustup")})
# Classify large files without exposing private names or reading their contents.
facts["largeFileCategories"] = {}
for base in ("/root", "/home", "/opt"):
    root = pathlib.Path(base)
    device = root.stat().st_dev
    result = subprocess.run(["find", base, "-xdev", "-type", "f", "-size", "+50M", "-print0"],
                            capture_output=True, timeout=45, check=False)
    if len(result.stdout) > 65536:
        raise ValueError("oversize large-file inventory")
    groups = {}
    for name in result.stdout.split(b"\0"):
        if not name:
            continue
        p = pathlib.Path(name.decode())
        meta = p.lstat()
        if not stat.S_ISREG(meta.st_mode) or meta.st_dev != device:
            continue
        suffix = p.suffix.lower()
        category = suffix if suffix in (".deb", ".iso", ".img", ".gz", ".xz", ".zip", ".tar", ".log") else "other"
        entry = groups.setdefault(category, {"count": 0, "allocatedBytes": 0})
        entry["count"] += 1
        entry["allocatedBytes"] += meta.st_blocks * 512
    facts["largeFileCategories"][base] = groups
print(json.dumps(facts, sort_keys=True))
'''


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    modes = parser.add_mutually_exclusive_group()
    modes.add_argument("--storage-only", action="store_true")
    modes.add_argument("--cleanup-audit", action="store_true")
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new system audit receipt required")
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root", common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    mode = "cleanup-audit" if args.cleanup_audit else "storage-audit" if args.storage_only else "system-upgrade-audit"
    receipt: dict[str, object] = {"probe": mode, "result": "failed", "readOnly": True}
    secret = common.transport.read_secret_stdin()
    try:
        remote = REMOTE_CLEANUP if args.cleanup_audit else REMOTE_STORAGE if args.storage_only else REMOTE
        result = common.transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", remote]),
            timeout=330 if args.storage_only or args.cleanup_audit else 140)
        common.require(result.returncode == 0 and len(result.stdout) <= 262144, "system audit failed")
        receipt["facts"] = json.loads(result.stdout)
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
