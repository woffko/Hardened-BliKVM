#!/usr/bin/env python3
"""Retain one exact inert runtime .deb under crash-safe rollback guards."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
from pathlib import Path, PurePosixPath
import re
import secrets
import shlex
import subprocess
import sys
import tarfile
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as debian
from tools import stage_device_debian_runtime_package as package_gate
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


PREFLIGHT_KEYS = {
    "accounts_absent_before",
    "architecture",
    "capacity_ready",
    "dpkg_audit_clean",
    "gates_absent_before",
    "inodes_ready",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "package_absent_before",
    "ports_free_before",
    "production_paths_absent_before",
    "rootfs_mode_before",
    "tools_ready",
}
PREPARE_KEYS = {
    "account_databases_unchanged",
    "accounts_absent_after_install",
    "boot_rollback_armed",
    "capabilities_absent",
    "dpkg_audit_clean_after_install",
    "installed_hashes_verified",
    "installed_modes_verified",
    "installed_tree_exact",
    "nginx_tree_unchanged",
    "other_packages_unchanged",
    "package_prepared",
    "persistent_runtime_roots_absent",
    "ports_free_after_install",
    "unit_templates_loaded_disabled_inactive",
    "watchdog_active",
}
COMMIT_KEYS = {
    "account_databases_unchanged",
    "boot_rollback_removed",
    "dpkg_audit_clean_after_commit",
    "nginx_tree_unchanged",
    "other_packages_unchanged",
    "package_retained",
    "stage_removed",
    "watchdog_removed",
}
FINAL_KEYS = {
    "accounts_absent_final",
    "capabilities_absent_final",
    "dpkg_audit_clean_final",
    "gates_absent_final",
    "installed_hashes_verified_final",
    "installed_modes_verified_final",
    "installed_tree_exact_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "package_installed_final",
    "persistent_runtime_roots_absent_final",
    "ports_free_final",
    "rootfs_mode_final",
    "unit_templates_loaded_disabled_inactive_final",
}
CLEANUP_KEYS = {"cleanup_removed"}
ALLOWED_FACT_VALUES = {"yes", "active", "aarch64", "rw"}
EXECUTABLE_PATHS = tuple(
    "/" + path for path in sorted(debian.EXECUTABLE_DESTINATIONS)
)
DEDICATED_ROOTS = (
    PurePosixPath("/usr/libexec/hardened-blikvm"),
    PurePosixPath("/usr/share/hardened-blikvm"),
    PurePosixPath("/usr/share/doc/hardened-blikvm-runtime"),
)


@dataclass(frozen=True)
class RemotePaths:
    run_id: str
    stage_root: str
    stage_marker: str
    package: str
    install_log: str
    rollback_root: str
    rollback_marker: str
    rollback_state: str
    rollback_script: str
    watchdog_script: str
    boot_unit: str
    boot_unit_path: str
    boot_unit_part: str
    boot_unit_link: str
    watchdog_unit: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    package: str
    expected_sha256: str
    expected_source_commit: str
    expected_source_bundle_sha256: str
    expected_builder_commit: str
    output: str


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", default="evidence/private/ssh-known-hosts")
    parser.add_argument("--package", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-source-commit", required=True)
    parser.add_argument("--expected-source-bundle-sha256", required=True)
    parser.add_argument("--expected-builder-commit", required=True)
    parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        package=cast(str, namespace.package),
        expected_sha256=cast(str, namespace.expected_sha256),
        expected_source_commit=cast(str, namespace.expected_source_commit),
        expected_source_bundle_sha256=cast(
            str, namespace.expected_source_bundle_sha256
        ),
        expected_builder_commit=cast(str, namespace.expected_builder_commit),
        output=cast(str, namespace.output),
    )


def remote_paths(package_sha256: str, run_id: str) -> RemotePaths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("retained-package stage identity is invalid")
    short_hash = package_sha256[:12]
    stage_root = f"/tmp/hardened-blikvm-retain-{short_hash}-{run_id}"
    rollback_root = (
        f"/var/lib/hardened-blikvm-retain-rollback-{short_hash}-{run_id}"
    )
    boot_unit = f"hardened-blikvm-retain-rollback-{run_id}.service"
    return RemotePaths(
        run_id=run_id,
        stage_root=stage_root,
        stage_marker=f"{stage_root}/.owner",
        package=f"{stage_root}/runtime.deb",
        install_log=f"{stage_root}/install.log",
        rollback_root=rollback_root,
        rollback_marker=f"{rollback_root}/.owner",
        rollback_state=f"{rollback_root}/state",
        rollback_script=f"{rollback_root}/rollback.sh",
        watchdog_script=f"{rollback_root}/watchdog.sh",
        boot_unit=boot_unit,
        boot_unit_path=f"/etc/systemd/system/{boot_unit}",
        boot_unit_part=f"/etc/systemd/system/{boot_unit}.part",
        boot_unit_link=f"/etc/systemd/system/sysinit.target.wants/{boot_unit}",
        watchdog_unit=f"hardened-blikvm-retain-watchdog-{run_id}.service",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


PORT_FUNCTION = package_gate.PORT_FUNCTION

STATE_FUNCTIONS = r"""
package_any_state() {
  dpkg-query -W "$package_name" >/dev/null 2>&1
}
package_installed() {
  dpkg-query -W -f='${db:Status-Abbrev}' "$package_name" 2>/dev/null | grep -q '^ii '
}
package_identity_exact() {
  [ "$(dpkg-query -W -f='${Version}:${Architecture}' "$package_name" 2>/dev/null || true)" = "$package_version:$package_architecture" ]
}
accounts_absent() {
  for account in hardened-blikvm-server hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-control hardened-blikvm-update; do
    if getent passwd "$account" >/dev/null 2>&1; then return 1; fi
  done
  if getent group hardened-blikvm >/dev/null 2>&1; then return 1; fi
  return 0
}
production_paths_absent() {
  for path in /usr/libexec/hardened-blikvm /usr/share/hardened-blikvm /usr/share/doc/hardened-blikvm-runtime \
    /lib/systemd/system/hardened-blikvm-platform-broker.service \
    /lib/systemd/system/hardened-blikvm-video-proxy.service \
    /lib/systemd/system/hardened-blikvm-control-broker.service \
    /lib/systemd/system/hardened-blikvm-server.service; do
    [ ! -e "$path" ] || return 1
  done
}
persistent_roots_absent() {
  for path in /etc/hardened-blikvm /var/lib/hardened-blikvm /var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update; do
    [ ! -e "$path" ] || return 1
  done
}
units_absent() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service \
    hardened-blikvm-control-broker.service hardened-blikvm-server.service; do
    [ "$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)" = not-found ] || return 1
  done
}
units_loaded_disabled_inactive() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service \
    hardened-blikvm-control-broker.service hardened-blikvm-server.service; do
    [ "$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)" = loaded ] || return 1
    [ "$(systemctl is-enabled "$unit" 2>/dev/null || true)" = disabled ] || return 1
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
  done
}
other_package_hash() {
  dpkg-query -W -f='${binary:Package}\t${Version}\t${Architecture}\t${db:Status-Abbrev}\n' 2>/dev/null | \
    awk -F '\t' -v package="$package_name" '$1 != package' | sort | sha256sum | awk '{print $1}'
}
account_database_hash() {
  sha256sum /etc/passwd /etc/group /etc/shadow /etc/gshadow | sha256sum | awk '{print $1}'
}
nginx_tree_hash() {
  find /etc/nginx -xdev -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}'
}
""".strip()


def _dedicated_tree(package: package_gate.PackageInfo) -> tuple[str, list[str]]:
    entries: set[str] = set()
    directories: set[str] = set()
    for path_value in package.installed_files:
        path = PurePosixPath(path_value)
        root = next((item for item in DEDICATED_ROOTS if item == path or item in path.parents), None)
        if root is None:
            continue
        entries.add(f"f {path}")
        parent = path.parent
        while parent == root or root in parent.parents:
            directories.add(str(parent))
            if parent == root:
                break
            parent = parent.parent
    if not directories or not entries:
        raise ValueError("package dedicated tree is incomplete")
    entries.update(f"d {path}" for path in directories)
    listing = "".join(f"{entry}\n" for entry in sorted(entries))
    return hashlib.sha256(listing.encode("utf-8")).hexdigest(), sorted(directories)


def _installed_checks(package: package_gate.PackageInfo) -> str:
    lines: list[str] = []
    for path, (digest, mode) in sorted(package.installed_files.items()):
        quoted = shlex.quote(path)
        lines.append(
            f'[ "$(sha256sum {quoted} | awk \'{{print $1}}\')" = "{digest}" ]'
        )
        lines.append(
            f'[ "$(stat -c \'%a:%u:%g:%h\' {quoted})" = "{mode:o}:0:0:1" ]'
        )
    _, directories = _dedicated_tree(package)
    for path in directories:
        lines.append(
            f'[ "$(stat -c \'%a:%u:%g\' {shlex.quote(path)})" = "755:0:0" ]'
        )
    return "\n".join(lines)


def _capability_checks() -> str:
    return "\n".join(
        f'[ -z "$(getcap {shlex.quote(path)})" ]' for path in EXECUTABLE_PATHS
    )


def build_boot_unit(paths: RemotePaths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM retained-package emergency rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=kvmd.service kvmd-nginx.service multi-user.target",
            f"ConditionPathExists={paths.rollback_marker}",
            "",
            "[Service]",
            "Type=oneshot",
            "User=root",
            "Group=root",
            "UMask=0077",
            "NoNewPrivileges=yes",
            "PrivateDevices=yes",
            "PrivateNetwork=yes",
            "ProtectHome=yes",
            "ProtectKernelLogs=yes",
            "ProtectKernelModules=yes",
            "ProtectKernelTunables=yes",
            "RestrictAddressFamilies=AF_UNIX",
            "RestrictNamespaces=yes",
            f"ExecStart=/bin/sh {paths.rollback_script}",
            "TimeoutStartSec=5min",
            "",
            "[Install]",
            "WantedBy=sysinit.target",
            "",
        )
    )


def build_preflight_script(
    paths: RemotePaths, package: package_gate.PackageInfo
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "rollback_root": paths.rollback_root,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "required_bytes": package.size_bytes * 4 + 32 * 1024 * 1024,
            "required_inodes": len(package.installed_files) * 4 + 1024,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=identity
on_exit() {
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
phase=tools
for tool in awk base64 cat chmod df dpkg dpkg-deb dpkg-query find findmnt getcap getent grep ln mkdir mv pgrep readlink rm rmdir sha256sum sort stat sync systemctl systemd-run tail tr wc xargs; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
phase=package-manager-idle
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
phase=dpkg-audit
[ -z "$(dpkg --audit 2>/dev/null)" ]
phase=rootfs
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=capacity
for path in /usr /var /tmp; do
  available=$(df -B1 --output=avail "$path" | tail -n 1 | tr -d ' ')
  [ "$available" -ge "$required_bytes" ]
done
phase=inodes
for path in /usr /var /tmp; do
  available=$(df --output=iavail "$path" | tail -n 1 | tr -d ' ')
  [ "$available" -ge "$required_inodes" ]
done
phase=kvmd
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
phase=gates
[ ! -e "$stage_root" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /tmp -maxdepth 1 -name 'hardened-blikvm-retain-*' -print -quit)" ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-retain-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-retain-*' -print -quit)" ]
systemctl list-units --all --plain --no-legend 2>/dev/null | \
  awk '$1 ~ /^hardened-blikvm-retain-(rollback|watchdog)-[0-9a-f]+\.service$/ {found=1} END {exit found}'
phase=package
package_any_state && exit 1
phase=accounts
accounts_absent
phase=production-paths
production_paths_absent
persistent_roots_absent
phase=units
units_absent
phase=ports
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
phase=htpasswd
[ -f /etc/kvmd/htpasswd ]
[ ! -L /etc/kvmd/htpasswd ]
[ "$(stat -c '%a' /etc/kvmd/htpasswd)" = 600 ]
phase=complete
emit architecture aarch64
emit tools_ready yes
emit capacity_ready yes
emit inodes_ready yes
emit rootfs_mode_before "$rootfs_mode"
emit package_absent_before yes
emit production_paths_absent_before yes
emit accounts_absent_before yes
emit gates_absent_before yes
emit dpkg_audit_clean yes
emit ports_free_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def build_upload_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "stage_marker": paths.stage_marker,
            "marker_value": f"{paths.run_id}:{package.sha256}",
            "package_file": paths.package,
            "expected_sha256": package.sha256,
            "expected_size": package.size_bytes,
        }
    )
    body = r"""
