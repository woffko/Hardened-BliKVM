#!/usr/bin/env python3
"""Reversibly test diagnostic credential and empty store maintenance on H616."""

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
CREDENTIAL_PARENT = "/etc/hardened-blikvm"
CREDENTIAL_TARGET = f"{CREDENTIAL_PARENT}/diagnostic-journal-keyring"
BINARY_PARENT = "/usr/libexec/hardened-blikvm"
BINARY_TARGET = f"{BINARY_PARENT}/bli-kvm-diagnostic-maintenance"
INIT_UNIT_NAME = "hardened-blikvm-diagnostic-store-init.service"
INSPECT_UNIT_NAME = "hardened-blikvm-diagnostic-store-inspect.service"
INIT_UNIT_TARGET = f"/run/systemd/system/{INIT_UNIT_NAME}"
INSPECT_UNIT_TARGET = f"/run/systemd/system/{INSPECT_UNIT_NAME}"
BROKER_ACCOUNT = "hardened-blikvm-broker"
SHARED_GROUP = "hardened-blikvm"
ACCOUNT_DATABASE_FILES = ("passwd", "group", "shadow", "gshadow")
COPY_BYTES = 16_552
CREDENTIAL_BYTES = 96
MAX_BINARY_BYTES = 8 * 1024 * 1024
MIN_TMP_HEADROOM_BYTES = 8 * 1024 * 1024
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-diagnostic-maintenance-[0-9a-f]{12}-[0-9a-f]{16}$"
)
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-diagnostic-maintenance-[0-9a-f]{64}$"
)

PREFLIGHT_KEYS = {
    "architecture",
    "binary_absent_before",
    "binary_parent_before",
    "credential_absent_before",
    "credential_parent_before",
    "identity_state_before",
    "init_unit_absent_before",
    "inspect_unit_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "production_root_absent_before",
    "rootfs_mode_before",
    "stage_absent_before",
    "tools_ready",
    "watchdog_absent_before",
}
RUN_KEYS = {
    "account_databases_restored",
    "binary_hash_verified",
    "copies_equal",
    "copies_mode_verified",
    "credential_runtime_accepted",
    "empty_generation",
    "init_result",
    "inspect_nonmutating",
    "inspect_result",
    "rollback_confirmed",
    "sandbox_sources_verified",
    "temporary_rejected_preserved",
    "watchdog_armed",
}
FINAL_KEYS = {
    "binary_absent_final",
    "binary_parent_final",
    "credential_absent_final",
    "credential_parent_final",
    "identity_state_final",
    "init_unit_absent_final",
    "inspect_unit_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "production_root_absent_final",
    "rootfs_mode_final",
    "stage_absent_final",
    "watchdog_inactive_final",
}
CLEANUP_KEYS = {"cleanup_restored"}
RECOVERY_KEYS = {
    "binary_parent_before",
    "credential_parent_before",
    "identity_state_before",
    "persistent_state",
    "run_id",
    "state_present",
    "watchdog_state",
}
RECOVERY_OVERVIEW_KEYS = {
    "account_state",
    "binary_present",
    "credential_present",
    "init_unit_present",
    "inspect_unit_present",
    "kvmd_active",
    "kvmd_nginx_active",
    "marker_count",
    "production_root_present",
    "rootfs_mode",
    "stage_count",
    "state_count",
    "watchdog_service_active_count",
    "watchdog_timer_active_count",
}
ORPHAN_CLEANUP_KEYS = {"orphan_stage_removed"}


@dataclass(frozen=True)
class Options:
    binary: str
    source_commit: str
    init_unit: str
    inspect_unit: str
    tmpfiles: str
    endpoint: str
    known_hosts: str
    output: str
    recover_only: bool


@dataclass(frozen=True)
class Asset:
    path: Path
    sha256: str
    size_bytes: int


@dataclass(frozen=True)
class Assets:
    binary: Asset
    init_unit: Asset
    inspect_unit: Asset
    tmpfiles: Asset
    source_commit: str


