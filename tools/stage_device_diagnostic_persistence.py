#!/usr/bin/env python3
"""Run the synthetic diagnostic persistence gate reversibly on test H616."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import re
import secrets
import stat
import struct
import subprocess
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import stage_device_system_action_maintenance as common
from tools import stage_device_system_action_store as native


PRODUCTION_ROOT = "/var/lib/hardened-blikvm-diagnostics"
RUNTIME_ROOT = "/run/hardened-blikvm-diagnostics"
RESULT_FILE = f"{RUNTIME_ROOT}/gate.result"
EXPORT_SOCKET = f"{RUNTIME_ROOT}/export.sock"
CREDENTIAL_PARENT = "/etc/hardened-blikvm"
CREDENTIAL_TARGET = f"{CREDENTIAL_PARENT}/diagnostic-journal-keyring"
BINARY_PARENT = "/usr/libexec/hardened-blikvm"
BINARY_TARGET = f"{BINARY_PARENT}/bli-kvm-diagnostic-persistence-gate"
UNIT_NAME = "hardened-blikvm-diagnostic-store-init.service"
UNIT_TARGET = f"/run/systemd/system/{UNIT_NAME}"
BROKER_ACCOUNT = "hardened-blikvm-broker"
SHARED_GROUP = "hardened-blikvm"
ACCOUNT_DATABASE_FILES = ("passwd", "group", "shadow", "gshadow")
COPY_BYTES = 16_552
CREDENTIAL_BYTES = 96
MAX_BINARY_BYTES = 16 * 1024 * 1024
MIN_TMP_HEADROOM_BYTES = 16 * 1024 * 1024
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-diagnostic-persistence-[0-9a-f]{12}-[0-9a-f]{16}$"
)
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-diagnostic-persistence-gate-[0-9a-f]{64}$"
)
EXPECTED_GATE_RESULT = (
    "schema=1\n"
    "result=passed\n"
    "final_generation=7\n"
    "entry_count=2\n"
    "acknowledged_entries=2\n"
    "read_requests=7\n"
    "acknowledgement_requests=3\n"
    "cancelled_acknowledgements=1\n"
    "automatic_retries=0\n"
)
EXPECTED_GATE_RESULT_SHA256 = hashlib.sha256(
    EXPECTED_GATE_RESULT.encode("ascii")
).hexdigest()
EXPECTED_GATE_RESULT_BYTES = len(EXPECTED_GATE_RESULT.encode("ascii"))

PREFLIGHT_KEYS = {
    "architecture",
    "binary_absent_before",
    "binary_parent_before",
    "credential_absent_before",
    "credential_parent_before",
    "identities_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "production_root_absent_before",
    "rootfs_mode_before",
    "runtime_root_absent_before",
    "stage_absent_before",
    "tools_ready",
    "unit_absent_before",
    "watchdog_absent_before",
}
RUN_KEYS = {
    "account_databases_restored",
    "acknowledged_entries",
    "acknowledgement_requests",
    "automatic_retries",
    "binary_hash_verified",
    "cancelled_acknowledgements",
    "copies_equal",
    "copies_mode_verified",
    "credential_runtime_accepted",
    "entry_count",
    "final_generation",
    "read_requests",
    "rollback_confirmed",
    "sandbox_verified",
    "watchdog_armed",
}
FINAL_KEYS = {
    "binary_absent_final",
    "binary_parent_final",
    "credential_absent_final",
    "credential_parent_final",
    "identities_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "production_root_absent_final",
    "rootfs_mode_final",
    "runtime_root_absent_final",
    "stage_absent_final",
    "unit_absent_final",
    "watchdog_inactive_final",
}
CLEANUP_KEYS = {"cleanup_restored"}


@dataclass(frozen=True)
class Options:
    binary: str
    source_commit: str
    unit: str
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
    source_commit: str


@dataclass(frozen=True)
class Paths:
    root: str
    marker: str
    marker_value: str
    binary: str
    unit: str
    rollback: str
    state: str
    account_backup: str
    synthetic_root: str
    sysusers: str
    watchdog: str


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool
    failurePhase: str | None


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument(
        "--unit",
        default=(
            "packaging/systemd/"
            "hardened-blikvm-diagnostic-persistence-gate.service"
        ),
    )
    parser.add_argument("--endpoint", default="")
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument(
        "--output",
        default="evidence/private/device-diagnostic-persistence.json",
    )
    namespace = parser.parse_args()
    return Options(
        binary=cast(str, namespace.binary),
        source_commit=cast(str, namespace.source_commit),
        unit=cast(str, namespace.unit),
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
        types: list[int] = []
        for _ in range(program_count):
            entry = handle.read(program_entry_size)
            if len(entry) != program_entry_size:
                raise ValueError("binary program headers are truncated")
            types.append(struct.unpack_from("<I", entry)[0])
        if 3 in types or 1 not in types:
            raise ValueError("binary is not static PIE")
    sha256 = hash_file(path)
    if path.name != f"hardened-blikvm-diagnostic-persistence-gate-{sha256}":
        raise ValueError("binary snapshot name does not match SHA-256")
    return Asset(path, sha256, metadata.st_size)


def validate_assets(options: Options) -> Assets:
    if SOURCE_COMMIT.fullmatch(options.source_commit) is None:
        raise ValueError("source commit is invalid")
    unit = common.validate_config(
        options.unit,
        Path(
            "packaging/systemd/"
            "hardened-blikvm-diagnostic-persistence-gate.service"
        ),
    )
    return Assets(
        binary=validate_binary(options.binary),
        unit=Asset(unit.path, unit.sha256, unit.size_bytes),
        source_commit=options.source_commit,
    )


def stage_paths(binary_sha256: str, run_id: str) -> Paths:
    if SHA256.fullmatch(binary_sha256) is None or RUN_ID.fullmatch(run_id) is None:
        raise ValueError("staging identity is invalid")
    root = (
        f"/tmp/hardened-blikvm-diagnostic-persistence-"
        f"{binary_sha256[:12]}-{run_id}"
    )
    if STAGE_ROOT.fullmatch(root) is None:
        raise ValueError("staging root is invalid")
    marker_value = f"diagnostic-persistence:{binary_sha256}:{run_id}"
    return Paths(
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        marker_value=marker_value,
        binary=f"{root}/gate",
        unit=f"{root}/gate.service",
        rollback=f"{root}/rollback.sh",
        state=f"{root}/rollback.state",
        account_backup=f"{root}/account-backup",
        synthetic_root=f"{root}/synthetic-root",
        sysusers=f"{root}/diagnostic-sysusers.conf",
        watchdog=f"hardened-blikvm-diagnostic-persistence-rollback-{run_id}",
    )


def build_preflight_script(paths: Paths, required_bytes: int) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "production_root": PRODUCTION_ROOT,
            "runtime_root": RUNTIME_ROOT,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "unit_target": UNIT_TARGET,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) : ;; esac
available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
case "$available" in ''|*[!0-9]*) exit 1 ;; esac
[ "$available" -ge "$required_bytes" ]
[ "$(uname -m)" = aarch64 ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ ! -e "$stage_root" ]
[ ! -e "$production_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_target.part" ]
[ ! -e "$unit_target" ]
[ ! -e "$unit_target.part" ]
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
if getent passwd "$broker_account" >/dev/null 2>&1 || \
   getent group "$broker_account" >/dev/null 2>&1 || \
   getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
if [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_before=present
else binary_parent_before=absent; fi
if [ -e "$credential_parent" ]; then
  [ -d "$credential_parent" ]
  [ ! -L "$credential_parent" ]
  case "$(stat -c '%a:%u:%g' "$credential_parent")" in 700:0:0|750:0:0|755:0:0) ;; *) exit 1 ;; esac
  credential_parent_before=present
else credential_parent_before=absent; fi
for database in /etc/passwd /etc/group /etc/shadow /etc/gshadow; do
  [ -f "$database" ]
  [ ! -L "$database" ]
done
for tool in awk base64 cat chmod chown cmp cp cut dd df find findmnt getent grep install mkdir mv od readlink rm rmdir sha256sum stat sync systemctl systemd-analyze systemd-run systemd-sysusers tail tr uname wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit architecture aarch64
emit binary_absent_before yes
emit binary_parent_before "$binary_parent_before"
emit credential_absent_before yes
emit credential_parent_before "$credential_parent_before"
emit identities_absent_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit production_root_absent_before yes
emit rootfs_mode_before "$rootfs_mode"
emit runtime_root_absent_before yes
emit stage_absent_before yes
emit tools_ready yes
emit unit_absent_before yes
emit watchdog_absent_before yes
""".strip()
    return f"{values}\n{body}"