set -eu
umask 077
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$marker_value" > "$stage_marker"
chmod 600 "$stage_marker"
cat > "$package_file.part"
[ "$(wc -c < "$package_file.part" | tr -d ' ')" = "$expected_size" ]
[ "$(sha256sum "$package_file.part" | awk '{print $1}')" = "$expected_sha256" ]
chmod 600 "$package_file.part"
mv "$package_file.part" "$package_file"
""".strip()
    return f"{values}\n{body}"


def build_rollback_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    boot_unit = build_boot_unit(paths)
    boot_unit_sha256 = hashlib.sha256(boot_unit.encode()).hexdigest()
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "stage_marker": paths.stage_marker,
            "rollback_root": paths.rollback_root,
            "rollback_marker": paths.rollback_marker,
            "rollback_state": paths.rollback_state,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_link": paths.boot_unit_link,
            "boot_unit": paths.boot_unit,
            "boot_unit_sha256": boot_unit_sha256,
            "watchdog_unit": paths.watchdog_unit,
            "marker_value": f"{paths.run_id}:{package.sha256}",
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
[ -f "$rollback_marker" ]
[ ! -L "$rollback_marker" ]
[ "$(cat "$rollback_marker")" = "$marker_value" ]
if [ -e "$boot_unit_path" ]; then
  [ -f "$boot_unit_path" ]
  [ ! -L "$boot_unit_path" ]
  [ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
fi
if package_any_state; then
  package_identity_exact
  dpkg --purge --force-remove-reinstreq "$package_name" >/dev/null 2>&1
fi
systemctl daemon-reload >/dev/null 2>&1
! package_any_state
production_paths_absent
persistent_roots_absent
accounts_absent
units_absent
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ]
  [ ! -L "$stage_marker" ]
  [ "$(cat "$stage_marker")" = "$marker_value" ]
  unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 \
    ! -name .owner ! -name runtime.deb ! -name runtime.deb.part ! -name install.log -print -quit)
  [ -z "$unexpected" ]
  rm -f -- "$stage_root/runtime.deb" "$stage_root/runtime.deb.part" "$stage_root/install.log" "$stage_marker"
  rmdir "$stage_root"
fi
if [ -e "$boot_unit_link" ]; then
  [ -L "$boot_unit_link" ]
  [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
  rm -f -- "$boot_unit_link"
fi
rm -f -- "$boot_unit_path"
unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
  ! -name .owner ! -name state ! -name rollback.sh ! -name watchdog.sh -print -quit)
[ -z "$unexpected" ]
rm -f -- "$rollback_state" "$watchdog_script" "$rollback_script" "$rollback_marker"
rmdir "$rollback_root"
systemctl daemon-reload >/dev/null 2>&1
""".strip()
    return f"#!/bin/sh\n{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}\n"


