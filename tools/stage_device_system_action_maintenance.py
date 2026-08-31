#!/usr/bin/env python3
"""Reversibly test empty system-action store provisioning under systemd."""

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


PRODUCTION_ROOT = "/var/lib/hardened-blikvm-system-actions"
BINARY_PARENT = "/usr/libexec/hardened-blikvm"
BINARY_TARGET = f"{BINARY_PARENT}/bli-kvm-system-action-maintenance"
INIT_UNIT_TARGET = "/run/systemd/system/hardened-blikvm-system-action-store-init.service"
INSPECT_UNIT_TARGET = (
    "/run/systemd/system/hardened-blikvm-system-action-store-inspect.service"
)
MAX_BINARY_BYTES = 8 * 1024 * 1024
MAX_CONFIG_BYTES = 64 * 1024
MIN_TMP_HEADROOM_BYTES = 8 * 1024 * 1024
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-system-action-maintenance-[0-9a-f]{12}-[0-9a-f]{16}$"
)
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-system-action-maintenance-[0-9a-f]{64}$"
)

PREFLIGHT_KEYS = {
    "architecture",
    "binary_absent_before",
    "binary_parent_before",
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
    "binary_hash_verified",
    "copies_equal",
    "copies_mode_verified",
    "empty_generation",
    "init_result",
    "inspect_result",
    "rollback_confirmed",
    "sandbox_sources_verified",
    "watchdog_armed",
}
FINAL_KEYS = {
    "binary_absent_final",
    "binary_parent_final",
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
    watchdog: str


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument(
        "--init-unit",
        default="packaging/systemd/hardened-blikvm-system-action-store-init.service",
    )
    parser.add_argument(
        "--inspect-unit",
        default="packaging/systemd/hardened-blikvm-system-action-store-inspect.service",
    )
    parser.add_argument(
        "--tmpfiles",
        default="packaging/tmpfiles.d/hardened-blikvm-system-actions.conf",
    )
    parser.add_argument("--endpoint", required=True)
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument(
        "--output",
        default="evidence/private/device-system-action-maintenance.json",
    )
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
    if path.name != f"hardened-blikvm-system-action-maintenance-{sha256}":
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
        init_unit=validate_config(
            options.init_unit,
            Path("packaging/systemd/hardened-blikvm-system-action-store-init.service"),
        ),
        inspect_unit=validate_config(
            options.inspect_unit,
            Path(
                "packaging/systemd/hardened-blikvm-system-action-store-inspect.service"
            ),
        ),
        tmpfiles=validate_config(
            options.tmpfiles,
            Path("packaging/tmpfiles.d/hardened-blikvm-system-actions.conf"),
        ),
        source_commit=options.source_commit,
    )


def stage_paths(binary_sha256: str, run_id: str) -> Paths:
    if SHA256.fullmatch(binary_sha256) is None or RUN_ID.fullmatch(run_id) is None:
        raise ValueError("staging identity is invalid")
    root = (
        f"/tmp/hardened-blikvm-system-action-maintenance-"
        f"{binary_sha256[:12]}-{run_id}"
    )
    if STAGE_ROOT.fullmatch(root) is None:
        raise ValueError("staging root is invalid")
    marker_value = f"system-action-maintenance:{binary_sha256}:{run_id}"
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
        watchdog=f"hardened-blikvm-system-action-rollback-{run_id}",
    )


def assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def encode_script(script: str) -> str:
    return base64.b64encode(script.encode()).decode("ascii")