def build_first_upload_script(paths: Paths) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": paths.binary,
        }
    )
    body = r"""
set -eu
umask 077
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-persistence-*) ;; *) exit 1 ;; esac
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod 700 "$destination"
""".strip()
    return f"{values}\n{body}"


def build_upload_script(paths: Paths, destination: str) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": destination,
        }
    )
    body = r"""
set -eu
[ -d "$stage_root" ]
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ ! -e "$destination" ]
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod 600 "$destination"
""".strip()
    return f"{values}\n{body}"


def build_rollback_script(paths: Paths, assets: Assets) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "state_file": paths.state,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "runtime_root": RUNTIME_ROOT,
            "result_file": RESULT_FILE,
            "export_socket": EXPORT_SOCKET,
            "result_sha": EXPECTED_GATE_RESULT_SHA256,
            "result_size": EXPECTED_GATE_RESULT_BYTES,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "binary_size": assets.binary.size_bytes,
            "binary_sha": assets.binary.sha256,
            "unit_target": UNIT_TARGET,
            "unit_size": assets.unit.size_bytes,
            "unit_sha": assets.unit.sha256,
            "unit_name": UNIT_NAME,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "account_files": " ".join(
                f"/etc/{name}" for name in ACCOUNT_DATABASE_FILES
            ),
        }
    )
    body = rf"""
set -eu
phase=validate
trap 'status=$?; trap - EXIT; if [ "$status" -ne 0 ]; then printf "rollback_failure_phase=%s\n" "$phase"; fi; exit "$status"' EXIT
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-persistence-*) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$state_file" ]
value() {{ awk -F= -v key="$1" '$1 == key {{print $2}}' "$state_file"; }}
parent_before=$(value parent_before)
credential_parent_before=$(value credential_parent_before)
case "$parent_before:$credential_parent_before" in present:present|present:absent|absent:present|absent:absent) ;; *) exit 1 ;; esac
broker_uid=$(value broker_uid)
shared_gid=$(value shared_gid)
case "$broker_uid:$shared_gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
phase=stop-unit
systemctl stop "$unit_name" 2>/dev/null || true
phase=remove-runtime
if [ -e "$runtime_root" ]; then
  [ -d "$runtime_root" ]
  [ ! -L "$runtime_root" ]
  [ "$(stat -c '%a:%u:%g' "$runtime_root")" = "700:$broker_uid:$shared_gid" ]
  unexpected=$(find "$runtime_root" -mindepth 1 -maxdepth 1 ! -name gate.result ! -name export.sock -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$result_file" ]; then
    [ -f "$result_file" ]
    [ ! -L "$result_file" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "$result_file")" = "600:$broker_uid:$shared_gid:1:$result_size" ]
    [ "$(sha256sum "$result_file" | cut -d ' ' -f 1)" = "$result_sha" ]
    rm -f -- "$result_file"
  fi
  if [ -e "$export_socket" ]; then
    [ -S "$export_socket" ]
    [ ! -L "$export_socket" ]
    [ "$(stat -c '%u:%g' "$export_socket")" = "$broker_uid:$shared_gid" ]
    rm -f -- "$export_socket"
  fi
  rmdir "$runtime_root"
fi
phase=remove-store
if [ -e "$production_root" ]; then
  [ -d "$production_root" ]
  [ ! -L "$production_root" ]
  [ "$(stat -c '%a:%u:%g' "$production_root")" = "700:$broker_uid:$shared_gid" ]
  unexpected=$(find "$production_root" -mindepth 1 -maxdepth 1 ! -name journal-a.bin ! -name journal-b.bin ! -name .journal-a.tmp ! -name .journal-b.tmp -print -quit)
  [ -z "$unexpected" ]
  for name in journal-a.bin journal-b.bin .journal-a.tmp .journal-b.tmp; do
    path="$production_root/$name"
    if [ -e "$path" ]; then
      [ -f "$path" ]
      [ ! -L "$path" ]
      [ "$(stat -c '%a:%u:%g:%h' "$path")" = "600:$broker_uid:$shared_gid:1" ]
      size=$(stat -c %s "$path")
      case "$name" in journal-*) [ "$size" = {COPY_BYTES} ] ;; *) [ "$size" -le {COPY_BYTES} ] ;; esac
      rm -f -- "$path"
    fi
  done
  rmdir "$production_root"
fi
phase=remove-credential
for path in "$credential_target.part" "$credential_target"; do
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    case "$(stat -c '%a:%u:%g:%h' "$path")" in 400:0:0:1|600:0:0:1) ;; *) exit 1 ;; esac
    [ "$(stat -c %s "$path")" -le {CREDENTIAL_BYTES} ]
    rm -f -- "$path"
  fi
done
phase=remove-unit
if [ -e "$unit_target" ]; then
  [ -f "$unit_target" ]
  [ ! -L "$unit_target" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$unit_target")" = "644:0:0:1:$unit_size" ]
  [ "$(sha256sum "$unit_target" | cut -d ' ' -f 1)" = "$unit_sha" ]
  rm -f -- "$unit_target"
fi
if [ -e "$unit_target.part" ]; then
  [ -f "$unit_target.part" ]
  [ ! -L "$unit_target.part" ]
  [ "$(stat -c %s "$unit_target.part")" -le "$unit_size" ]
  rm -f -- "$unit_target.part"
fi
systemctl daemon-reload
systemctl reset-failed "$unit_name" 2>/dev/null || true
phase=remove-binary
if [ -e "$binary_target" ]; then
  [ -f "$binary_target" ]
  [ ! -L "$binary_target" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$binary_target")" = "755:0:0:1:$binary_size" ]
  [ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
  rm -f -- "$binary_target"
fi
if [ -e "$binary_target.part" ]; then
  [ -f "$binary_target.part" ]
  [ ! -L "$binary_target.part" ]
  [ "$(stat -c %s "$binary_target.part")" -le "$binary_size" ]
  rm -f -- "$binary_target.part"
fi
phase=restore-accounts
for database in $account_files; do
  name=${{database##*/}}
  before=$(value "before_$name")
  modified=$(value "modified_$name")
  case "$before:$modified" in *[!0-9a-f:]*|:|*:|*:) exit 1 ;; esac
  [ "${{#before}}" = 64 ]
  [ "${{#modified}}" = 64 ]
  current=$(sha256sum "$database" | cut -d ' ' -f 1)
  if [ "$current" = "$modified" ]; then
    restore_tmp="/etc/.$name.hardened-blikvm-diagnostic-persistence-{paths.root.rsplit('-', 1)[-1]}"
    [ ! -e "$restore_tmp" ]
    cp -a -- "$account_backup/$name" "$restore_tmp"
    sync "$restore_tmp"
    mv -f -- "$restore_tmp" "$database"
    sync "$database"
  elif [ "$current" != "$before" ]; then exit 3; fi
  [ "$(sha256sum "$database" | cut -d ' ' -f 1)" = "$before" ]
  cmp -s "$account_backup/$name" "$database"
done
if getent passwd "$broker_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$broker_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
phase=remove-parents
if [ "$credential_parent_before" = absent ] && [ -e "$credential_parent" ]; then rmdir "$credential_parent"; fi
if [ "$parent_before" = absent ] && [ -e "$binary_parent" ]; then rmdir "$binary_parent"; fi
phase=stop-watchdog
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${{INVOCATION_ID:-}}" ]; then systemctl stop "$watchdog.service" 2>/dev/null || true; fi
phase=remove-stage
rm -f -- "$state_file" "$stage_root/rollback.sh" "$sysusers_file" \
  "$stage_root/gate" "$stage_root/gate.service" "$marker"
if [ -d "$account_backup" ]; then
  for name in {' '.join(ACCOUNT_DATABASE_FILES)}; do rm -f -- "$account_backup/$name"; done
  rmdir "$account_backup"
fi
if [ -d "$synthetic_root" ]; then
  rm -f -- "$synthetic_root/etc/passwd" "$synthetic_root/etc/group" \
    "$synthetic_root/etc/shadow" "$synthetic_root/etc/gshadow" \
    "$synthetic_root/etc/passwd-" "$synthetic_root/etc/group-" \
    "$synthetic_root/etc/shadow-" "$synthetic_root/etc/gshadow-" \
    "$synthetic_root/etc/.pwd.lock" \
    "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
  rmdir "$synthetic_root/usr/lib/sysusers.d" "$synthetic_root/usr/lib" \
    "$synthetic_root/usr" "$synthetic_root/etc" "$synthetic_root"
fi
rmdir "$stage_root"
trap - EXIT
""".strip()
    return f"{values}\n{body}"