def _prepare_material(paths: RemotePaths, package: package_gate.PackageInfo) -> tuple[str, str, str]:
    boot_unit = build_boot_unit(paths)
    rollback = build_rollback_script(paths, package)
    watchdog = f"#!/bin/sh\nset -eu\nsleep 300\nexec /bin/sh {shlex.quote(paths.rollback_script)}\n"
    return boot_unit, rollback, watchdog


def build_prepare_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    boot_unit, rollback, watchdog = _prepare_material(paths, package)
    tree_sha256, _ = _dedicated_tree(package)
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "stage_marker": paths.stage_marker,
            "package_file": paths.package,
            "install_log": paths.install_log,
            "rollback_root": paths.rollback_root,
            "rollback_marker": paths.rollback_marker,
            "rollback_state": paths.rollback_state,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "marker_value": f"{paths.run_id}:{package.sha256}",
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "expected_sha256": package.sha256,
            "expected_tree_sha256": tree_sha256,
            "boot_unit_b64": base64.b64encode(boot_unit.encode()).decode("ascii"),
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
            "rollback_b64": base64.b64encode(rollback.encode()).decode("ascii"),
            "watchdog_b64": base64.b64encode(watchdog.encode()).decode("ascii"),
        }
    )
    checks = _installed_checks(package)
    capability_checks = _capability_checks()
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=validate-stage
on_exit() {
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ -f "$stage_marker" ]
[ ! -L "$stage_marker" ]
[ "$(cat "$stage_marker")" = "$marker_value" ]
[ "$(sha256sum "$package_file" | awk '{print $1}')" = "$expected_sha256" ]
account_hash_before=$(account_database_hash)
nginx_hash_before=$(nginx_tree_hash)
htpasswd_hash_before=$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')
other_package_hash_before=$(other_package_hash)
phase=create-boot-rollback
[ ! -e "$rollback_root" ]
mkdir -m 700 "$rollback_root"
printf '%s\n' "$marker_value" > "$rollback_marker"
chmod 600 "$rollback_marker"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
printf '%s' "$watchdog_b64" | base64 -d > "$watchdog_script"
chmod 700 "$rollback_script" "$watchdog_script"
{
  printf 'run_id=%s\n' "${marker_value%%:*}"
  printf 'package_sha=%s\n' "$expected_sha256"
  printf 'account_hash=%s\n' "$account_hash_before"
  printf 'nginx_hash=%s\n' "$nginx_hash_before"
  printf 'htpasswd_hash=%s\n' "$htpasswd_hash_before"
  printf 'other_package_hash=%s\n' "$other_package_hash_before"
} > "$rollback_state.part"
chmod 600 "$rollback_state.part"
mv "$rollback_state.part" "$rollback_state"
printf '%s' "$boot_unit_b64" | base64 -d > "$boot_unit_part"
[ "$(sha256sum "$boot_unit_part" | awk '{print $1}')" = "$boot_unit_sha256" ]
chmod 644 "$boot_unit_part"
mv "$boot_unit_part" "$boot_unit_path"
[ -d /etc/systemd/system/sysinit.target.wants ]
ln -s "../$boot_unit" "$boot_unit_link"
systemctl daemon-reload
[ "$(systemctl is-enabled "$boot_unit" 2>/dev/null || true)" = enabled ]
sync
phase=start-watchdog
systemd-run --quiet --collect --unit="$watchdog_unit" \
  --property=NoNewPrivileges=yes --property=PrivateDevices=yes \
  --property=PrivateNetwork=yes --property=ProtectHome=yes \
  --property=ProtectKernelLogs=yes --property=ProtectKernelModules=yes \
  --property=ProtectKernelTunables=yes --property=RestrictAddressFamilies=AF_UNIX \
  /bin/sh "$watchdog_script"
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" != active ]; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
phase=package-manager-idle
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
phase=install
dpkg -i "$package_file" > "$install_log" 2>&1
systemctl daemon-reload
phase=package-identity
package_installed
package_identity_exact
[ -z "$(dpkg --audit 2>/dev/null)" ]
phase=installed-files
""".strip()
    tail = r"""