def build_preflight_script(paths: Paths, required_bytes: int) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "production_root": PRODUCTION_ROOT,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "binary_part": f"{BINARY_TARGET}.part",
            "init_target": INIT_UNIT_TARGET,
            "init_part": f"{INIT_UNIT_TARGET}.part",
            "inspect_target": INSPECT_UNIT_TARGET,
            "inspect_part": f"{INSPECT_UNIT_TARGET}.part",
        }
    )
    body = r"""
set -eu
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
for tool in awk base64 cat chmod cut df find findmnt grep install mkdir mv readlink rm rmdir sha256sum stat sync systemctl systemd-run systemd-tmpfiles tail tr uname wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit architecture aarch64
emit binary_absent_before yes
emit binary_parent_before "$binary_parent_before"
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
    values = assignments(
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
case "$stage_root" in /tmp/hardened-blikvm-system-action-maintenance-*) ;; *) exit 1 ;; esac
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod 700 "$destination"
""".strip()
    return f"{values}\n{body}"


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
    body = r"""
set -eu
[ -d "$stage_root" ]
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ ! -e "$destination" ]
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod "$mode" "$destination"
""".strip()
    return f"{values}\n{body}"


def build_rollback_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "state_file": paths.state,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
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
        }
    )
    body = r"""
set -eu
case "$stage_root" in /tmp/hardened-blikvm-system-action-maintenance-*) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
parent_before=$(awk -F= '$1 == "parent_before" {print $2}' "$state_file")
case "$parent_before" in present|absent) ;; *) exit 1 ;; esac
systemctl stop hardened-blikvm-system-action-store-init.service 2>/dev/null || true
systemctl stop hardened-blikvm-system-action-store-inspect.service 2>/dev/null || true
if [ -e "$production_root" ]; then
  [ -d "$production_root" ]
  [ ! -L "$production_root" ]
  [ "$(stat -c '%a:%u:%g' "$production_root")" = 700:0:0 ]
  unexpected=$(find "$production_root" -mindepth 1 -maxdepth 1 \
    ! -name journal-a.bin ! -name journal-b.bin -print -quit)
  [ -z "$unexpected" ]
  for copy in journal-a.bin journal-b.bin; do
    if [ -e "$production_root/$copy" ]; then
      [ -f "$production_root/$copy" ]
      [ ! -L "$production_root/$copy" ]
      [ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/$copy")" = 600:0:0:1:2076 ]
      rm -f -- "$production_root/$copy"
    fi
  done
  rmdir "$production_root"
fi
for path_and_size in "$binary_target.part:$binary_size:755" \
  "$init_target.part:$init_size:644" "$inspect_target.part:$inspect_size:644"; do
  path=${path_and_size%%:*}
  remainder=${path_and_size#*:}
  maximum=${remainder%%:*}
  expected_mode=${remainder##*:}
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h' "$path")" = "$expected_mode:0:0:1" ]
    [ "$(stat -c %s "$path")" -le "$maximum" ]
    rm -f -- "$path"
  fi
done
for path_and_hash in "$init_target:$init_sha" "$inspect_target:$inspect_sha"; do
  path=${path_and_hash%:*}
  expected=${path_and_hash##*:}
  if [ -e "$path" ]; then
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(sha256sum "$path" | cut -d ' ' -f 1)" = "$expected" ]
    rm -f -- "$path"
  fi
done
systemctl daemon-reload
if [ -e "$binary_target" ]; then
  [ -f "$binary_target" ]
  [ ! -L "$binary_target" ]
  [ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
  rm -f -- "$binary_target"
fi
if [ "$parent_before" = absent ]; then
  rmdir "$binary_parent"
fi
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${INVOCATION_ID:-}" ]; then
  systemctl stop "$watchdog.service" 2>/dev/null || true
fi
rm -f -- "$state_file" "$stage_root/rollback.sh"
rm -f -- "$stage_root/maintenance" "$stage_root/init.service" \
  "$stage_root/inspect.service" "$stage_root/tmpfiles.conf" "$marker"
rmdir "$stage_root"
""".strip()
    return f"{values}\n{body}"