@dataclass(frozen=True)
class Paths:
    root: str
    marker: str
    marker_value: str
    binary: str
    init_unit: str
    inspect_unit: str
    tmpfiles: str
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
        "--init-unit",
        default="packaging/systemd/hardened-blikvm-diagnostic-store-init.service",
    )
    parser.add_argument(
        "--inspect-unit",
        default="packaging/systemd/hardened-blikvm-diagnostic-store-inspect.service",
    )
    parser.add_argument(
        "--tmpfiles",
        default="packaging/tmpfiles.d/hardened-blikvm-diagnostics.conf",
    )
    parser.add_argument("--endpoint", default="")
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument(
        "--output",
        default="evidence/private/device-diagnostic-maintenance.json",
    )
    parser.add_argument("--recover-only", action="store_true")
    namespace = parser.parse_args()
    return Options(
        binary=cast(str, namespace.binary),
        source_commit=cast(str, namespace.source_commit),
        init_unit=cast(str, namespace.init_unit),
        inspect_unit=cast(str, namespace.inspect_unit),
        tmpfiles=cast(str, namespace.tmpfiles),
        endpoint=cast(str, namespace.endpoint),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        recover_only=cast(bool, namespace.recover_only),
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
    if path.name != f"hardened-blikvm-diagnostic-maintenance-{sha256}":
        raise ValueError("binary snapshot name does not match SHA-256")
    return Asset(path, sha256, metadata.st_size)


def validate_config(path_value: str, expected: Path) -> Asset:
    validated = common.validate_config(path_value, expected)
    return Asset(validated.path, validated.sha256, validated.size_bytes)


def validate_assets(options: Options) -> Assets:
    if SOURCE_COMMIT.fullmatch(options.source_commit) is None:
        raise ValueError("source commit is invalid")
    return Assets(
        binary=validate_binary(options.binary),
        init_unit=validate_config(
            options.init_unit,
            Path("packaging/systemd/hardened-blikvm-diagnostic-store-init.service"),
        ),
        inspect_unit=validate_config(
            options.inspect_unit,
            Path(
                "packaging/systemd/"
                "hardened-blikvm-diagnostic-store-inspect.service"
            ),
        ),
        tmpfiles=validate_config(
            options.tmpfiles,
            Path("packaging/tmpfiles.d/hardened-blikvm-diagnostics.conf"),
        ),
        source_commit=options.source_commit,
    )


def stage_paths(binary_sha256: str, run_id: str) -> Paths:
    if SHA256.fullmatch(binary_sha256) is None or RUN_ID.fullmatch(run_id) is None:
        raise ValueError("staging identity is invalid")
    root = (
        f"/tmp/hardened-blikvm-diagnostic-maintenance-"
        f"{binary_sha256[:12]}-{run_id}"
    )
    if STAGE_ROOT.fullmatch(root) is None:
        raise ValueError("staging root is invalid")
    marker_value = f"diagnostic-maintenance:{binary_sha256}:{run_id}"
    return Paths(
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        marker_value=marker_value,
        binary=f"{root}/maintenance",
        init_unit=f"{root}/init.service",
        inspect_unit=f"{root}/inspect.service",
        tmpfiles=f"{root}/tmpfiles.conf",
        rollback=f"{root}/rollback.sh",
        state=f"{root}/rollback.state",
        account_backup=f"{root}/account-backup",
        synthetic_root=f"{root}/synthetic-root",
        sysusers=f"{root}/diagnostic-sysusers.conf",
        watchdog=f"hardened-blikvm-diagnostic-rollback-{run_id}",
    )


def build_preflight_script(paths: Paths, required_bytes: int) -> str:
    values = common.assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "production_root": PRODUCTION_ROOT,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "credential_part": f"{CREDENTIAL_TARGET}.part",
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "binary_part": f"{BINARY_TARGET}.part",
            "init_target": INIT_UNIT_TARGET,
            "init_part": f"{INIT_UNIT_TARGET}.part",
            "inspect_target": INSPECT_UNIT_TARGET,
            "inspect_part": f"{INSPECT_UNIT_TARGET}.part",
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
[ ! -e "$credential_target" ]
[ ! -e "$credential_part" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
[ ! -e "$init_target" ]
[ ! -e "$init_part" ]
[ ! -e "$inspect_target" ]
[ ! -e "$inspect_part" ]
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
if [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_before=present
else
  binary_parent_before=absent
fi
if [ -e "$credential_parent" ]; then
  [ -d "$credential_parent" ]
  [ ! -L "$credential_parent" ]
  case "$(stat -c '%a:%u:%g' "$credential_parent")" in
    700:0:0|750:0:0|755:0:0) ;;
    *) exit 1 ;;
  esac
  credential_parent_before=present
else
  credential_parent_before=absent
fi
if getent passwd "$broker_account" >/dev/null 2>&1 || \
   getent group "$broker_account" >/dev/null 2>&1 || \
   getent group "$shared_group" >/dev/null 2>&1; then
  getent passwd "$broker_account" | awk -F: '$3 != 0 && $6 == "/nonexistent" && $7 == "/usr/sbin/nologin" {ok=1} END {exit !ok}'
  getent group "$broker_account" | awk -F: '$3 != 0 {ok=1} END {exit !ok}'
  getent group "$shared_group" | awk -F: -v account="$broker_account" '$3 != 0 && ("," $4 ",") ~ ("," account ",") {ok=1} END {exit !ok}'
  identity_state=present
else
  identity_state=absent
fi
for database in /etc/passwd /etc/group /etc/shadow /etc/gshadow; do
  [ -f "$database" ]
  [ ! -L "$database" ]
done
for tool in awk base64 cat chmod chown cmp cp cut dd df find findmnt getent grep install mkdir mv od readlink rm rmdir sha256sum stat sync systemctl systemd-analyze systemd-run systemd-sysusers systemd-tmpfiles tail tr uname wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit architecture aarch64
emit binary_absent_before yes
emit binary_parent_before "$binary_parent_before"
emit credential_absent_before yes
emit credential_parent_before "$credential_parent_before"
emit identity_state_before "$identity_state"
emit init_unit_absent_before yes
emit inspect_unit_absent_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit production_root_absent_before yes
emit rootfs_mode_before "$rootfs_mode"
emit stage_absent_before yes
emit tools_ready yes
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
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-maintenance-*) ;; *) exit 1 ;; esac
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
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "credential_part": f"{CREDENTIAL_TARGET}.part",
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "binary_size": assets.binary.size_bytes,
            "binary_sha": assets.binary.sha256,
            "init_target": INIT_UNIT_TARGET,
            "init_size": assets.init_unit.size_bytes,
            "init_sha": assets.init_unit.sha256,
            "inspect_target": INSPECT_UNIT_TARGET,
            "inspect_size": assets.inspect_unit.size_bytes,
            "inspect_sha": assets.inspect_unit.sha256,
            "init_name": INIT_UNIT_NAME,
            "inspect_name": INSPECT_UNIT_NAME,
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
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-maintenance-*) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$state_file" ]
value() {{ awk -F= -v key="$1" '$1 == key {{print $2}}' "$state_file"; }}
parent_before=$(value parent_before)
credential_parent_before=$(value credential_parent_before)
identity_before=$(value identity_before)
case "$parent_before:$credential_parent_before:$identity_before" in
  present:present:present|present:present:absent|present:absent:present|present:absent:absent|absent:present:present|absent:present:absent|absent:absent:present|absent:absent:absent) ;;
  *) exit 1 ;;
esac
broker_uid=$(value broker_uid)
shared_gid=$(value shared_gid)
case "$broker_uid:$shared_gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
phase=stop-units
systemctl stop "$init_name" 2>/dev/null || true
systemctl stop "$inspect_name" 2>/dev/null || true
phase=remove-store
if [ -e "$production_root" ]; then
  [ -d "$production_root" ]
  [ ! -L "$production_root" ]
  [ "$(stat -c '%a:%u:%g' "$production_root")" = "700:$broker_uid:$shared_gid" ]
  unexpected=$(find "$production_root" -mindepth 1 -maxdepth 1 \
    ! -name journal-a.bin ! -name journal-b.bin \
    ! -name .journal-a.tmp ! -name .journal-b.tmp -print -quit)
  [ -z "$unexpected" ]
  for name in journal-a.bin journal-b.bin .journal-a.tmp .journal-b.tmp; do
    path="$production_root/$name"
    if [ -e "$path" ]; then
      [ -f "$path" ]
      [ ! -L "$path" ]
      mode_owner=$(stat -c '%a:%u:%g:%h' "$path")
      [ "$mode_owner" = "600:$broker_uid:$shared_gid:1" ]
      size=$(stat -c %s "$path")
      case "$name" in journal-*) [ "$size" = {COPY_BYTES} ] ;; *) [ "$size" -le {COPY_BYTES} ] ;; esac
      rm -f -- "$path"
    fi
  done
  rmdir "$production_root"
fi
phase=remove-credential
for path in "$credential_part" "$credential_target"; do
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    case "$(stat -c '%a:%u:%g:%h' "$path")" in 400:0:0:1|600:0:0:1) ;; *) exit 1 ;; esac
    [ "$(stat -c %s "$path")" -le {CREDENTIAL_BYTES} ]
    rm -f -- "$path"
  fi