phase=installed-tree
actual_tree_sha256=$(find /usr/libexec/hardened-blikvm /usr/share/hardened-blikvm \
  /usr/share/doc/hardened-blikvm-runtime -xdev -printf '%y %p\n' | sort | sha256sum | awk '{print $1}')
[ "$actual_tree_sha256" = "$expected_tree_sha256" ]
phase=capabilities
CAPABILITY_CHECKS
phase=units
units_loaded_disabled_inactive
phase=unchanged-state
accounts_absent
persistent_roots_absent
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ "$(account_database_hash)" = "$account_hash_before" ]
[ "$(nginx_tree_hash)" = "$nginx_hash_before" ]
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$htpasswd_hash_before" ]
[ "$(other_package_hash)" = "$other_package_hash_before" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
[ "$(systemctl is-enabled "$boot_unit" 2>/dev/null || true)" = enabled ]
sync
phase=complete
emit package_prepared yes
emit installed_hashes_verified yes
emit installed_modes_verified yes
emit installed_tree_exact yes
emit capabilities_absent yes
emit accounts_absent_after_install yes
emit unit_templates_loaded_disabled_inactive yes
emit persistent_runtime_roots_absent yes
emit account_databases_unchanged yes
emit nginx_tree_unchanged yes
emit other_packages_unchanged yes
emit dpkg_audit_clean_after_install yes
emit boot_rollback_armed yes
emit watchdog_active yes
emit ports_free_after_install yes
""".strip().replace("CAPABILITY_CHECKS", capability_checks)
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}\n{checks}\n{tail}"


def _state_validation() -> str:
    return r"""
[ -f "$rollback_state" ]
[ ! -L "$rollback_state" ]
[ "$(stat -c '%a:%u:%g:%h' "$rollback_state")" = 600:0:0:1 ]
[ "$(wc -l < "$rollback_state" | tr -d ' ')" = 6 ]
state_value() {
  awk -F= -v key="$1" '$1 == key {value=$2; count++} END {if (count == 1) print value; else exit 1}' "$rollback_state"
}
state_run_id=$(state_value run_id)
state_package_sha=$(state_value package_sha)
account_hash_before=$(state_value account_hash)
nginx_hash_before=$(state_value nginx_hash)
htpasswd_hash_before=$(state_value htpasswd_hash)
other_package_hash_before=$(state_value other_package_hash)
[ "$state_run_id:$state_package_sha" = "$marker_value" ]
for digest in "$account_hash_before" "$nginx_hash_before" "$htpasswd_hash_before" "$other_package_hash_before"; do
  case "$digest" in *[!0-9a-f]*|'') exit 1 ;; esac
  [ "${#digest}" = 64 ]