def build_run_script(paths: Paths, assets: Assets) -> str:
    rollback_b64 = encode_script(build_rollback_script(paths, assets))
    values = assignments(
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
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "binary_parent": BINARY_PARENT,
            "binary_target": BINARY_TARGET,
            "init_target": INIT_UNIT_TARGET,
            "inspect_target": INSPECT_UNIT_TARGET,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(stat -c %s "$binary")" = "$binary_size" ]
[ "$(sha256sum "$binary" | cut -d ' ' -f 1)" = "$binary_sha" ]
[ "$(sha256sum "$init_source" | cut -d ' ' -f 1)" = "$init_sha" ]
[ "$(sha256sum "$inspect_source" | cut -d ' ' -f 1)" = "$inspect_sha" ]
[ "$(sha256sum "$tmpfiles_source" | cut -d ' ' -f 1)" = "$tmpfiles_sha" ]
if [ -e "$binary_parent" ]; then parent_before=present; else parent_before=absent; fi
printf 'parent_before=%s\n' "$parent_before" > "$rollback_state"
chmod 600 "$rollback_state"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
systemd-run --quiet --unit="$watchdog" --on-active=10min \
  --timer-property=AccuracySec=1s /bin/sh "$rollback_script"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
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
[ "$(sha256sum "$init_target" | cut -d ' ' -f 1)" = "$init_sha" ]
[ "$(sha256sum "$inspect_target" | cut -d ' ' -f 1)" = "$inspect_sha" ]
systemctl daemon-reload
systemd-tmpfiles --create "$tmpfiles_source"
[ -d "$production_root" ]
[ "$(stat -c '%a:%u:%g' "$production_root")" = 700:0:0 ]
systemctl start hardened-blikvm-system-action-store-init.service
[ "$(systemctl show -p Result --value hardened-blikvm-system-action-store-init.service)" = success ]
[ "$(systemctl show -p ExecMainStatus --value hardened-blikvm-system-action-store-init.service)" = 0 ]
[ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/journal-a.bin")" = 600:0:0:1:2076 ]
[ "$(stat -c '%a:%u:%g:%h:%s' "$production_root/journal-b.bin")" = 600:0:0:1:2076 ]
copy_a_sha=$(sha256sum "$production_root/journal-a.bin" | cut -d ' ' -f 1)
copy_b_sha=$(sha256sum "$production_root/journal-b.bin" | cut -d ' ' -f 1)
[ "$copy_a_sha" = "$copy_b_sha" ]
systemctl start hardened-blikvm-system-action-store-inspect.service
[ "$(systemctl show -p Result --value hardened-blikvm-system-action-store-inspect.service)" = success ]
[ "$(systemctl show -p ExecMainStatus --value hardened-blikvm-system-action-store-inspect.service)" = 0 ]
/bin/sh "$rollback_script"
[ ! -e "$production_root" ]
[ ! -e "$binary_target" ]
[ ! -e "$init_target" ]
[ ! -e "$inspect_target" ]
emit binary_hash_verified yes
emit copies_equal yes
emit copies_mode_verified yes
emit empty_generation 1
emit init_result success
emit inspect_result success
emit rollback_confirmed yes
emit sandbox_sources_verified yes
emit watchdog_armed yes
""".strip()
    return f"{values}\n{body}"


def build_cleanup_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "rollback_b64": encode_script(build_rollback_script(paths, assets)),
        }
    )
    body = r"""
set -eu
if [ -d "$stage_root" ]; then
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  if [ ! -x "$rollback_script" ]; then
    printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
    chmod 700 "$rollback_script"
  fi
  if [ ! -f "$rollback_state" ]; then
    if [ -d /usr/libexec/hardened-blikvm ]; then parent_before=present; else parent_before=absent; fi
    printf 'parent_before=%s\n' "$parent_before" > "$rollback_state"
    chmod 600 "$rollback_state"
  fi
  /bin/sh "$rollback_script"
fi
[ ! -e "$stage_root" ]
printf 'cleanup_restored=yes\n'
""".strip()
    return f"{values}\n{body}"


def build_final_script(paths: Paths, binary_parent_before: str) -> str:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    values = assignments(
        {
            "stage_root": paths.root,
            "watchdog": paths.watchdog,
            "production_root": PRODUCTION_ROOT,
            "binary_target": BINARY_TARGET,
            "binary_part": f"{BINARY_TARGET}.part",
            "binary_parent": BINARY_PARENT,
            "binary_parent_before": binary_parent_before,
            "init_target": INIT_UNIT_TARGET,
            "init_part": f"{INIT_UNIT_TARGET}.part",
            "inspect_target": INSPECT_UNIT_TARGET,
            "inspect_part": f"{INSPECT_UNIT_TARGET}.part",
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$production_root" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
[ ! -e "$init_target" ]
[ ! -e "$init_part" ]
[ ! -e "$inspect_target" ]
[ ! -e "$inspect_part" ]
if [ "$binary_parent_before" = present ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_final=present
else
  [ ! -e "$binary_parent" ]
  binary_parent_final=absent
fi
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit binary_absent_final yes
emit binary_parent_final "$binary_parent_final"
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


def require_result(stage: str, result: subprocess.CompletedProcess[bytes]) -> bytes:
    if result.returncode != 0 or len(result.stdout) > 16 * 1024:
        raise RuntimeError(stage)
    return result.stdout


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "architecture": "aarch64",
        "binary_absent_before": "yes",
        "binary_parent_before": facts.get("binary_parent_before", ""),
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
    if expected["binary_parent_before"] not in {"present", "absent"}:
        raise ValueError("binary parent state is invalid")
    if facts != dict(sorted(expected.items())):
        raise ValueError("preflight invariant failed")


def validate_run(facts: dict[str, str]) -> None:
    expected = {
        "binary_hash_verified": "yes",
        "copies_equal": "yes",
        "copies_mode_verified": "yes",
        "empty_generation": "1",
        "init_result": "success",
        "inspect_result": "success",
        "rollback_confirmed": "yes",
        "sandbox_sources_verified": "yes",
        "watchdog_armed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def validate_final(facts: dict[str, str], binary_parent_before: str) -> None:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    expected = {
        "binary_absent_final": "yes",
        "binary_parent_final": binary_parent_before,
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
                target, secret, build_cleanup_script(paths, assets), timeout=30
            ),
        )
        facts = native.parse_facts(output, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_restored": "yes"}}
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


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
                    timeout=20,
                ),
            ),
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        upload(target, secret, assets.binary, build_first_upload_script(paths))
        upload(
            target,
            secret,
            assets.init_unit,
            build_upload_script(paths, paths.init_unit, "600"),
        )
        upload(
            target,
            secret,
            assets.inspect_unit,
            build_upload_script(paths, paths.inspect_unit, "600"),
        )
        upload(
            target,
            secret,
            assets.tmpfiles,
            build_upload_script(paths, paths.tmpfiles, "600"),
        )

        stage = "provision-and-rollback"
        checks = native.parse_facts(
            require_result(
                stage,
                prototype.run_ssh(
                    target, secret, build_run_script(paths, assets), timeout=120
                ),
            ),
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
                    timeout=20,
                ),
            ),
            FINAL_KEYS,
        )
        validate_final(final_state, preflight["binary_parent_before"])
        cleanup_required = False

        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-system-action-maintenance",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "artifacts": {
                "binary": {"sha256": assets.binary.sha256, "sizeBytes": assets.binary.size_bytes},
                "initUnitSha256": assets.init_unit.sha256,
                "inspectUnitSha256": assets.inspect_unit.sha256,
                "tmpfilesSha256": assets.tmpfiles.sha256,
            },
            "policy": {
                "commands": ["initialize-empty", "inspect-empty"],
                "deviceActionsExecuted": False,
                "externalFloorAccepted": False,
                "persistentStateRestored": True,
                "rebootOrShutdownAvailable": False,
                "rollbackWatchdog": "ten-minute-systemd",
            },
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
            "probe": "hardened-blikvm-system-action-maintenance",
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