def build_run_script(paths: Paths, assets: Assets) -> str:
    rollback_b64 = common.encode_script(build_rollback_script(paths, assets))
    expected_result_b64 = common.encode_script(EXPECTED_GATE_RESULT)
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "binary": paths.binary,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "unit_source": paths.unit,
            "unit_sha": assets.unit.sha256,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "rollback_b64": rollback_b64,
            "expected_result_b64": expected_result_b64,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "runtime_root": RUNTIME_ROOT,
            "result_file": RESULT_FILE,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "credential_part": f"{CREDENTIAL_TARGET}.part",
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "unit_target": UNIT_TARGET,
            "unit_name": UNIT_NAME,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "account_files": " ".join(
                f"/etc/{name}" for name in ACCOUNT_DATABASE_FILES
            ),
        }
    )
    body = rf"""
set -eu
export LC_ALL=C
phase=validate-assets
trap 'status=$?; trap - EXIT; if [ "$status" -ne 0 ]; then printf "run_failure_phase=%s\n" "$phase"; fi; exit "$status"' EXIT
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(stat -c %s "$binary")" = "$binary_size" ]
[ "$(sha256sum "$binary" | cut -d ' ' -f 1)" = "$binary_sha" ]
[ "$(sha256sum "$unit_source" | cut -d ' ' -f 1)" = "$unit_sha" ]
phase=prepare-rollback-state
if [ -e "$binary_parent" ]; then parent_before=present; else parent_before=absent; fi
if [ -e "$credential_parent" ]; then credential_parent_before=present; else credential_parent_before=absent; fi
state_part="$rollback_state.part"
[ ! -e "$state_part" ]
printf 'parent_before=%s\ncredential_parent_before=%s\n' "$parent_before" "$credential_parent_before" > "$state_part"
printf '%s\n' \
  'g hardened-blikvm - -' \
  'u hardened-blikvm-broker - "Hardened BliKVM platform broker" /nonexistent /usr/sbin/nologin' \
  'm hardened-blikvm-broker hardened-blikvm' > "$sysusers_file"
chmod 600 "$sysusers_file"
mkdir -m 700 "$account_backup"
install -d -m 700 "$synthetic_root/etc"
install -d -m 755 "$synthetic_root/usr/lib/sysusers.d"
for database in $account_files; do
  name=${{database##*/}}
  cp -a -- "$database" "$account_backup/$name"
  cp -a -- "$database" "$synthetic_root/etc/$name"
  printf 'before_%s=%s\n' "$name" "$(sha256sum "$database" | cut -d ' ' -f 1)" >> "$state_part"
done
install -m 0644 "$sysusers_file" "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
broker_uid=$(awk -F: -v account="$broker_account" '$1 == account {{print $3}}' "$synthetic_root/etc/passwd")
shared_gid=$(awk -F: -v group="$shared_group" '$1 == group {{print $3}}' "$synthetic_root/etc/group")
case "$broker_uid:$shared_gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
awk -F: -v group="$shared_group" -v account="$broker_account" '$1 == group && ("," $4 ",") ~ ("," account ",") {{ok=1}} END {{exit !ok}}' "$synthetic_root/etc/group"
printf 'g hardened-blikvm %s -\n' "$shared_gid" > "$sysusers_file"
printf 'u hardened-blikvm-broker %s "Hardened BliKVM platform broker" /nonexistent /usr/sbin/nologin\n' "$broker_uid" >> "$sysusers_file"
printf 'm hardened-blikvm-broker hardened-blikvm\n' >> "$sysusers_file"
chmod 600 "$sysusers_file"
for database in $account_files; do name=${{database##*/}}; cp -a -- "$account_backup/$name" "$synthetic_root/etc/$name"; done
install -m 0644 "$sysusers_file" "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
[ "$(awk -F: -v account="$broker_account" '$1 == account {{print $3}}' "$synthetic_root/etc/passwd")" = "$broker_uid" ]
[ "$(awk -F: -v group="$shared_group" '$1 == group {{print $3}}' "$synthetic_root/etc/group")" = "$shared_gid" ]
for database in $account_files; do
  name=${{database##*/}}
  printf 'modified_%s=%s\n' "$name" "$(sha256sum "$synthetic_root/etc/$name" | cut -d ' ' -f 1)" >> "$state_part"
done
printf 'broker_uid=%s\nshared_gid=%s\n' "$broker_uid" "$shared_gid" >> "$state_part"
chmod 600 "$state_part"
sync "$state_part"
mv "$state_part" "$rollback_state"
sync "$rollback_state"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
sync "$rollback_state" "$rollback_script"
phase=arm-watchdog
systemd-run --quiet --unit="$watchdog" --on-active=10min --timer-property=AccuracySec=1s /bin/sh "$rollback_script"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
phase=create-identities
systemd-sysusers "$sysusers_file" >/dev/null 2>&1
for database in $account_files; do
  name=${{database##*/}}
  expected=$(awk -F= -v key="modified_$name" '$1 == key {{print $2}}' "$rollback_state")
  [ "$(sha256sum "$database" | cut -d ' ' -f 1)" = "$expected" ]
done
getent group "$shared_group" | awk -F: -v account="$broker_account" '("," $4 ",") ~ ("," account ",") {{ok=1}} END {{exit !ok}}'
phase=install-runtime
if [ "$parent_before" = absent ]; then mkdir -m 755 "$binary_parent"; fi
install -m 0755 "$binary" "$binary_target.part"
mv "$binary_target.part" "$binary_target"
[ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
install -m 0644 "$unit_source" "$unit_target.part"
[ "$(sha256sum "$unit_target.part" | cut -d ' ' -f 1)" = "$unit_sha" ]
mv "$unit_target.part" "$unit_target"
phase=create-credential
if [ "$credential_parent_before" = absent ]; then mkdir -m 700 "$credential_parent"; fi
umask 077
printf 'BLIDKEY\000\001\001\000\000\001\000\000\000' > "$credential_part"
dd if=/dev/urandom bs=32 count=1 status=none >> "$credential_part"
dd if=/dev/zero bs=48 count=1 status=none >> "$credential_part"
[ "$(wc -c < "$credential_part" | tr -d ' ')" = {CREDENTIAL_BYTES} ]
od -An -v -j16 -N32 -tx1 "$credential_part" | tr -d ' \n' | grep -q '[1-9a-f]'
chmod 400 "$credential_part"
chown 0:0 "$credential_part"
sync "$credential_part"
mv "$credential_part" "$credential_target"
sync "$credential_parent"
phase=create-root
install -d -m 0700 -o "$broker_uid" -g "$shared_gid" "$production_root"
phase=run-gate
systemctl daemon-reload
systemd-analyze verify "$unit_name" >/dev/null 2>&1
systemctl start "$unit_name"
[ "$(systemctl show -p Result --value "$unit_name")" = success ]
[ "$(systemctl show -p ExecMainStatus --value "$unit_name")" = 0 ]
[ "$(systemctl show -p User --value "$unit_name")" = "$broker_account" ]
[ "$(systemctl show -p Group --value "$unit_name")" = "$shared_group" ]
[ "$(systemctl show -p PrivateNetwork --value "$unit_name")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$unit_name")" = closed ]
[ -f "$result_file" ]
[ "$(stat -c '%a:%u:%g:%h:%s' "$result_file")" = "600:$broker_uid:$shared_gid:1:{EXPECTED_GATE_RESULT_BYTES}" ]
printf '%s' "$expected_result_b64" | base64 -d | cmp -s - "$result_file"
for copy in journal-a.bin journal-b.bin; do
  [ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/$copy")" = "600:$broker_uid:$shared_gid:1:{COPY_BYTES}" ]
done
[ "$(sha256sum "$production_root/journal-a.bin" | cut -d ' ' -f 1)" = "$(sha256sum "$production_root/journal-b.bin" | cut -d ' ' -f 1)" ]
phase=rollback
/bin/sh "$rollback_script"
[ ! -e "$production_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$unit_target" ]
phase=emit-results
emit account_databases_restored yes
emit acknowledged_entries 2
emit acknowledgement_requests 3
emit automatic_retries 0
emit binary_hash_verified yes
emit cancelled_acknowledgements 1
emit copies_equal yes
emit copies_mode_verified yes
emit credential_runtime_accepted yes
emit entry_count 2
emit final_generation 7
emit read_requests 7
emit rollback_confirmed yes
emit sandbox_verified yes
emit watchdog_armed yes
trap - EXIT
""".strip()
    return f"{values}\n{body}"