done
""".strip()


def build_commit_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    boot_unit = build_boot_unit(paths)
    tree_sha256, _ = _dedicated_tree(package)
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "stage_marker": paths.stage_marker,
            "package_file": paths.package,
            "install_log": paths.install_log,
            "rollback_root": paths.rollback_root,
            "rollback_marker": paths.rollback_marker,
            "rollback_state": paths.rollback_state,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "marker_value": f"{paths.run_id}:{package.sha256}",
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "expected_sha256": package.sha256,
            "expected_tree_sha256": tree_sha256,
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        }
    )
    checks = _installed_checks(package)
    capability_checks = _capability_checks()
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=validate-rollback
on_exit() {
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ -f "$stage_marker" ]
[ "$(cat "$stage_marker")" = "$marker_value" ]
[ "$(sha256sum "$package_file" | awk '{print $1}')" = "$expected_sha256" ]
[ -f "$rollback_marker" ]
[ "$(cat "$rollback_marker")" = "$marker_value" ]
[ -f "$boot_unit_path" ]
[ ! -L "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
[ -L "$boot_unit_link" ]
[ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
STATE_VALIDATION
phase=revalidate-package
package_installed
package_identity_exact
[ -z "$(dpkg --audit 2>/dev/null)" ]
""".strip().replace("STATE_VALIDATION", _state_validation())
    tail = r"""
actual_tree_sha256=$(find /usr/libexec/hardened-blikvm /usr/share/hardened-blikvm \
  /usr/share/doc/hardened-blikvm-runtime -xdev -printf '%y %p\n' | sort | sha256sum | awk '{print $1}')
[ "$actual_tree_sha256" = "$expected_tree_sha256" ]
CAPABILITY_CHECKS
units_loaded_disabled_inactive
accounts_absent
persistent_roots_absent
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ "$(account_database_hash)" = "$account_hash_before" ]
[ "$(nginx_tree_hash)" = "$nginx_hash_before" ]
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$htpasswd_hash_before" ]
[ "$(other_package_hash)" = "$other_package_hash_before" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
phase=remove-stage
unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 \
  ! -name .owner ! -name runtime.deb ! -name install.log -print -quit)
[ -z "$unexpected" ]
rm -f -- "$package_file" "$install_log" "$stage_marker"
rmdir "$stage_root"
phase=stop-watchdog
systemctl stop "$watchdog_unit" >/dev/null 2>&1
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
phase=remove-boot-rollback
rm -f -- "$boot_unit_link"
rm -f -- "$boot_unit_path"
unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
  ! -name .owner ! -name state ! -name rollback.sh ! -name watchdog.sh -print -quit)
[ -z "$unexpected" ]
rm -f -- "$rollback_state" "$watchdog_script" "$rollback_script" "$rollback_marker"
rmdir "$rollback_root"
systemctl daemon-reload
phase=finalize
package_installed
package_identity_exact
units_loaded_disabled_inactive
[ ! -e "$stage_root" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ -z "$(dpkg --audit 2>/dev/null)" ]
emit package_retained yes
emit stage_removed yes
emit watchdog_removed yes
emit boot_rollback_removed yes
emit account_databases_unchanged yes
emit nginx_tree_unchanged yes
emit other_packages_unchanged yes
emit dpkg_audit_clean_after_commit yes
""".strip().replace("CAPABILITY_CHECKS", capability_checks)
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}\n{checks}\n{tail}"