done
phase=remove-units
for spec in "$init_target:$init_sha:$init_size" "$inspect_target:$inspect_sha:$inspect_size"; do
  path=${{spec%%:*}}
  rest=${{spec#*:}}
  expected_sha=${{rest%%:*}}
  expected_size=${{rest##*:}}
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "$path")" = "644:0:0:1:$expected_size" ]
    [ "$(sha256sum "$path" | cut -d ' ' -f 1)" = "$expected_sha" ]
    rm -f -- "$path"
  fi
  part="$path.part"
  if [ -e "$part" ]; then
    [ -f "$part" ]
    [ ! -L "$part" ]
    [ "$(stat -c '%a:%u:%g:%h' "$part")" = 644:0:0:1 ]
    [ "$(stat -c %s "$part")" -le "$expected_size" ]
    rm -f -- "$part"
  fi
done
systemctl daemon-reload
systemctl reset-failed "$init_name" "$inspect_name" 2>/dev/null || true
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
if [ "$identity_before" = absent ]; then
  for database in $account_files; do
    name=${{database##*/}}
    before=$(value "before_$name")
    modified=$(value "modified_$name")
    case "$before:$modified" in *[!0-9a-f:]*|:|*:|*:) exit 1 ;; esac
    [ "${{#before}}" = 64 ]
    [ "${{#modified}}" = 64 ]
    current=$(sha256sum "$database" | cut -d ' ' -f 1)
    if [ "$current" = "$modified" ]; then
      restore_tmp="/etc/.$name.hardened-blikvm-diagnostic-{paths.root.rsplit('-', 1)[-1]}"
      [ ! -e "$restore_tmp" ]
      cp -a -- "$account_backup/$name" "$restore_tmp"
      sync "$restore_tmp"
      mv -f -- "$restore_tmp" "$database"
      sync "$database"
    elif [ "$current" != "$before" ]; then
      exit 3
    fi
    [ "$(sha256sum "$database" | cut -d ' ' -f 1)" = "$before" ]
    cmp -s "$account_backup/$name" "$database"
  done
  if getent passwd "$broker_account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$broker_account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
fi
phase=remove-parents
if [ "$credential_parent_before" = absent ] && [ -e "$credential_parent" ]; then
  rmdir "$credential_parent"
fi
if [ "$parent_before" = absent ] && [ -e "$binary_parent" ]; then
  rmdir "$binary_parent"
fi
phase=stop-watchdog
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${{INVOCATION_ID:-}}" ]; then systemctl stop "$watchdog.service" 2>/dev/null || true; fi
phase=remove-stage
rm -f -- "$state_file" "$stage_root/rollback.sh" "$sysusers_file"
rm -f -- "$stage_root/maintenance" "$stage_root/init.service" \
  "$stage_root/inspect.service" "$stage_root/tmpfiles.conf" "$marker"
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
    values = common.assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "binary": paths.binary,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "init_source": paths.init_unit,
            "init_sha": assets.init_unit.sha256,
            "inspect_source": paths.inspect_unit,
            "inspect_sha": assets.inspect_unit.sha256,
            "tmpfiles_source": paths.tmpfiles,
            "tmpfiles_sha": assets.tmpfiles.sha256,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "rollback_b64": rollback_b64,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "credential_part": f"{CREDENTIAL_TARGET}.part",
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "init_name": INIT_UNIT_NAME,
            "inspect_name": INSPECT_UNIT_NAME,
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
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(stat -c %s "$binary")" = "$binary_size" ]
[ "$(sha256sum "$binary" | cut -d ' ' -f 1)" = "$binary_sha" ]
[ "$(sha256sum "$init_source" | cut -d ' ' -f 1)" = "$init_sha" ]
[ "$(sha256sum "$inspect_source" | cut -d ' ' -f 1)" = "$inspect_sha" ]
[ "$(sha256sum "$tmpfiles_source" | cut -d ' ' -f 1)" = "$tmpfiles_sha" ]
if [ -e "$binary_parent" ]; then parent_before=present; else parent_before=absent; fi
if [ -e "$credential_parent" ]; then credential_parent_before=present; else credential_parent_before=absent; fi
if getent passwd "$broker_account" >/dev/null 2>&1; then identity_before=present; else identity_before=absent; fi
state_part="$rollback_state.part"
[ ! -e "$state_part" ]
printf 'parent_before=%s\ncredential_parent_before=%s\nidentity_before=%s\n' \
  "$parent_before" "$credential_parent_before" "$identity_before" > "$state_part"
if [ "$identity_before" = absent ]; then
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
  for database in $account_files; do
    name=${{database##*/}}
    cp -a -- "$account_backup/$name" "$synthetic_root/etc/$name"
  done
  install -m 0644 "$sysusers_file" "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
  systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
  [ "$(awk -F: -v account="$broker_account" '$1 == account {{print $3}}' "$synthetic_root/etc/passwd")" = "$broker_uid" ]
  [ "$(awk -F: -v group="$shared_group" '$1 == group {{print $3}}' "$synthetic_root/etc/group")" = "$shared_gid" ]
  for database in $account_files; do
    name=${{database##*/}}
    printf 'modified_%s=%s\n' "$name" "$(sha256sum "$synthetic_root/etc/$name" | cut -d ' ' -f 1)" >> "$state_part"
  done
else
  broker_uid=$(getent passwd "$broker_account" | awk -F: 'NR == 1 {{print $3}}')
  shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {{print $3}}')
fi
case "$broker_uid:$shared_gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
printf 'broker_uid=%s\nshared_gid=%s\n' "$broker_uid" "$shared_gid" >> "$state_part"
chmod 600 "$state_part"
sync "$state_part"
mv "$state_part" "$rollback_state"
sync "$rollback_state"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
sync "$rollback_state" "$rollback_script"
systemd-run --quiet --unit="$watchdog" --on-active=10min \
  --timer-property=AccuracySec=1s /bin/sh "$rollback_script"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
if [ "$identity_before" = absent ]; then
  systemd-sysusers "$sysusers_file" >/dev/null 2>&1
  for database in $account_files; do
    name=${{database##*/}}
    expected=$(awk -F= -v key="modified_$name" '$1 == key {{print $2}}' "$rollback_state")
    [ "$(sha256sum "$database" | cut -d ' ' -f 1)" = "$expected" ]
  done
fi
getent group "$shared_group" | awk -F: -v account="$broker_account" '("," $4 ",") ~ ("," account ",") {{ok=1}} END {{exit !ok}}'
if [ "$parent_before" = absent ]; then mkdir -m 755 "$binary_parent"; fi
install -m 0755 "$binary" "$binary_target.part"
mv "$binary_target.part" "$binary_target"
[ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
install -m 0644 "$init_source" "$init_target.part"
install -m 0644 "$inspect_source" "$inspect_target.part"
[ "$(sha256sum "$init_target.part" | cut -d ' ' -f 1)" = "$init_sha" ]
[ "$(sha256sum "$inspect_target.part" | cut -d ' ' -f 1)" = "$inspect_sha" ]
mv "$init_target.part" "$init_target"
mv "$inspect_target.part" "$inspect_target"
if [ "$credential_parent_before" = absent ]; then mkdir -m 700 "$credential_parent"; fi
umask 077
[ ! -e "$credential_part" ]
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
systemctl daemon-reload
systemd-analyze verify "$init_name" "$inspect_name" >/dev/null 2>&1
systemd-tmpfiles --create "$tmpfiles_source"
[ "$(stat -c '%a:%u:%g' "$production_root")" = "700:$broker_uid:$shared_gid" ]
systemctl start "$init_name"
[ "$(systemctl show -p Result --value "$init_name")" = success ]
[ "$(systemctl show -p ExecMainStatus --value "$init_name")" = 0 ]
for unit in "$init_name" "$inspect_name"; do
  [ "$(systemctl show -p User --value "$unit")" = "$broker_account" ]
  [ "$(systemctl show -p Group --value "$unit")" = "$shared_group" ]
  [ "$(systemctl show -p PrivateNetwork --value "$unit")" = yes ]
  [ "$(systemctl show -p DevicePolicy --value "$unit")" = closed ]
done
for copy in journal-a.bin journal-b.bin; do
  [ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/$copy")" = "600:$broker_uid:$shared_gid:1:{COPY_BYTES}" ]
done
copy_a_sha=$(sha256sum "$production_root/journal-a.bin" | cut -d ' ' -f 1)
copy_b_sha=$(sha256sum "$production_root/journal-b.bin" | cut -d ' ' -f 1)
[ "$copy_a_sha" = "$copy_b_sha" ]
meta_a=$(stat -c '%i:%s:%Y:%Z' "$production_root/journal-a.bin")
meta_b=$(stat -c '%i:%s:%Y:%Z' "$production_root/journal-b.bin")
systemctl start "$inspect_name"
[ "$(systemctl show -p Result --value "$inspect_name")" = success ]
[ "$(systemctl show -p ExecMainStatus --value "$inspect_name")" = 0 ]
[ "$(sha256sum "$production_root/journal-a.bin" | cut -d ' ' -f 1)" = "$copy_a_sha" ]
[ "$(sha256sum "$production_root/journal-b.bin" | cut -d ' ' -f 1)" = "$copy_b_sha" ]
[ "$(stat -c '%i:%s:%Y:%Z' "$production_root/journal-a.bin")" = "$meta_a" ]
[ "$(stat -c '%i:%s:%Y:%Z' "$production_root/journal-b.bin")" = "$meta_b" ]
printf partial > "$production_root/.journal-a.tmp"
chown "$broker_uid:$shared_gid" "$production_root/.journal-a.tmp"
chmod 600 "$production_root/.journal-a.tmp"
if systemctl start "$inspect_name" >/dev/null 2>&1; then exit 1; fi
[ "$(cat "$production_root/.journal-a.tmp")" = partial ]
[ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/.journal-a.tmp")" = "600:$broker_uid:$shared_gid:1:7" ]
[ "$(sha256sum "$production_root/journal-a.bin" | cut -d ' ' -f 1)" = "$copy_a_sha" ]
[ "$(sha256sum "$production_root/journal-b.bin" | cut -d ' ' -f 1)" = "$copy_b_sha" ]
rm -f -- "$production_root/.journal-a.tmp"
systemctl reset-failed "$inspect_name" 2>/dev/null || true
systemctl start "$inspect_name"
[ "$(systemctl show -p Result --value "$inspect_name")" = success ]
/bin/sh "$rollback_script"
[ ! -e "$production_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$init_target" ]
[ ! -e "$inspect_target" ]
emit account_databases_restored yes
emit binary_hash_verified yes
emit copies_equal yes
emit copies_mode_verified yes
emit credential_runtime_accepted yes
emit empty_generation 1
emit init_result success
emit inspect_nonmutating yes
emit inspect_result success
emit rollback_confirmed yes
emit sandbox_sources_verified yes
emit temporary_rejected_preserved yes
emit watchdog_armed yes
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
            "rollback_b64": common.encode_script(build_rollback_script(paths, assets)),
            "production_root": PRODUCTION_ROOT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_target": BINARY_TARGET,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
        }
    )
    body = r"""
set -eu
if [ -d "$stage_root" ]; then
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  if [ -f "$rollback_state" ]; then
    if [ ! -x "$rollback_script" ]; then
      printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
      chmod 700 "$rollback_script"
    fi
    /bin/sh "$rollback_script"
  else
    [ ! -e "$production_root" ]
    [ ! -e "$credential_target" ]
    [ ! -e "$binary_target" ]
    [ ! -e "$init_target" ]
    [ ! -e "$inspect_target" ]
    rm -f -- "$rollback_script" "$rollback_state.part" "$sysusers_file"
    if [ -d "$account_backup" ]; then
      for name in passwd group shadow gshadow; do rm -f -- "$account_backup/$name"; done
      rmdir "$account_backup"
    fi
    if [ -d "$synthetic_root" ]; then
      rm -f -- "$synthetic_root/etc/passwd" "$synthetic_root/etc/group" \
        "$synthetic_root/etc/shadow" "$synthetic_root/etc/gshadow" \
        "$synthetic_root/etc/passwd-" "$synthetic_root/etc/group-" \
        "$synthetic_root/etc/shadow-" "$synthetic_root/etc/gshadow-" \
        "$synthetic_root/etc/.pwd.lock" \
        "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
      for directory in "$synthetic_root/usr/lib/sysusers.d" \
        "$synthetic_root/usr/lib" "$synthetic_root/usr" \
        "$synthetic_root/etc" "$synthetic_root"; do
        if [ -d "$directory" ]; then rmdir "$directory"; fi
      done
    fi
    rm -f -- "$stage_root/maintenance" "$stage_root/init.service" \
      "$stage_root/inspect.service" "$stage_root/tmpfiles.conf" "$marker"
    rmdir "$stage_root"
  fi
fi
[ ! -e "$stage_root" ]
printf 'cleanup_restored=yes\n'
""".strip()
    return f"{values}\n{body}"


def build_final_script(
    paths: Paths,
    binary_parent_before: str,
    credential_parent_before: str,
    identity_state_before: str,
) -> str:
    for value in (
        binary_parent_before,
        credential_parent_before,
        identity_state_before,
    ):
        if value not in {"present", "absent"}:
            raise ValueError("final-state baseline is invalid")
    values = common.assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "credential_parent": CREDENTIAL_PARENT,
            "credential_target": CREDENTIAL_TARGET,
            "credential_part": f"{CREDENTIAL_TARGET}.part",
            "credential_parent_before": credential_parent_before,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "binary_part": f"{BINARY_TARGET}.part",
            "binary_parent_before": binary_parent_before,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "identity_state_before": identity_state_before,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$production_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$credential_part" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
[ ! -e "$init_target" ]
[ ! -e "$inspect_target" ]
if [ "$binary_parent_before" = present ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_final=present
else
  [ ! -e "$binary_parent" ]
  binary_parent_final=absent
fi
if [ "$credential_parent_before" = present ]; then
  [ -d "$credential_parent" ]
  [ ! -L "$credential_parent" ]
  credential_parent_final=present
else
  [ ! -e "$credential_parent" ]
  credential_parent_final=absent
fi
if [ "$identity_state_before" = present ]; then
  getent passwd "$broker_account" >/dev/null
  getent group "$shared_group" >/dev/null
  identity_state_final=present
else
  if getent passwd "$broker_account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$broker_account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
  identity_state_final=absent
fi
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit binary_absent_final yes
emit binary_parent_final "$binary_parent_final"
emit credential_absent_final yes
emit credential_parent_final "$credential_parent_final"
emit identity_state_final "$identity_state_final"
emit init_unit_absent_final yes
emit inspect_unit_absent_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit production_root_absent_final yes
emit rootfs_mode_final "$rootfs_mode"
emit stage_absent_final yes
emit watchdog_inactive_final yes
""".strip()
    return f"{values}\n{body}"


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "architecture": "aarch64",
        "binary_absent_before": "yes",
        "binary_parent_before": facts.get("binary_parent_before", ""),
        "credential_absent_before": "yes",
        "credential_parent_before": facts.get("credential_parent_before", ""),
        "identity_state_before": facts.get("identity_state_before", ""),
        "init_unit_absent_before": "yes",
        "inspect_unit_absent_before": "yes",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "production_root_absent_before": "yes",
        "rootfs_mode_before": "rw",
        "stage_absent_before": "yes",
        "tools_ready": "yes",
        "watchdog_absent_before": "yes",
    }
    for key in (
        "binary_parent_before",
        "credential_parent_before",
        "identity_state_before",
    ):
        if expected[key] not in {"present", "absent"}:
            raise ValueError("preflight state is invalid")
    if facts != dict(sorted(expected.items())):
        raise ValueError("preflight invariant failed")


def validate_run(facts: dict[str, str]) -> None:
    expected = {
        "account_databases_restored": "yes",
        "binary_hash_verified": "yes",
        "copies_equal": "yes",
        "copies_mode_verified": "yes",
        "credential_runtime_accepted": "yes",
        "empty_generation": "1",
        "init_result": "success",
        "inspect_nonmutating": "yes",
        "inspect_result": "success",
        "rollback_confirmed": "yes",
        "sandbox_sources_verified": "yes",
        "temporary_rejected_preserved": "yes",
        "watchdog_armed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def validate_final(
    facts: dict[str, str],
    binary_parent_before: str,
    credential_parent_before: str,
    identity_state_before: str,
) -> None:
    if any(
        value not in {"present", "absent"}
        for value in (
            binary_parent_before,
            credential_parent_before,
            identity_state_before,
        )
    ):
        raise ValueError("final-state baseline is invalid")
    expected = {
        "binary_absent_final": "yes",
        "binary_parent_final": binary_parent_before,
        "credential_absent_final": "yes",
        "credential_parent_final": credential_parent_before,
        "identity_state_final": identity_state_before,
        "init_unit_absent_final": "yes",
        "inspect_unit_absent_final": "yes",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
        "production_root_absent_final": "yes",
        "rootfs_mode_final": "rw",
        "stage_absent_final": "yes",
        "watchdog_inactive_final": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def build_recovery_discovery_script(binary_sha256: str) -> str:
    if SHA256.fullmatch(binary_sha256) is None:
        raise ValueError("binary SHA-256 is invalid")
    values = common.assignments(
        {
            "prefix": (
                "/tmp/hardened-blikvm-diagnostic-maintenance-"
                f"{binary_sha256[:12]}-"
            ),
            "marker_prefix": f"diagnostic-maintenance:{binary_sha256}:",
            "production_root": PRODUCTION_ROOT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "credential_parent": CREDENTIAL_PARENT,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "broker_account": BROKER_ACCOUNT,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
count=0
found=
for candidate in "$prefix"*; do
  [ -d "$candidate" ] || continue
  count=$((count + 1))
  found=$candidate
done
[ "$count" = 1 ]
run_id=${found##*-}
case "$run_id" in *[!0-9a-f]*|'') exit 1 ;; esac
[ "${#run_id}" = 16 ]
[ "$found" = "$prefix$run_id" ]
marker="$found/.hardened-blikvm-owner"
state="$found/rollback.state"
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_prefix$run_id" ]
if [ -f "$state" ]; then
  value() { awk -F= -v key="$1" '$1 == key {print $2}' "$state"; }
  binary_parent_before=$(value parent_before)
  credential_parent_before=$(value credential_parent_before)
  identity_state_before=$(value identity_before)
  state_present=yes
else
  if [ -e "$binary_parent" ]; then binary_parent_before=present; else binary_parent_before=absent; fi
  if [ -e "$credential_parent" ]; then credential_parent_before=present; else credential_parent_before=absent; fi
  if getent passwd "$broker_account" >/dev/null 2>&1; then identity_state_before=present; else identity_state_before=absent; fi
  state_present=no
fi
for value in "$binary_parent_before" "$credential_parent_before" "$identity_state_before"; do
  case "$value" in present|absent) ;; *) exit 1 ;; esac
done
watchdog="hardened-blikvm-diagnostic-rollback-$run_id"
if [ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ] || \
   [ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" = active ]; then
  watchdog_state=active
else
  watchdog_state=inactive
fi
if [ -e "$production_root" ] || [ -e "$credential_target" ] || \
   [ -e "$binary_target" ] || [ -e "$init_target" ] || [ -e "$inspect_target" ]; then
  persistent_state=present
else
  persistent_state=absent
fi
emit binary_parent_before "$binary_parent_before"
emit credential_parent_before "$credential_parent_before"
emit identity_state_before "$identity_state_before"
emit persistent_state "$persistent_state"
emit run_id "$run_id"
emit state_present "$state_present"
emit watchdog_state "$watchdog_state"
""".strip()
    return f"{values}\n{body}"


def build_recovery_overview_script(binary_sha256: str) -> str:
    if SHA256.fullmatch(binary_sha256) is None:
        raise ValueError("binary SHA-256 is invalid")
    values = common.assignments(
        {
            "prefix": (
                "/tmp/hardened-blikvm-diagnostic-maintenance-"
                f"{binary_sha256[:12]}-"
            ),
            "production_root": PRODUCTION_ROOT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_target": BINARY_TARGET,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "broker_account": BROKER_ACCOUNT,
            "shared_group": SHARED_GROUP,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
stage_count=0
marker_count=0
state_count=0
for candidate in "$prefix"*; do
  [ -d "$candidate" ] || continue
  stage_count=$((stage_count + 1))
  if [ -f "$candidate/.hardened-blikvm-owner" ]; then marker_count=$((marker_count + 1)); fi
  if [ -f "$candidate/rollback.state" ]; then state_count=$((state_count + 1)); fi
done
present() { if [ -e "$1" ]; then printf yes; else printf no; fi; }
user_present=no
broker_group_present=no
shared_group_present=no
if getent passwd "$broker_account" >/dev/null 2>&1; then user_present=yes; fi
if getent group "$broker_account" >/dev/null 2>&1; then broker_group_present=yes; fi
if getent group "$shared_group" >/dev/null 2>&1; then shared_group_present=yes; fi
case "$user_present:$broker_group_present:$shared_group_present" in
  yes:yes:yes) account_state=present ;;
  no:no:no) account_state=absent ;;
  *) account_state=partial ;;
esac
watchdog_timer_active_count=$(systemctl list-units --type=timer --state=active --plain --no-legend 2>/dev/null | awk '$1 ~ /^hardened-blikvm-diagnostic-rollback-[0-9a-f]+\.timer$/ {count++} END {print count+0}')
watchdog_service_active_count=$(systemctl list-units --type=service --state=active --plain --no-legend 2>/dev/null | awk '$1 ~ /^hardened-blikvm-diagnostic-rollback-[0-9a-f]+\.service$/ {count++} END {print count+0}')
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) rootfs_mode=other ;; esac
emit account_state "$account_state"
emit binary_present "$(present "$binary_target")"
emit credential_present "$(present "$credential_target")"
emit init_unit_present "$(present "$init_target")"
emit inspect_unit_present "$(present "$inspect_target")"
emit kvmd_active "$(systemctl is-active kvmd.service 2>/dev/null || true)"
emit kvmd_nginx_active "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)"
emit marker_count "$marker_count"
emit production_root_present "$(present "$production_root")"
emit rootfs_mode "$rootfs_mode"
emit stage_count "$stage_count"
emit state_count "$state_count"
emit watchdog_service_active_count "$watchdog_service_active_count"
emit watchdog_timer_active_count "$watchdog_timer_active_count"
""".strip()
    return f"{values}\n{body}"


def build_orphan_stage_cleanup_script(assets: Assets) -> str:
    values = common.assignments(
        {
            "prefix": (
                "/tmp/hardened-blikvm-diagnostic-maintenance-"
                f"{assets.binary.sha256[:12]}-"
            ),
            "production_root": PRODUCTION_ROOT,
            "credential_target": CREDENTIAL_TARGET,
            "binary_target": BINARY_TARGET,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "init_sha": assets.init_unit.sha256,
            "init_size": assets.init_unit.size_bytes,
            "inspect_sha": assets.inspect_unit.sha256,
            "inspect_size": assets.inspect_unit.size_bytes,
            "tmpfiles_sha": assets.tmpfiles.sha256,
            "tmpfiles_size": assets.tmpfiles.size_bytes,
        }
    )
    body = r"""
set -eu
phase=validate-root
trap 'status=$?; trap - EXIT; if [ "$status" -ne 0 ]; then printf "orphan_failure_phase=%s\n" "$phase"; fi; exit "$status"' EXIT
count=0
stage_root=
for candidate in "$prefix"*; do
  [ -d "$candidate" ] || continue
  count=$((count + 1))
  stage_root=$candidate
done
[ "$count" = 1 ]
run_id=${stage_root##*-}
case "$run_id" in *[!0-9a-f]*|'') exit 1 ;; esac
[ "${#run_id}" = 16 ]
[ "$stage_root" = "$prefix$run_id" ]
[ "$(stat -c '%a:%u:%g' "$stage_root")" = 700:0:0 ]
[ ! -e "$stage_root/.hardened-blikvm-owner" ]
[ ! -e "$stage_root/rollback.state" ]
[ ! -e "$production_root" ]
[ ! -e "$credential_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$init_target" ]
[ ! -e "$inspect_target" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) ;; *) exit 1 ;; esac
timer_count=$(systemctl list-units --type=timer --state=active --plain --no-legend 2>/dev/null | awk '$1 ~ /^hardened-blikvm-diagnostic-rollback-[0-9a-f]+\.timer$/ {count++} END {print count+0}')
service_count=$(systemctl list-units --type=service --state=active --plain --no-legend 2>/dev/null | awk '$1 ~ /^hardened-blikvm-diagnostic-rollback-[0-9a-f]+\.service$/ {count++} END {print count+0}')
[ "$timer_count" = 0 ]
[ "$service_count" = 0 ]
phase=validate-top
unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 \
  ! -name maintenance ! -name init.service ! -name inspect.service \
  ! -name tmpfiles.conf ! -name rollback.sh ! -name rollback.state.part \
  ! -name diagnostic-sysusers.conf ! -name account-backup \
  ! -name synthetic-root -print -quit)
[ -z "$unexpected" ]
validate_asset() {
  path=$1 expected_sha=$2 expected_size=$3 expected_mode=$4
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "$path")" = "$expected_mode:0:0:1:$expected_size" ]
    [ "$(sha256sum "$path" | cut -d ' ' -f 1)" = "$expected_sha" ]
  fi
}
validate_private() {
  path=$1 maximum=$2
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%u:%h' "$path")" = 0:1 ]
    [ "$(stat -c %s "$path")" -le "$maximum" ]
  fi
}
validate_asset "$stage_root/maintenance" "$binary_sha" "$binary_size" 700
validate_asset "$stage_root/init.service" "$init_sha" "$init_size" 600
validate_asset "$stage_root/inspect.service" "$inspect_sha" "$inspect_size" 600
validate_asset "$stage_root/tmpfiles.conf" "$tmpfiles_sha" "$tmpfiles_size" 600
validate_private "$stage_root/rollback.sh" 262144
validate_private "$stage_root/rollback.state.part" 16384
validate_private "$stage_root/diagnostic-sysusers.conf" 4096
phase=validate-account-backup
if [ -e "$stage_root/account-backup" ]; then
  [ -d "$stage_root/account-backup" ]
  [ ! -L "$stage_root/account-backup" ]
  unexpected=$(find "$stage_root/account-backup" -mindepth 1 -maxdepth 1 \
    ! -name passwd ! -name group ! -name shadow ! -name gshadow -print -quit)
  [ -z "$unexpected" ]
  for name in passwd group shadow gshadow; do
    validate_private "$stage_root/account-backup/$name" 1048576
  done
fi
phase=validate-synthetic
if [ -e "$stage_root/synthetic-root" ]; then
  [ -d "$stage_root/synthetic-root" ]
  [ ! -L "$stage_root/synthetic-root" ]
  unexpected=$(find "$stage_root/synthetic-root" -mindepth 1 \
    ! -type d ! -path "$stage_root/synthetic-root/etc/passwd" \
    ! -path "$stage_root/synthetic-root/etc/group" \
    ! -path "$stage_root/synthetic-root/etc/shadow" \
    ! -path "$stage_root/synthetic-root/etc/gshadow" \
    ! -path "$stage_root/synthetic-root/etc/passwd-" \
    ! -path "$stage_root/synthetic-root/etc/group-" \
    ! -path "$stage_root/synthetic-root/etc/shadow-" \
    ! -path "$stage_root/synthetic-root/etc/gshadow-" \
    ! -path "$stage_root/synthetic-root/etc/.pwd.lock" \
    ! -path "$stage_root/synthetic-root/usr/lib/sysusers.d/50-diagnostics.conf" \
    -print -quit)
  [ -z "$unexpected" ]
  for path in "$stage_root/synthetic-root/etc/passwd" \
    "$stage_root/synthetic-root/etc/group" "$stage_root/synthetic-root/etc/shadow" \
    "$stage_root/synthetic-root/etc/gshadow" "$stage_root/synthetic-root/etc/passwd-" \
    "$stage_root/synthetic-root/etc/group-" "$stage_root/synthetic-root/etc/shadow-" \
    "$stage_root/synthetic-root/etc/gshadow-" "$stage_root/synthetic-root/etc/.pwd.lock" \
    "$stage_root/synthetic-root/usr/lib/sysusers.d/50-diagnostics.conf"; do
    validate_private "$path" 1048576
  done
fi
phase=remove-top
rm -f -- "$stage_root/maintenance" "$stage_root/init.service" \
  "$stage_root/inspect.service" "$stage_root/tmpfiles.conf" \
  "$stage_root/rollback.sh" "$stage_root/rollback.state.part" \
  "$stage_root/diagnostic-sysusers.conf"
phase=remove-account-backup
if [ -d "$stage_root/account-backup" ]; then
  rm -f -- "$stage_root/account-backup/passwd" "$stage_root/account-backup/group" \
    "$stage_root/account-backup/shadow" "$stage_root/account-backup/gshadow"
  rmdir "$stage_root/account-backup"
fi
phase=remove-synthetic
if [ -d "$stage_root/synthetic-root" ]; then
  rm -f -- "$stage_root/synthetic-root/etc/passwd" \
    "$stage_root/synthetic-root/etc/group" "$stage_root/synthetic-root/etc/shadow" \
    "$stage_root/synthetic-root/etc/gshadow" "$stage_root/synthetic-root/etc/passwd-" \
    "$stage_root/synthetic-root/etc/group-" "$stage_root/synthetic-root/etc/shadow-" \
    "$stage_root/synthetic-root/etc/gshadow-" "$stage_root/synthetic-root/etc/.pwd.lock" \
    "$stage_root/synthetic-root/usr/lib/sysusers.d/50-diagnostics.conf"
  for directory in "$stage_root/synthetic-root/usr/lib/sysusers.d" \
    "$stage_root/synthetic-root/usr/lib" "$stage_root/synthetic-root/usr" \
    "$stage_root/synthetic-root/etc" "$stage_root/synthetic-root"; do
    if [ -d "$directory" ]; then rmdir "$directory"; fi
  done
fi
phase=remove-root
rmdir "$stage_root"
trap - EXIT
printf 'orphan_stage_removed=yes\n'
""".strip()
    return f"{values}\n{body}"


def attempt_cleanup(
    target: prototype.RemoteTarget,
    secret: bytearray,
    paths: Paths,
    assets: Assets,
) -> CleanupReceipt:
    try:
        result = prototype.run_ssh(
            target, secret, build_cleanup_script(paths, assets), timeout=60
        )
        if len(result.stdout) > 16 * 1024:
            raise ValueError("cleanup output exceeded the size limit")
        if result.returncode == 0:
            facts = native.parse_facts(result.stdout, CLEANUP_KEYS)
            return {
                "attempted": True,
                "confirmed": facts == {"cleanup_restored": "yes"},
                "failurePhase": None,
            }
        decoded = result.stdout.decode("ascii")
        phase = None
        for line in decoded.splitlines():
            if line.startswith("rollback_failure_phase="):
                candidate = line.split("=", 1)[1]
                if re.fullmatch(r"[a-z-]{1,32}", candidate):
                    phase = candidate
        return {"attempted": True, "confirmed": False, "failurePhase": phase}
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False, "failurePhase": None}


def upload(
    target: prototype.RemoteTarget,
    secret: bytearray,
    source: Asset,
    script: str,
) -> None:
    with source.path.open("rb") as stream:
        _ = common.require_result(
            "upload",
            prototype.run_ssh(target, secret, script, stdin=stream, timeout=60),
        )


def capture_recovery_overview(
    assets: Assets,
    target: prototype.RemoteTarget,
    secret: bytearray,
) -> dict[str, str]:
    overview = native.parse_facts(
        common.require_result(
            "recovery-overview",
            prototype.run_ssh(
                target,
                secret,
                build_recovery_overview_script(assets.binary.sha256),
                timeout=30,
            ),
        ),
        RECOVERY_OVERVIEW_KEYS,
    )
    for key in (
        "binary_present",
        "credential_present",
        "init_unit_present",
        "inspect_unit_present",
        "production_root_present",
    ):
        if overview[key] not in {"yes", "no"}:
            raise ValueError("recovery presence fact is invalid")
    for key in (
        "marker_count",
        "stage_count",
        "state_count",
        "watchdog_service_active_count",
        "watchdog_timer_active_count",
    ):
        if not overview[key].isdigit() or int(overview[key]) > 8:
            raise ValueError("recovery count is invalid")
    if overview["account_state"] not in {"present", "absent", "partial"}:
        raise ValueError("recovery account state is invalid")
    if overview["kvmd_active"] != "active" or overview["kvmd_nginx_active"] != "active":
        raise ValueError("required device service is inactive")
    if overview["rootfs_mode"] not in {"rw", "other"}:
        raise ValueError("recovery rootfs fact is invalid")
    return overview


def recover_existing(
    output: Path,
    assets: Assets,
    target: prototype.RemoteTarget,
    secret: bytearray,
) -> int:
    stage = "discover-recovery"
    cleanup: CleanupReceipt = {
        "attempted": False,
        "confirmed": False,
        "failurePhase": None,
    }
    overview: dict[str, str] | None = None
    try:
        discovery = native.parse_facts(
            common.require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_recovery_discovery_script(assets.binary.sha256),
                    timeout=30,
                ),
            ),
            RECOVERY_KEYS,
        )
        run_id = discovery["run_id"]
        if RUN_ID.fullmatch(run_id) is None:
            raise ValueError("recovery run ID is invalid")
        for key in (
            "binary_parent_before",
            "credential_parent_before",
            "identity_state_before",
        ):
            if discovery[key] not in {"present", "absent"}:
                raise ValueError("recovery baseline is invalid")
        if discovery["state_present"] not in {"yes", "no"}:
            raise ValueError("recovery state fact is invalid")
        if discovery["persistent_state"] not in {"present", "absent"}:
            raise ValueError("recovery persistence fact is invalid")
        if discovery["watchdog_state"] not in {"active", "inactive"}:
            raise ValueError("recovery watchdog fact is invalid")

        paths = stage_paths(assets.binary.sha256, run_id)
        stage = "recover-cleanup"
        cleanup = attempt_cleanup(target, secret, paths, assets)
        if not cleanup["confirmed"]:
            raise RuntimeError(stage)

        stage = "recover-final-state"
        final_state = native.parse_facts(
            common.require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_final_script(
                        paths,
                        discovery["binary_parent_before"],
                        discovery["credential_parent_before"],
                        discovery["identity_state_before"],
                    ),
                    timeout=30,
                ),
            ),
            FINAL_KEYS,
        )
        validate_final(
            final_state,
            discovery["binary_parent_before"],
            discovery["credential_parent_before"],
            discovery["identity_state_before"],
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-maintenance-recovery",
            "result": "recovered",
            "sourceCommit": assets.source_commit,
            "observation": {
                "persistentStateBefore": discovery["persistent_state"],
                "rollbackStatePresent": discovery["state_present"],
                "watchdogBefore": discovery["watchdog_state"],
            },
            "finalState": final_state,
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        try:
            overview = capture_recovery_overview(assets, target, secret)
        except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError):
            overview = None
        orphan_safe = overview is not None and all(
            (
                overview["stage_count"] == "1",
                overview["marker_count"] == "0",
                overview["state_count"] == "0",
                overview["binary_present"] == "no",
                overview["credential_present"] == "no",
                overview["init_unit_present"] == "no",
                overview["inspect_unit_present"] == "no",
                overview["production_root_present"] == "no",
                overview["watchdog_service_active_count"] == "0",
                overview["watchdog_timer_active_count"] == "0",
                overview["rootfs_mode"] == "rw",
                overview["account_state"] in {"present", "absent"},
            )
        )
        if orphan_safe and overview is not None:
            try:
                stage = "remove-orphan-stage"
                orphan_result = prototype.run_ssh(
                    target,
                    secret,
                    build_orphan_stage_cleanup_script(assets),
                    timeout=60,
                )
                if len(orphan_result.stdout) > 16 * 1024:
                    raise ValueError("orphan cleanup output exceeded the size limit")
                if orphan_result.returncode != 0:
                    phase = None
                    for line in orphan_result.stdout.decode("ascii").splitlines():
                        if line.startswith("orphan_failure_phase="):
                            candidate = line.split("=", 1)[1]
                            if re.fullmatch(r"[a-z-]{1,32}", candidate):
                                phase = candidate
                    cleanup = {
                        "attempted": True,
                        "confirmed": False,
                        "failurePhase": phase,
                    }
                    raise RuntimeError(stage)
                removed = native.parse_facts(
                    orphan_result.stdout,
                    ORPHAN_CLEANUP_KEYS,
                )
                if removed != {"orphan_stage_removed": "yes"}:
                    raise ValueError("orphan cleanup was not confirmed")
                after = capture_recovery_overview(assets, target, secret)
                if not all(
                    (
                        after["stage_count"] == "0",
                        after["marker_count"] == "0",
                        after["state_count"] == "0",
                        after["binary_present"] == "no",
                        after["credential_present"] == "no",
                        after["init_unit_present"] == "no",
                        after["inspect_unit_present"] == "no",
                        after["production_root_present"] == "no",
                        after["watchdog_service_active_count"] == "0",
                        after["watchdog_timer_active_count"] == "0",
                        after["rootfs_mode"] == "rw",
                        after["account_state"] == overview["account_state"],
                    )
                ):
                    raise ValueError("orphan cleanup final state is invalid")
                receipt = {
                    "schemaVersion": 1,
                    "probe": "hardened-blikvm-diagnostic-maintenance-recovery",
                    "result": "orphan-stage-removed",
                    "sourceCommit": assets.source_commit,
                    "observation": {
                        "accountState": overview["account_state"],
                        "persistentStateBefore": "absent",
                        "rollbackStatePresent": "no",
                        "watchdogBefore": "inactive",
                    },
                    "finalOverview": after,
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
            ) as orphan_error:
                error = orphan_error
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-maintenance-recovery",
            "result": "failed",
            "sourceCommit": assets.source_commit,
            "failure": {"stage": stage, "failureType": type(error).__name__},
            "overview": overview,
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    target = native.target_from_known_hosts(
        options.known_hosts, options.endpoint or None
    )
    secret = prototype.read_secret_stdin()
    if options.recover_only:
        try:
            return recover_existing(output, assets, target, secret)
        finally:
            native.zero(secret)
    paths = stage_paths(assets.binary.sha256, secrets.token_hex(8))
    required_bytes = assets.binary.size_bytes * 4 + MIN_TMP_HEADROOM_BYTES
    stage = "preflight"
    cleanup_required = False
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
        upload(target, secret, assets.init_unit, build_upload_script(paths, paths.init_unit))
        upload(
            target,
            secret,
            assets.inspect_unit,
            build_upload_script(paths, paths.inspect_unit),
        )
        upload(target, secret, assets.tmpfiles, build_upload_script(paths, paths.tmpfiles))

        stage = "provision-and-rollback"
        checks = native.parse_facts(
            common.require_result(
                stage,
                prototype.run_ssh(
                    target, secret, build_run_script(paths, assets), timeout=180
                ),
            ),
            RUN_KEYS,
        )
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
                        preflight["identity_state_before"],
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
            preflight["identity_state_before"],
        )
        cleanup_required = False

        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-maintenance",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "artifacts": {
                "binary": {
                    "sha256": assets.binary.sha256,
                    "sizeBytes": assets.binary.size_bytes,
                },
                "initUnitSha256": assets.init_unit.sha256,
                "inspectUnitSha256": assets.inspect_unit.sha256,
                "tmpfilesSha256": assets.tmpfiles.sha256,
            },
            "policy": {
                "commands": ["initialize-empty", "inspect-empty"],
                "credential": "ephemeral-synthetic-systemd-runtime-only",
                "deviceActionsExecuted": False,
                "externalFloorAccepted": False,
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
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            attempt_cleanup(target, secret, paths, assets)
            if cleanup_required
            else {"attempted": False, "confirmed": True, "failurePhase": None}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-maintenance",
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