def build_cleanup_script(paths: Paths, assets: Assets) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "rollback_b64": common.encode_script(
                build_rollback_script(paths, assets)
            ),
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-persistence-*) ;; *) exit 1 ;; esac
if [ ! -e "$stage_root" ]; then emit cleanup_restored yes; exit 0; fi
[ -d "$stage_root" ]
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
if [ -f "$rollback_state" ]; then
  if [ ! -f "$rollback_script" ]; then
    printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
    chmod 700 "$rollback_script"
  fi
  /bin/sh "$rollback_script"
else
  unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 \
    ! -name .hardened-blikvm-owner ! -name gate ! -name gate.part \
    ! -name gate.service ! -name gate.service.part \
    ! -name rollback.state.part ! -name rollback.sh \
    ! -name diagnostic-sysusers.conf ! -name account-backup \
    ! -name synthetic-root -print -quit)
  [ -z "$unexpected" ]
  rm -f -- "$stage_root/gate" "$stage_root/gate.part" \
    "$stage_root/gate.service" "$stage_root/gate.service.part" \
    "$stage_root/rollback.state.part" "$stage_root/rollback.sh" \
    "$stage_root/diagnostic-sysusers.conf" "$marker"
  if [ -d "$stage_root/account-backup" ]; then
    for name in passwd group shadow gshadow; do rm -f -- "$stage_root/account-backup/$name"; done
    rmdir "$stage_root/account-backup"
  fi
  if [ -d "$stage_root/synthetic-root" ]; then
    rm -f -- "$stage_root/synthetic-root/etc/passwd" \
      "$stage_root/synthetic-root/etc/group" \
      "$stage_root/synthetic-root/etc/shadow" \
      "$stage_root/synthetic-root/etc/gshadow" \
      "$stage_root/synthetic-root/etc/passwd-" \
      "$stage_root/synthetic-root/etc/group-" \
      "$stage_root/synthetic-root/etc/shadow-" \
      "$stage_root/synthetic-root/etc/gshadow-" \
      "$stage_root/synthetic-root/etc/.pwd.lock" \
      "$stage_root/synthetic-root/usr/lib/sysusers.d/50-diagnostics.conf"
    rmdir "$stage_root/synthetic-root/usr/lib/sysusers.d" \
      "$stage_root/synthetic-root/usr/lib" "$stage_root/synthetic-root/usr" \
      "$stage_root/synthetic-root/etc" "$stage_root/synthetic-root"
  fi
  rmdir "$stage_root"