def build_cleanup_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    boot_unit = build_boot_unit(paths)
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "stage_marker": paths.stage_marker,
            "rollback_root": paths.rollback_root,
            "rollback_marker": paths.rollback_marker,
            "rollback_state": paths.rollback_state,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "marker_value": f"{paths.run_id}:{package.sha256}",
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
if package_any_state; then
  package_identity_exact
  dpkg --purge --force-remove-reinstreq "$package_name" >/dev/null 2>&1
fi
systemctl daemon-reload >/dev/null 2>&1
if [ -e "$stage_root" ]; then
  [ -d "$stage_root" ]
  [ ! -L "$stage_root" ]
  [ "$(stat -c '%a:%u:%g' "$stage_root")" = 700:0:0 ]
  if [ -e "$stage_marker" ]; then
    [ -f "$stage_marker" ]
    [ ! -L "$stage_marker" ]
    [ "$(cat "$stage_marker")" = "$marker_value" ]
    unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 \
      ! -name .owner ! -name runtime.deb ! -name runtime.deb.part ! -name install.log -print -quit)
    [ -z "$unexpected" ]
    rm -f -- "$stage_root/runtime.deb" "$stage_root/runtime.deb.part" "$stage_root/install.log" "$stage_marker"
  else
    [ -z "$(find "$stage_root" -mindepth 1 -print -quit)" ]
  fi
  rmdir "$stage_root"
