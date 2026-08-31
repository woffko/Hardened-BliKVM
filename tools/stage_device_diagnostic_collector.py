#!/usr/bin/env python3
"""Run a reversible transient diagnostic-collector gate on the test H616."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import secrets
import shlex
import stat
import struct
import subprocess
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import stage_device_system_action_store as native


UNIT_NAME = "hardened-blikvm-diagnostic-collector.service"
COLLECTOR_ACCOUNT = "hardened-blikvm-diagnostics"
SHARED_GROUP = "hardened-blikvm"
RUNTIME_DIRECTORY = "/run/hardened-blikvm-diagnostics"
SOCKET_PATH = f"{RUNTIME_DIRECTORY}/collector.sock"
BINARY_PARENT = "/usr/libexec/hardened-blikvm"
BINARY_TARGET = f"{BINARY_PARENT}/bli-kvm-diagnostic-collector"
PRODUCER_ACCOUNTS = (
    "hardened-blikvm-server",
    "hardened-blikvm-broker",
    "hardened-blikvm-video",
    "hardened-blikvm-control",
    "hardened-blikvm-update",
)
TRANSIENT_IDENTITY_ACCOUNTS = (SHARED_GROUP, *PRODUCER_ACCOUNTS)
MAX_BINARY_BYTES = 16 * 1024 * 1024
MAX_CONFIG_BYTES = 64 * 1024
MIN_TMP_HEADROOM_BYTES = 16 * 1024 * 1024
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-diagnostic-collector-[0-9a-f]{12}-[0-9a-f]{16}$"
)
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-diagnostic-collector-[0-9a-f]{64}$"
)
PREFLIGHT_KEYS = {
    "account_absent_before",
    "architecture",
    "binary_absent_before",
    "binary_parent_before",
    "identities_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "rootfs_mode_before",
    "runtime_absent_before",
    "stage_absent_before",
    "systemd_ready",
    "tools_ready",
    "unit_absent_before",
    "watchdog_absent_before",
}
RUN_KEYS = {
    "binary_hash_verified",
    "collector_caps",
    "collector_dynamic_user",
    "collector_network_namespace",
    "collector_no_new_privs",
    "collector_unit",
    "duplicate_rejected",
    "five_sources_accepted",
    "graceful_shutdown",
    "kvmd_active_after_run",
    "kvmd_nginx_active_after_run",
    "malformed_rejected",
    "partial_timeout",
    "root_rejected",
    "rollback_confirmed",
    "runtime_removed",
    "socket_mode",
    "socket_owner",
    "sysusers_synthetic",
    "transient_identities",
    "unit_hash_verified",
    "unknown_rejected",
    "watchdog_armed",
}
FINAL_KEYS = {
    "account_absent_final",
    "binary_absent_final",
    "binary_parent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "rootfs_mode_final",
    "runtime_absent_final",
    "stage_absent_final",
    "unit_absent_final",
    "watchdog_inactive_final",
}
FAILURE_KEYS = {"failure_phase"}
CLEANUP_KEYS = {"cleanup_restored"}
SAFE_PHASE = re.compile(r"^[a-z0-9-]{1,80}$")


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


class PolicyReceipt(TypedDict):
    accountMode: str
    deviceActionsExecuted: bool
    networkAccess: str
    persistentChangeScope: str
    persistentStateRestored: bool
    physicalPowerOrReset: bool
    rawLogsRead: bool
    rollbackWatchdog: str


@dataclass(frozen=True)
class Options:
    binary: str
    source_commit: str
    unit: str
    sysusers: str
    endpoint: str
    known_hosts: str
    output: str


@dataclass(frozen=True)
class Asset:
    path: Path
    sha256: str
    size_bytes: int


@dataclass(frozen=True)
class Assets:
    binary: Asset
    unit: Asset
    sysusers: Asset
    source_commit: str


@dataclass(frozen=True)
class Paths:
    root: str
    marker: str
    marker_value: str
    binary: str
    unit: str
    sysusers: str
    client: str
    rollback: str
    state: str
    unit_target: str
    binary_parent: str
    binary_target: str
    binary_part: str
    runtime_directory: str
    socket: str
    client_state: str
    watchdog: str
    identity_units: tuple[str, ...]


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument(
        "--unit",
        default="packaging/systemd/hardened-blikvm-diagnostic-collector.service",
    )
    parser.add_argument(
        "--sysusers",
        default="packaging/sysusers.d/hardened-blikvm-diagnostics.conf",
    )
    parser.add_argument("--endpoint", required=True)
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument(
        "--output",
        default="evidence/private/device-diagnostic-collector.json",
    )
    namespace = parser.parse_args()
    return Options(
        binary=cast(str, namespace.binary),
        source_commit=cast(str, namespace.source_commit),
        unit=cast(str, namespace.unit),
        sysusers=cast(str, namespace.sysusers),
        endpoint=cast(str, namespace.endpoint),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
    )


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def hash_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def validate_binary(path_value: str) -> Asset:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("binary must not be a symlink")
    path = source.resolve(strict=True)
    if path.parent != Path("/tmp") or LOCAL_SNAPSHOT.fullmatch(path.name) is None:
        raise ValueError("binary must be an exact hash-named local snapshot")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_BINARY_BYTES
        or metadata.st_mode & 0o022
        or metadata.st_nlink != 1
        or metadata.st_uid != os.geteuid()
    ):
        raise ValueError("binary metadata is unsafe")
    with path.open("rb") as handle:
        header = handle.read(64)
        if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01":
            raise ValueError("binary is not ELF64 little-endian")
        elf_type, machine = struct.unpack_from("<HH", header, 16)
        program_offset = struct.unpack_from("<Q", header, 32)[0]
        program_entry_size = struct.unpack_from("<H", header, 54)[0]
        program_count = struct.unpack_from("<H", header, 56)[0]
        if elf_type != 3 or machine != 183 or program_entry_size < 56 or program_count == 0:
            raise ValueError("binary is not an AArch64 PIE")
        handle.seek(program_offset)
        program_types: list[int] = []
        for _ in range(program_count):
            entry = handle.read(program_entry_size)
            if len(entry) != program_entry_size:
                raise ValueError("binary program headers are truncated")
            program_types.append(struct.unpack_from("<I", entry)[0])
        if 3 in program_types or 1 not in program_types:
            raise ValueError("binary is not static PIE")
    sha256 = hash_file(path)
    if path.name != f"hardened-blikvm-diagnostic-collector-{sha256}":
        raise ValueError("binary snapshot name does not match SHA-256")
    return Asset(path, sha256, metadata.st_size)


def validate_config(path_value: str, expected: Path) -> Asset:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("configuration source must not be a symlink")
    path = source.resolve(strict=True)
    if path != expected.resolve(strict=True):
        raise ValueError("configuration source path is not exact")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_CONFIG_BYTES
        or metadata.st_mode & 0o022
    ):
        raise ValueError("configuration source metadata is unsafe")
    return Asset(path, hash_file(path), metadata.st_size)


def validate_assets(options: Options) -> Assets:
    if SOURCE_COMMIT.fullmatch(options.source_commit) is None:
        raise ValueError("source commit is invalid")
    return Assets(
        binary=validate_binary(options.binary),
        unit=validate_config(
            options.unit,
            Path("packaging/systemd/hardened-blikvm-diagnostic-collector.service"),
        ),
        sysusers=validate_config(
            options.sysusers,
            Path("packaging/sysusers.d/hardened-blikvm-diagnostics.conf"),
        ),
        source_commit=options.source_commit,
    )


def stage_paths(binary_sha256: str, run_id: str) -> Paths:
    if SHA256.fullmatch(binary_sha256) is None or RUN_ID.fullmatch(run_id) is None:
        raise ValueError("staging identity is invalid")
    root = f"/tmp/hardened-blikvm-diagnostic-collector-{binary_sha256[:12]}-{run_id}"
    if STAGE_ROOT.fullmatch(root) is None:
        raise ValueError("staging root is invalid")
    marker_value = f"diagnostic-collector:{binary_sha256}:{run_id}"
    unit_target = f"/run/systemd/system/{UNIT_NAME}"
    identity_units = tuple(
        f"hardened-blikvm-diagnostic-identity-{run_id}-{role}.service"
        for role in ("anchor", "server", "broker", "video", "control", "update")
    )
    return Paths(
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        marker_value=marker_value,
        binary=f"{root}/collector",
        unit=f"{root}/collector.service",
        sysusers=f"{root}/diagnostics.sysusers",
        client=f"{root}/client.py",
        rollback=f"{root}/rollback.sh",
        state=f"{root}/rollback.state",
        unit_target=unit_target,
        binary_parent=BINARY_PARENT,
        binary_target=BINARY_TARGET,
        binary_part=f"{BINARY_TARGET}.part",
        runtime_directory=RUNTIME_DIRECTORY,
        socket=SOCKET_PATH,
        client_state=f"{root}/client-state",
        watchdog=f"hardened-blikvm-diagnostic-rollback-{run_id}",
        identity_units=identity_units,
    )


def assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def encode_text(value: str) -> str:
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def policy_receipt() -> PolicyReceipt:
    return {
        "accountMode": "all-dynamic-test-static-template-unshipped",
        "deviceActionsExecuted": False,
        "networkAccess": "private-af-unix-only",
        "persistentChangeScope": "temporary-binary-and-rollback-required",
        "persistentStateRestored": True,
        "physicalPowerOrReset": False,
        "rawLogsRead": False,
        "rollbackWatchdog": "ten-minute-systemd",
    }


def build_client(paths: Paths) -> str:
    return f'''#!/usr/bin/python3
import os
import socket
import sys
import time

SOCKET = {paths.socket!r}
READY = bytes((1, 1, 0, 0, 0, 0, 0, 0))
ACCEPTED = bytes((1, 1, 0, 0))

def receive_exact(stream, size):
    output = bytearray()
    while len(output) < size:
        chunk = stream.recv(size - len(output))
        if not chunk:
            raise RuntimeError("early eof")
        output.extend(chunk)
    return bytes(output)

mode = sys.argv[1] if len(sys.argv) > 1 else ""
stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
stream.settimeout(8)
stream.connect(SOCKET)
if mode == "accepted":
    stream.sendall(READY)
    if receive_exact(stream, 4) != ACCEPTED:
        raise RuntimeError("response mismatch")
elif mode == "rejected":
    if stream.recv(1) != b"":
        raise RuntimeError("rejected peer received data")
elif mode == "malformed":
    stream.sendall(bytes(8))
    if stream.recv(1) != b"":
        raise RuntimeError("malformed frame received data")
elif mode == "partial":
    started = time.monotonic()
    stream.sendall(READY[:1])
    if stream.recv(1) != b"":
        raise RuntimeError("partial frame received data")
    elapsed = time.monotonic() - started
    if elapsed < 4.0 or elapsed > 8.0:
        raise RuntimeError("partial timeout out of bounds")
elif mode == "hold":
    if len(sys.argv) != 3 or not sys.argv[2].startswith({(paths.client_state + '/')!r}):
        raise RuntimeError("ready path invalid")
    stream.sendall(READY)
    if receive_exact(stream, 4) != ACCEPTED:
        raise RuntimeError("response mismatch")
    descriptor = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    if stream.recv(1) != b"":
        raise RuntimeError("hold received trailing data")
else:
    raise RuntimeError("mode invalid")
stream.close()
'''


def build_preflight_script(paths: Paths, required_bytes: int) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "collector_account": COLLECTOR_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "identity_accounts": " ".join(TRANSIENT_IDENTITY_ACCOUNTS),
            "identity_units": " ".join(paths.identity_units),
            "unit_name": UNIT_NAME,
        }
    )
    body = r'''
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) : ;; esac
available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
case "$available" in ''|*[!0-9]*) exit 1 ;; esac
[ "$available" -ge "$required_bytes" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
[ ! -e "$runtime_directory" ]
[ ! -e "$socket_path" ]
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
if systemctl cat "$unit_name" >/dev/null 2>&1; then exit 1; fi
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $identity_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
done
for account in $producer_accounts; do if getent group "$account" >/dev/null 2>&1; then exit 1; fi; done
for identity_unit in $identity_units; do
  if systemctl cat "$identity_unit" >/dev/null 2>&1; then exit 1; fi
  [ "$(systemctl is-active "$identity_unit" 2>/dev/null || true)" != active ]
done
if [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_before=present
else
  binary_parent_before=absent
fi
version=$(systemctl --version | head -n 1 | awk '{print $2}')
case "$version" in ''|*[!0-9]*) exit 1 ;; esac
[ "$version" -ge 249 ]
[ -x /bin/sleep ]
for tool in awk base64 cat chmod cp cut df find findmnt getent grep head id install kill mkdir mv python3 readlink rm rmdir sed setpriv sha256sum sleep stat systemctl systemd-analyze systemd-run systemd-sysusers tail tr uname wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit account_absent_before yes
emit architecture aarch64
emit binary_absent_before yes
emit binary_parent_before "$binary_parent_before"
emit identities_absent_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit rootfs_mode_before "$rootfs_mode"
emit runtime_absent_before yes
emit stage_absent_before yes
emit systemd_ready yes
emit tools_ready yes
emit unit_absent_before yes
emit watchdog_absent_before yes
'''.strip()
    return f"{values}\n{body}\n"


def build_first_upload_script(paths: Paths) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": paths.binary,
        }
    )
    body = r'''
set -eu
umask 077
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
[ ! -e "$stage_root" ]
mkdir -m 755 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod 755 "$destination"
'''.strip()
    return f"{values}\n{body}\n"


def build_upload_script(paths: Paths, destination: str, mode: str) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": destination,
            "mode": mode,
        }
    )
    body = r'''
set -eu
[ -d "$stage_root" ]
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ ! -e "$destination" ]
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod "$mode" "$destination"
'''.strip()
    return f"{values}\n{body}\n"


def build_rollback_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "state_file": paths.state,
            "unit_name": UNIT_NAME,
            "unit_target": paths.unit_target,
            "unit_sha": assets.unit.sha256,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "collector_account": COLLECTOR_ACCOUNT,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "identity_accounts": " ".join(TRANSIENT_IDENTITY_ACCOUNTS),
            "shared_group": SHARED_GROUP,
            "identity_units_stop": " ".join(reversed(paths.identity_units)),
            "watchdog": paths.watchdog,
        }
    )
    body = r'''
set -eu
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$state_file" ]
parent_before=$(awk -F= '$1 == "parent_before" {print $2}' "$state_file")
case "$parent_before" in present|absent) ;; *) exit 1 ;; esac
systemctl stop "$unit_name" 2>/dev/null || true
count=0
while [ "$count" -lt 100 ] && getent passwd "$collector_account" >/dev/null 2>&1; do
  sleep 0.1
  count=$((count + 1))
done
if [ -e "$runtime_directory" ]; then
  [ -d "$runtime_directory" ]
  [ ! -L "$runtime_directory" ]
  unexpected=$(find "$runtime_directory" -mindepth 1 -maxdepth 1 ! -name collector.sock -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$socket_path" ]; then
    [ -S "$socket_path" ]
    [ "$(stat -c '%a' "$socket_path")" = 660 ]
    rm -f -- "$socket_path"
  fi
  rmdir "$runtime_directory"
fi
for identity_unit in $identity_units_stop; do
  systemctl stop "$identity_unit" 2>/dev/null || true
  systemctl reset-failed "$identity_unit" 2>/dev/null || true
done
count=0
while [ "$count" -lt 100 ]; do
  identities_present=no
  if getent group "$shared_group" >/dev/null 2>&1; then identities_present=yes; fi
  for account in $identity_accounts; do
    if getent passwd "$account" >/dev/null 2>&1; then identities_present=yes; fi
  done
  [ "$identities_present" = yes ] || break
  sleep 0.1
  count=$((count + 1))
done
if [ -e "$unit_target" ]; then
  [ -f "$unit_target" ]
  [ ! -L "$unit_target" ]
  [ "$(sha256sum "$unit_target" | cut -d ' ' -f 1)" = "$unit_sha" ]
  rm -f -- "$unit_target"
fi
systemctl daemon-reload
systemctl reset-failed "$unit_name" 2>/dev/null || true
if [ -e "$binary_part" ]; then
  [ -f "$binary_part" ]
  [ ! -L "$binary_part" ]
  [ "$(stat -c '%a:%u:%g:%h' "$binary_part")" = 755:0:0:1 ]
  [ "$(stat -c %s "$binary_part")" -le "$binary_size" ]
  rm -f -- "$binary_part"
fi
if [ -e "$binary_target" ]; then
  [ -f "$binary_target" ]
  [ ! -L "$binary_target" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$binary_target")" = "755:0:0:1:$binary_size" ]
  [ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
  rm -f -- "$binary_target"
fi
if [ "$parent_before" = absent ] && [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  rmdir "$binary_parent"
fi
[ ! -e "$runtime_directory" ]
[ ! -e "$binary_target" ]
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $identity_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
done
for account in $producer_accounts; do
  if getent group "$account" >/dev/null 2>&1; then exit 1; fi
done
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${INVOCATION_ID:-}" ]; then systemctl stop "$watchdog.service" 2>/dev/null || true; fi
rm -rf -- "$stage_root"
'''.strip()
    return f"{values}\n{body}\n"


def build_run_script(paths: Paths, assets: Assets) -> str:
    client = build_client(paths)
    rollback = build_rollback_script(paths, assets)
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "binary": paths.binary,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "unit_source": paths.unit,
            "unit_sha": assets.unit.sha256,
            "sysusers_source": paths.sysusers,
            "sysusers_sha": assets.sysusers.sha256,
            "client": paths.client,
            "client_sha": hash_text(client),
            "client_b64": encode_text(client),
            "rollback_script": paths.rollback,
            "rollback_b64": encode_text(rollback),
            "rollback_state": paths.state,
            "unit_name": UNIT_NAME,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "client_state": paths.client_state,
            "watchdog": paths.watchdog,
            "collector_account": COLLECTOR_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "identity_pairs": " ".join(
                f"{account}:{unit}"
                for account, unit in zip(TRANSIENT_IDENTITY_ACCOUNTS, paths.identity_units)
            ),
        }
    )
    body = r'''
set -eu
export LC_ALL=C
umask 077
phase=validate-assets
run_succeeded=no
emit() { printf '%s=%s\n' "$1" "$2"; }
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then emit failure_phase "$phase"; fi
  exit "$exit_code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(stat -c %s "$binary")" = "$binary_size" ]
[ "$(sha256sum "$binary" | cut -d ' ' -f 1)" = "$binary_sha" ]
[ "$(sha256sum "$unit_source" | cut -d ' ' -f 1)" = "$unit_sha" ]
[ "$(sha256sum "$sysusers_source" | cut -d ' ' -f 1)" = "$sysusers_sha" ]
kvmd_pid_before=$(systemctl show -p MainPID --value kvmd.service)
nginx_pid_before=$(systemctl show -p MainPID --value kvmd-nginx.service)
for pid in "$kvmd_pid_before" "$nginx_pid_before"; do
  case "$pid" in ''|0|*[!0-9]*) exit 1 ;; esac
done
printf '%s' "$client_b64" | base64 -d > "$client"
chmod 755 "$client"
[ "$(sha256sum "$client" | cut -d ' ' -f 1)" = "$client_sha" ]
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
phase=verify-sysusers
synthetic_root="$stage_root/sysusers-root"
install -d -m 755 "$synthetic_root/etc" "$synthetic_root/usr/lib/sysusers.d"
printf 'g %s -\n' "$shared_group" > "$synthetic_root/usr/lib/sysusers.d/00-base.conf"
install -m 0644 "$sysusers_source" "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
awk -F: -v account="$collector_account" '$1 == account && $3 != 0 && $6 == "/nonexistent" && $7 == "/usr/sbin/nologin" {found=1} END {exit !found}' "$synthetic_root/etc/passwd"
awk -F: -v group="$shared_group" -v account="$collector_account" '$1 == group && ("," $4 ",") ~ ("," account ",") {found=1} END {exit !found}' "$synthetic_root/etc/group"
if [ -e "$binary_parent" ]; then parent_before=present; else parent_before=absent; fi
printf 'parent_before=%s\n' "$parent_before" > "$rollback_state"
chmod 600 "$rollback_state"
phase=arm-watchdog
systemd-run --quiet --unit="$watchdog" --on-active=10min --timer-property=AccuracySec=1s /bin/sh "$rollback_script"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
phase=create-transient-identities
for identity_pair in $identity_pairs; do
  account=${identity_pair%%:*}
  identity_unit=${identity_pair#*:}
  systemd-run --quiet --collect --unit="$identity_unit" \
    --property=DynamicUser=yes --property="User=$account" --property="Group=$shared_group" \
    --property=PrivateNetwork=yes --property=NoNewPrivileges=yes \
    --property=ProtectSystem=strict --property=ProtectHome=yes \
    --property=PrivateTmp=yes --property=RuntimeMaxSec=9min /bin/sleep 600
  [ "$(systemctl is-active "$identity_unit" 2>/dev/null || true)" = active ]
  getent passwd "$account" >/dev/null
done
shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
case "$shared_gid" in ''|0|*[!0-9]*) exit 1 ;; esac
seen=:
for identity_pair in $identity_pairs; do
  account=${identity_pair%%:*}
  identity_unit=${identity_pair#*:}
  uid=$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')
  pid=$(systemctl show -p MainPID --value "$identity_unit")
  for value in "$uid" "$pid"; do
    case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac
  done
  case "$seen" in *":$uid:"*) exit 1 ;; esac
  seen="$seen$uid:"
  [ "$(awk '/^Gid:/ {print $2}' "/proc/$pid/status")" = "$shared_gid" ]
done
phase=install-runtime
if [ "$parent_before" = absent ]; then mkdir -m 755 "$binary_parent"; fi
install -m 0755 "$binary" "$binary_part"
[ "$(sha256sum "$binary_part" | cut -d ' ' -f 1)" = "$binary_sha" ]
mv "$binary_part" "$binary_target"
[ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
install -m 0644 "$unit_source" "$unit_target"
[ "$(sha256sum "$unit_target" | cut -d ' ' -f 1)" = "$unit_sha" ]
systemctl daemon-reload
systemd-analyze verify "$unit_name" >/dev/null 2>&1
phase=start-collector
systemctl start "$unit_name"
count=0
while [ "$count" -lt 100 ] && [ ! -S "$socket_path" ]; do
  [ "$(systemctl is-active "$unit_name" 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S "$socket_path" ]
collector_pid=$(systemctl show -p MainPID --value "$unit_name")
case "$collector_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
[ "$(systemctl show -p User --value "$unit_name")" = "$collector_account" ]
[ "$(systemctl show -p Group --value "$unit_name")" = "$shared_group" ]
[ "$(systemctl show -p DynamicUser --value "$unit_name")" = yes ]
[ "$(systemctl show -p PrivateNetwork --value "$unit_name")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$unit_name")" = closed ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$collector_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$collector_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$collector_pid/status")" = 1 ]
[ "$(readlink "/proc/$collector_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
collector_uid=$(getent passwd "$collector_account" | awk -F: 'NR == 1 {print $3}')
shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
for value in "$collector_uid" "$shared_gid"; do
  case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac
done
[ "$(awk '/^Uid:/ {print $2}' "/proc/$collector_pid/status")" = "$collector_uid" ]
[ "$(stat -c '%a:%u:%g' "$runtime_directory")" = "750:$collector_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' "$socket_path")" = "660:$collector_uid:$shared_gid" ]
phase=accepted-sources
install -d -m 0770 -o root -g "$shared_group" "$client_state"
run_as() {
  account=$1
  shift
  uid=$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')
  gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
  setpriv --reuid="$uid" --regid="$gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" "$@"
}
accepted_count=0
for account in $producer_accounts; do
  run_as "$account" accepted
  accepted_count=$((accepted_count + 1))
done
[ "$accepted_count" = 5 ]
phase=rejected-peers
python3 "$client" rejected
unknown_uid=$(getent passwd nobody | awk -F: 'NR == 1 {print $3}')
case "$unknown_uid" in ''|0|*[!0-9]*) exit 1 ;; esac
for account in $producer_accounts; do
  [ "$unknown_uid" != "$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')" ]
done
[ "$unknown_uid" != "$collector_uid" ]
setpriv --reuid="$unknown_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" rejected
phase=duplicate-peer
server_uid=$(getent passwd hardened-blikvm-server | awk -F: 'NR == 1 {print $3}')
duplicate_ready="$client_state/duplicate.ready"
setpriv --reuid="$server_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" hold "$duplicate_ready" &
holder_pid=$!
count=0
while [ "$count" -lt 80 ] && [ ! -f "$duplicate_ready" ]; do
  kill -0 "$holder_pid" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -f "$duplicate_ready" ]
run_as hardened-blikvm-server rejected
kill "$holder_pid"
wait "$holder_pid" 2>/dev/null || true
rm -f -- "$duplicate_ready"
sleep 0.2
phase=partial-frame
run_as hardened-blikvm-server partial
sleep 0.2
phase=malformed-frame
run_as hardened-blikvm-server malformed
[ "$(systemctl is-active "$unit_name" 2>/dev/null || true)" = active ]
phase=graceful-shutdown
update_uid=$(getent passwd hardened-blikvm-update | awk -F: 'NR == 1 {print $3}')
shutdown_ready="$client_state/shutdown.ready"
setpriv --reuid="$update_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" hold "$shutdown_ready" &
shutdown_client_pid=$!
count=0
while [ "$count" -lt 80 ] && [ ! -f "$shutdown_ready" ]; do
  kill -0 "$shutdown_client_pid" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -f "$shutdown_ready" ]
systemctl stop "$unit_name"
wait "$shutdown_client_pid"
count=0
while [ "$count" -lt 100 ] && getent passwd "$collector_account" >/dev/null 2>&1; do
  sleep 0.1
  count=$((count + 1))
done
[ ! -e "$runtime_directory" ]
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
[ "$(systemctl show -p MainPID --value kvmd.service)" = "$kvmd_pid_before" ]
[ "$(systemctl show -p MainPID --value kvmd-nginx.service)" = "$nginx_pid_before" ]
phase=rollback
/bin/sh "$rollback_script"
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$runtime_directory" ]
run_succeeded=yes
emit binary_hash_verified yes
emit collector_caps none
emit collector_dynamic_user yes
emit collector_network_namespace private
emit collector_no_new_privs yes
emit collector_unit active-then-stopped
emit duplicate_rejected yes
emit five_sources_accepted 5
emit graceful_shutdown yes
emit kvmd_active_after_run active
emit kvmd_nginx_active_after_run active
emit malformed_rejected yes
emit partial_timeout bounded
emit root_rejected yes
emit rollback_confirmed yes
emit runtime_removed yes
emit socket_mode 660
emit socket_owner dynamic-shared
emit sysusers_synthetic yes
emit transient_identities created-and-removed
emit unit_hash_verified yes
emit unknown_rejected yes
emit watchdog_armed yes
'''.strip()
    return f"{values}\n{body}\n"


def build_cleanup_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "rollback_b64": encode_text(build_rollback_script(paths, assets)),
        }
    )
    body = r'''
set -eu
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
if [ -e "$stage_root" ]; then
  [ -d "$stage_root" ]
  [ ! -L "$stage_root" ]
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  if [ ! -x "$rollback_script" ]; then
    printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
    chmod 700 "$rollback_script"
  fi
  if [ ! -f "$rollback_state" ]; then
    [ ! -e "$binary_target" ]
    if [ -d "$binary_parent" ] && [ ! -L "$binary_parent" ]; then
      parent_before=present
    else
      [ ! -e "$binary_parent" ]
      parent_before=absent
    fi
    printf 'parent_before=%s\n' "$parent_before" > "$rollback_state"
    chmod 600 "$rollback_state"
  fi
  /bin/sh "$rollback_script"
fi
[ ! -e "$stage_root" ]
printf 'cleanup_restored=yes\n'
'''.strip()
    return f"{values}\n{body}\n"


def build_final_script(paths: Paths, binary_parent_before: str) -> str:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    values = assignments(
        {
            "stage_root": paths.root,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "binary_parent_before": binary_parent_before,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "watchdog": paths.watchdog,
            "collector_account": COLLECTOR_ACCOUNT,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "identity_accounts": " ".join(TRANSIENT_IDENTITY_ACCOUNTS),
            "shared_group": SHARED_GROUP,
            "identity_units": " ".join(paths.identity_units),
        }
    )
    body = r'''
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
if [ "$binary_parent_before" = present ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_final=present
else
  [ ! -e "$binary_parent" ]
  binary_parent_final=absent
fi
[ ! -e "$runtime_directory" ]
[ ! -e "$socket_path" ]
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $identity_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
done
for account in $producer_accounts; do
  if getent group "$account" >/dev/null 2>&1; then exit 1; fi
done
for identity_unit in $identity_units; do
  [ "$(systemctl is-active "$identity_unit" 2>/dev/null || true)" != active ]
done
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit account_absent_final yes
emit binary_absent_final yes
emit binary_parent_final "$binary_parent_final"
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit rootfs_mode_final "$rootfs_mode"
emit runtime_absent_final yes
emit stage_absent_final yes
emit unit_absent_final yes
emit watchdog_inactive_final yes
'''.strip()
    return f"{values}\n{body}\n"


def require_result(stage: str, result: subprocess.CompletedProcess[bytes]) -> bytes:
    if result.returncode != 0 or len(result.stdout) > 16 * 1024:
        raise RuntimeError(stage)
    return result.stdout


def parse_runtime_failure(raw: bytes) -> str:
    facts = native.parse_facts(raw, FAILURE_KEYS)
    phase = facts["failure_phase"]
    if SAFE_PHASE.fullmatch(phase) is None:
        raise ValueError("runtime failure phase is invalid")
    return phase


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "account_absent_before": "yes",
        "architecture": "aarch64",
        "binary_absent_before": "yes",
        "binary_parent_before": facts.get("binary_parent_before", ""),
        "identities_absent_before": "yes",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "rootfs_mode_before": "rw",
        "runtime_absent_before": "yes",
        "stage_absent_before": "yes",
        "systemd_ready": "yes",
        "tools_ready": "yes",
        "unit_absent_before": "yes",
        "watchdog_absent_before": "yes",
    }
    if expected["binary_parent_before"] not in {"present", "absent"}:
        raise ValueError("binary parent state is invalid")
    if facts != dict(sorted(expected.items())):
        raise ValueError("preflight invariant failed")


def validate_run(facts: dict[str, str]) -> None:
    expected = {
        "binary_hash_verified": "yes",
        "collector_caps": "none",
        "collector_dynamic_user": "yes",
        "collector_network_namespace": "private",
        "collector_no_new_privs": "yes",
        "collector_unit": "active-then-stopped",
        "duplicate_rejected": "yes",
        "five_sources_accepted": "5",
        "graceful_shutdown": "yes",
        "kvmd_active_after_run": "active",
        "kvmd_nginx_active_after_run": "active",
        "malformed_rejected": "yes",
        "partial_timeout": "bounded",
        "root_rejected": "yes",
        "rollback_confirmed": "yes",
        "runtime_removed": "yes",
        "socket_mode": "660",
        "socket_owner": "dynamic-shared",
        "sysusers_synthetic": "yes",
        "transient_identities": "created-and-removed",
        "unit_hash_verified": "yes",
        "unknown_rejected": "yes",
        "watchdog_armed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def validate_final(facts: dict[str, str], binary_parent_before: str) -> None:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    expected = {
        "account_absent_final": "yes",
        "binary_absent_final": "yes",
        "binary_parent_final": binary_parent_before,
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
        "rootfs_mode_final": "rw",
        "runtime_absent_final": "yes",
        "stage_absent_final": "yes",
        "unit_absent_final": "yes",
        "watchdog_inactive_final": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def upload(
    target: prototype.RemoteTarget,
    secret: bytearray,
    source: Asset,
    script: str,
) -> None:
    with source.path.open("rb") as stream:
        _ = require_result(
            "upload",
            prototype.run_ssh(target, secret, script, stdin=stream, timeout=60),
        )


def attempt_cleanup(
    target: prototype.RemoteTarget,
    secret: bytearray,
    paths: Paths,
    assets: Assets,
) -> CleanupReceipt:
    try:
        output = require_result(
            "cleanup",
            prototype.run_ssh(
                target,
                secret,
                build_cleanup_script(paths, assets),
                timeout=45,
            ),
        )
        facts = native.parse_facts(output, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_restored": "yes"}}
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    target = native.target_from_known_hosts(options.known_hosts, options.endpoint)
    paths = stage_paths(assets.binary.sha256, secrets.token_hex(8))
    required_bytes = assets.binary.size_bytes * 4 + MIN_TMP_HEADROOM_BYTES
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    cleanup_required = False
    try:
        preflight = native.parse_facts(
            require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_preflight_script(paths, required_bytes),
                    timeout=25,
                ),
            ),
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        upload(target, secret, assets.binary, build_first_upload_script(paths))
        upload(target, secret, assets.unit, build_upload_script(paths, paths.unit, "600"))
        upload(
            target,
            secret,
            assets.sysusers,
            build_upload_script(paths, paths.sysusers, "600"),
        )

        stage = "runtime-and-rollback"
        runtime_result = prototype.run_ssh(
            target,
            secret,
            build_run_script(paths, assets),
            timeout=180,
        )
        if runtime_result.returncode != 0:
            try:
                stage = f"runtime-{parse_runtime_failure(runtime_result.stdout)}"
            except (UnicodeError, ValueError):
                stage = "runtime-unknown"
            raise RuntimeError(stage)
        checks = native.parse_facts(
            require_result(stage, runtime_result),
            RUN_KEYS,
        )
        validate_run(checks)

        stage = "final-state"
        final_state = native.parse_facts(
            require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_final_script(paths, preflight["binary_parent_before"]),
                    timeout=25,
                ),
            ),
            FINAL_KEYS,
        )
        validate_final(final_state, preflight["binary_parent_before"])
        cleanup_required = False

        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-collector",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "artifacts": {
                "binary": {
                    "sha256": assets.binary.sha256,
                    "sizeBytes": assets.binary.size_bytes,
                },
                "unitSha256": assets.unit.sha256,
                "sysusersSha256": assets.sysusers.sha256,
            },
            "policy": policy_receipt(),
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            attempt_cleanup(target, secret, paths, assets)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-collector",
            "result": "failed",
            "sourceCommit": assets.source_commit,
            "failure": {"stage": stage, "failureType": type(error).__name__},
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        native.zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