fi
emit cleanup_restored yes
""".strip()
    return f"{values}\n{body}"


def build_final_script(
    paths: Paths, binary_parent_before: str, credential_parent_before: str
) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "runtime_root": RUNTIME_ROOT,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "unit_target": UNIT_TARGET,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "binary_parent_before": binary_parent_before,
            "credential_parent_before": credential_parent_before,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$production_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$unit_target" ]
if getent passwd "$broker_account" >/dev/null 2>&1 || getent group "$broker_account" >/dev/null 2>&1 || getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
if [ "$binary_parent_before" = absent ]; then [ ! -e "$binary_parent" ]; else [ -d "$binary_parent" ]; fi
if [ "$credential_parent_before" = absent ]; then [ ! -e "$credential_parent" ]; else [ -d "$credential_parent" ]; fi
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit binary_absent_final yes
emit binary_parent_final "$binary_parent_before"
emit credential_absent_final yes
emit credential_parent_final "$credential_parent_before"
emit identities_absent_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit production_root_absent_final yes
emit rootfs_mode_final "$rootfs_mode"
emit runtime_root_absent_final yes
emit stage_absent_final yes
emit unit_absent_final yes
emit watchdog_inactive_final yes
""".strip()
    return f"{values}\n{body}"


def validate_preflight(facts: dict[str, str]) -> None:
    fixed = {
        "architecture": "aarch64",
        "binary_absent_before": "yes",
        "credential_absent_before": "yes",
        "identities_absent_before": "yes",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "production_root_absent_before": "yes",
        "rootfs_mode_before": "rw",
        "runtime_root_absent_before": "yes",
        "stage_absent_before": "yes",
        "tools_ready": "yes",
        "unit_absent_before": "yes",
        "watchdog_absent_before": "yes",
    }
    for key, value in fixed.items():
        if facts.get(key) != value:
            raise ValueError("preflight invariant failed")
    for key in ("binary_parent_before", "credential_parent_before"):
        if facts.get(key) not in {"present", "absent"}:
            raise ValueError("preflight parent state is invalid")