fi
if [ -e "$rollback_root" ]; then
  [ -d "$rollback_root" ]
  [ ! -L "$rollback_root" ]
  [ "$(stat -c '%a:%u:%g' "$rollback_root")" = 700:0:0 ]
  if [ -e "$rollback_marker" ]; then
    [ -f "$rollback_marker" ]
    [ ! -L "$rollback_marker" ]
    [ "$(cat "$rollback_marker")" = "$marker_value" ]
    unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
      ! -name .owner ! -name state ! -name state.part ! -name rollback.sh ! -name watchdog.sh -print -quit)
    [ -z "$unexpected" ]
  else
    [ -z "$(find "$rollback_root" -mindepth 1 -print -quit)" ]
  fi
fi
if [ -e "$boot_unit_link" ]; then
  [ -L "$boot_unit_link" ]
  [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
  rm -f -- "$boot_unit_link"
fi
if [ -e "$boot_unit_path" ]; then
  [ -f "$boot_unit_path" ]
  [ ! -L "$boot_unit_path" ]
  [ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
  rm -f -- "$boot_unit_path"
fi
if [ -e "$boot_unit_part" ]; then
  [ -f "$boot_unit_part" ]
  [ ! -L "$boot_unit_part" ]
  [ "$(stat -c '%a:%u:%g:%h' "$boot_unit_part")" = 600:0:0:1 ]
  [ "$(stat -c '%s' "$boot_unit_part")" -le 4096 ]
  rm -f -- "$boot_unit_part"
fi
if [ -e "$rollback_root" ]; then
  rm -f -- "$rollback_state" "$rollback_root/state.part" "$watchdog_script" "$rollback_script" "$rollback_marker"
  rmdir "$rollback_root"
fi
systemctl daemon-reload >/dev/null 2>&1
! package_any_state
production_paths_absent
persistent_roots_absent
accounts_absent
units_absent
[ ! -e "$stage_root" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ -z "$(dpkg --audit 2>/dev/null)" ]
emit cleanup_removed yes
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def build_final_script(paths: RemotePaths, package: package_gate.PackageInfo) -> str:
    tree_sha256, _ = _dedicated_tree(package)
    values = _assignments(
        {
            "stage_root": paths.stage_root,
            "rollback_root": paths.rollback_root,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "expected_tree_sha256": tree_sha256,
        }
    )
    checks = _installed_checks(package)
    capability_checks = _capability_checks()
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
package_installed
package_identity_exact
""".strip()
    tail = r"""
actual_tree_sha256=$(find /usr/libexec/hardened-blikvm /usr/share/hardened-blikvm \
  /usr/share/doc/hardened-blikvm-runtime -xdev -printf '%y %p\n' | sort | sha256sum | awk '{print $1}')
[ "$actual_tree_sha256" = "$expected_tree_sha256" ]
CAPABILITY_CHECKS
units_loaded_disabled_inactive
accounts_absent
persistent_roots_absent
[ ! -e "$stage_root" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /tmp -maxdepth 1 -name 'hardened-blikvm-retain-*' -print -quit)" ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-retain-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-retain-*' -print -quit)" ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ -z "$(dpkg --audit 2>/dev/null)" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit package_installed_final yes
emit installed_hashes_verified_final yes
emit installed_modes_verified_final yes
emit installed_tree_exact_final yes
emit capabilities_absent_final yes
emit unit_templates_loaded_disabled_inactive_final yes
emit accounts_absent_final yes
emit persistent_runtime_roots_absent_final yes
emit gates_absent_final yes
emit ports_free_final yes
emit dpkg_audit_clean_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit rootfs_mode_final "$rootfs_mode"
""".strip().replace("CAPABILITY_CHECKS", capability_checks)
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}\n{checks}\n{tail}"


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("retained-package gate output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in ALLOWED_FACT_VALUES
        ):
            raise ValueError("retained-package gate returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("retained-package gate result was incomplete")
    return dict(sorted(facts.items()))


def parse_failure_phase(raw: bytes) -> str:
    if len(raw) > 1024:
        raise ValueError("retained-package failure output exceeded the limit")
    lines = raw.decode("ascii", errors="strict").splitlines()
    if len(lines) != 1 or not lines[0].startswith("failure_phase="):
        raise ValueError("retained-package failure output is invalid")
    phase = lines[0].removeprefix("failure_phase=")
    if not re.fullmatch(r"[a-z-]{1,32}", phase):
        raise ValueError("retained-package failure phase is invalid")
    return phase


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    package = package_gate.validate_package(
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
    )
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    paths = remote_paths(package.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    try:
        preflight_result = prototype.run_ssh(
            target, secret, build_preflight_script(paths, package), timeout=45
        )
        if preflight_result.returncode != 0:
            phase = parse_failure_phase(preflight_result.stdout)
            raise ValueError(f"retained-package preflight failed at {phase}")
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        cleanup_required = True
        with package.path.open("rb") as stream:
            upload = prototype.run_ssh(
                target,
                secret,
                build_upload_script(paths, package),
                stdin=stream,
                timeout=60,
            )
        if upload.returncode != 0:
            raise ValueError("retained-package upload failed")
        prepare_result = prototype.run_ssh(
            target, secret, build_prepare_script(paths, package), timeout=180
        )
        if prepare_result.returncode != 0:
            phase = parse_failure_phase(prepare_result.stdout)
            raise ValueError(f"retained-package prepare failed at {phase}")
        prepared = parse_facts(prepare_result.stdout, PREPARE_KEYS)
        commit_result = prototype.run_ssh(
            target, secret, build_commit_script(paths, package), timeout=90
        )
        if commit_result.returncode != 0:
            phase = parse_failure_phase(commit_result.stdout)
            raise ValueError(f"retained-package commit failed at {phase}")
        committed = parse_facts(commit_result.stdout, COMMIT_KEYS)
        final_result = prototype.run_ssh(
            target, secret, build_final_script(paths, package), timeout=45
        )
        if final_result.returncode != 0:
            raise ValueError("retained-package final state failed")
        final_state = parse_facts(final_result.stdout, FINAL_KEYS)
        cleanup_required = False
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-debian-runtime-retain-gate",
            "result": "passed",
            "package": {
                "name": debian.PACKAGE_NAME,
                "version": debian.PACKAGE_VERSION,
                "architecture": debian.PACKAGE_ARCHITECTURE,
                "sha256": package.sha256,
                "sizeBytes": package.size_bytes,
                "sourceCommit": package.source_commit,
                "sourceBundleSha256": package.source_bundle_sha256,
                "builderCommit": package.builder_commit,
            },
            "policy": {
                "packageRetained": True,
                "maintainerScripts": "absent",
                "serviceAccounts": "absent",
                "credentials": "absent",
                "servicesActive": False,
                "servicesEnabled": False,
                "nginxConfig": "documentation-example-only",
                "updateRuntime": "absent",
                "diagnosticPersistence": "absent",
                "hardwareMutation": "absent",
                "bootRollback": "armed-before-dpkg-and-removed-after-verification",
                "dpkgAuditTrailRetained": True,
                "physicalActionsInvoked": False,
            },
            "preflight": preflight,
            "prepared": prepared,
            "committed": committed,
            "finalState": final_state,
            "rollback": {"armed": True, "bootSafe": True, "removed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (
        OSError,
        UnicodeError,
        ValueError,
        subprocess.SubprocessError,
        tarfile.TarError,
    ) as error:
        cleanup: CleanupReceipt = {"attempted": False, "confirmed": not cleanup_required}
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target, secret, build_cleanup_script(paths, package), timeout=120
                )
                facts = (
                    parse_facts(result.stdout, CLEANUP_KEYS)
                    if result.returncode == 0
                    else {}
                )
                cleanup = {
                    "attempted": True,
                    "confirmed": facts == {"cleanup_removed": "yes"},
                }
            except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
                cleanup = {"attempted": True, "confirmed": False}
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-debian-runtime-retain-gate",
            "result": "failed",
            "package": {
                "sha256": package.sha256,
                "sizeBytes": package.size_bytes,
                "sourceCommit": package.source_commit,
                "sourceBundleSha256": package.source_bundle_sha256,
                "builderCommit": package.builder_commit,
            },
            "failure": {
                "stage": str(error)[:96],
                "failureType": type(error).__name__,
            },
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        _zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (
        OSError,
        UnicodeError,
        ValueError,
        subprocess.SubprocessError,
        tarfile.TarError,
    ):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