def validate_run(facts: dict[str, str]) -> None:
    expected = {
        "account_databases_restored": "yes",
        "acknowledged_entries": "2",
        "acknowledgement_requests": "3",
        "automatic_retries": "0",
        "binary_hash_verified": "yes",
        "cancelled_acknowledgements": "1",
        "copies_equal": "yes",
        "copies_mode_verified": "yes",
        "credential_runtime_accepted": "yes",
        "entry_count": "2",
        "final_generation": "7",
        "read_requests": "7",
        "rollback_confirmed": "yes",
        "sandbox_verified": "yes",
        "watchdog_armed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def validate_final(
    facts: dict[str, str], binary_parent_before: str, credential_parent_before: str
) -> None:
    expected = {
        "binary_absent_final": "yes",
        "binary_parent_final": binary_parent_before,
        "credential_absent_final": "yes",
        "credential_parent_final": credential_parent_before,
        "identities_absent_final": "yes",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
        "production_root_absent_final": "yes",
        "rootfs_mode_final": "rw",
        "runtime_root_absent_final": "yes",
        "stage_absent_final": "yes",
        "unit_absent_final": "yes",
        "watchdog_inactive_final": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def upload(
    target: prototype.RemoteTarget,
    secret: bytearray,
    asset: Asset,
    script: str,
) -> None:
    with asset.path.open("rb") as stream:
        common.require_result(
            "upload",
            prototype.run_ssh(target, secret, script, stdin=stream, timeout=60),
        )


def attempt_cleanup(
    target: prototype.RemoteTarget,
    secret: bytearray,
    paths: Paths,
    assets: Assets,
) -> CleanupReceipt:
    failure_phase: str | None = None
    try:
        result = prototype.run_ssh(
            target, secret, build_cleanup_script(paths, assets), timeout=90
        )
        if len(result.stdout) <= 16 * 1024:
            for line in result.stdout.decode("ascii").splitlines():
                if line.startswith("rollback_failure_phase="):
                    candidate = line.split("=", 1)[1]
                    if re.fullmatch(r"[a-z-]{1,32}", candidate):
                        failure_phase = candidate
        if result.returncode != 0:
            return {
                "attempted": True,
                "confirmed": False,
                "failurePhase": failure_phase,
            }
        facts = native.parse_facts(result.stdout, CLEANUP_KEYS)
        return {
            "attempted": True,
            "confirmed": facts == {"cleanup_restored": "yes"},
            "failurePhase": failure_phase,
        }
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {
            "attempted": True,
            "confirmed": False,
            "failurePhase": failure_phase,
        }


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    target = native.target_from_known_hosts(
        options.known_hosts, options.endpoint or None
    )
    secret = prototype.read_secret_stdin()
    paths = stage_paths(assets.binary.sha256, secrets.token_hex(8))
    required_bytes = assets.binary.size_bytes * 4 + MIN_TMP_HEADROOM_BYTES
    stage = "preflight"
    cleanup_required = False
    remote_phase: str | None = None
    try:
        preflight = native.parse_facts(
            common.require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_preflight_script(paths, required_bytes),
                    timeout=30,
                ),
            ),
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        upload(target, secret, assets.binary, build_first_upload_script(paths))
        upload(target, secret, assets.unit, build_upload_script(paths, paths.unit))

        stage = "provision-run-rollback"
        result = prototype.run_ssh(
            target, secret, build_run_script(paths, assets), timeout=180
        )
        if len(result.stdout) > 16 * 1024:
            raise ValueError("runtime output exceeded the size limit")
        if result.returncode != 0:
            for line in result.stdout.decode("ascii").splitlines():
                if line.startswith("run_failure_phase="):
                    candidate = line.split("=", 1)[1]
                    if re.fullmatch(r"[a-z-]{1,32}", candidate):
                        remote_phase = candidate
            raise RuntimeError(stage)
        checks = native.parse_facts(result.stdout, RUN_KEYS)
        validate_run(checks)

        stage = "final-state"
        final_state = native.parse_facts(
            common.require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_final_script(
                        paths,
                        preflight["binary_parent_before"],
                        preflight["credential_parent_before"],
                    ),
                    timeout=30,
                ),
            ),
            FINAL_KEYS,
        )
        validate_final(
            final_state,
            preflight["binary_parent_before"],
            preflight["credential_parent_before"],
        )
        cleanup_required = False
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-persistence",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "artifacts": {
                "binary": {
                    "sha256": assets.binary.sha256,
                    "sizeBytes": assets.binary.size_bytes,
                },
                "unitSha256": assets.unit.sha256,
            },
            "policy": {
                "credential": "ephemeral-synthetic-systemd-runtime-only",
                "deviceActionsExecuted": False,
                "externalFloor": "synthetic-process-memory-only",
                "persistentStateRestored": True,
                "physicalPowerOrReset": False,
                "productionKeyProvisioned": False,
                "rollbackWatchdog": "ten-minute-systemd",
            },
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {
                "attempted": True,
                "confirmed": True,
                "failurePhase": None,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (
        OSError,
        RuntimeError,
        subprocess.SubprocessError,
        UnicodeError,
        ValueError,
    ) as error:
        cleanup = (
            attempt_cleanup(target, secret, paths, assets)
            if cleanup_required
            else {
                "attempted": False,
                "confirmed": True,
                "failurePhase": None,
            }
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-persistence",
            "result": "failed",
            "sourceCommit": assets.source_commit,
            "failure": {
                "stage": stage,
                "failureType": type(error).__name__,
                "remotePhase": remote_phase,
            },
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
