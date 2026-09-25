#!/usr/bin/env python3
"""Install, verify, and purge the inert runtime .deb under a rollback watchdog."""

from __future__ import annotations

import argparse
import base64
import hashlib
import io
import json
import re
import secrets
import shlex
import subprocess
import sys
import tarfile
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as debian
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


MAX_PACKAGE_BYTES = 16 * 1024 * 1024
MAX_PACKAGE_MEMBERS = 256
MAX_PACKAGE_UNCOMPRESSED_BYTES = 32 * 1024 * 1024
ALLOWED_DATA_FILE_PREFIXES = (
    PurePosixPath("usr/share/hardened-blikvm/web"),
    PurePosixPath("usr/share/doc/hardened-blikvm-runtime"),
)
ALLOWED_DATA_DIRECTORY_PREFIXES = (
    PurePosixPath("usr/libexec/hardened-blikvm"),
    *ALLOWED_DATA_FILE_PREFIXES,
)
ALLOWED_RUNTIME_FILES = {
    PurePosixPath(path) for path in debian.EXECUTABLE_DESTINATIONS
}
ALLOWED_UNIT_FILES = {
    PurePosixPath("lib/systemd/system/hardened-blikvm-platform-broker.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-video-proxy.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-control-broker.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-server.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-nginx.service"),
    # The board profile ships its own arrangement with the runtime package.
    PurePosixPath("lib/systemd/system/hardened-blikvm-board-owner.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-board-server.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-board-video-proxy.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-boardd.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-ustreamer.service"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-msd-writable.service"),
    # Reviewed owner drop-in: shared service group for the control socket.
    PurePosixPath("lib/systemd/system/hardened-blikvm-board-owner.service.d/90-socket-group.conf"),
}
ALLOWED_DATA_PARENT_DIRECTORIES = {
    PurePosixPath("usr"),
    PurePosixPath("usr/libexec"),
    PurePosixPath("usr/share"),
    PurePosixPath("usr/share/doc"),
    PurePosixPath("usr/share/hardened-blikvm"),
    PurePosixPath("lib"),
    PurePosixPath("lib/systemd"),
    PurePosixPath("lib/systemd/system"),
    PurePosixPath("lib/systemd/system/hardened-blikvm-board-owner.service.d"),
}
PREFLIGHT_KEYS = {
    "accounts_absent_before",
    "architecture",
    "dpkg_audit_clean",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "package_absent_before",
    "ports_free_before",
    "production_paths_absent_before",
    "rootfs_mode_before",
    "stage_absent_before",
    "tools_ready",
    "watchdog_absent_before",
}
RUN_KEYS = {
    "account_databases_unchanged",
    "accounts_absent_during_install",
    "installed_hashes_verified",
    "installed_modes_verified",
    "nginx_tree_unchanged",
    "package_installed",
    "package_purged",
    "persistent_runtime_roots_absent",
    "ports_free_after_run",
    "rootfs_mode_after_run",
    "stage_removed",
    "unit_templates_inactive",
    "watchdog_removed",
}
FINAL_KEYS = {
    "accounts_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "package_absent_final",
    "ports_free_final",
    "production_paths_absent_final",
    "rootfs_mode_final",
    "stage_absent_final",
    "watchdog_absent_final",
}
CLEANUP_KEYS = {"cleanup_removed"}


@dataclass(frozen=True)
class PackageInfo:
    path: Path
    sha256: str
    size_bytes: int
    source_commit: str
    source_bundle_sha256: str
    builder_commit: str
    installed_files: dict[str, tuple[str, int]]


@dataclass(frozen=True)
class RemotePaths:
    root: str
    marker: str
    package: str
    rollback_script: str
    install_log: str
    purge_log: str
    run_id: str
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


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def read_dpkg_deb_archive(path: Path, option: str) -> bytes:
    result = subprocess.run(
        ["dpkg-deb", option, str(path)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0 or len(result.stdout) > MAX_PACKAGE_UNCOMPRESSED_BYTES:
        raise ValueError("dpkg-deb rejected the package")
    return result.stdout


def package_member_path(name: str) -> PurePosixPath:
    original = PurePosixPath(name)
    parts = tuple(part for part in original.parts if part != ".")
    if not parts or original.is_absolute() or ".." in parts:
        raise ValueError("package member path is unsafe")
    return PurePosixPath(*parts)


def validate_tar_metadata(member: tarfile.TarInfo) -> None:
    if member.mode & 0o7000:
        raise ValueError("package member has a privileged mode bit")
    if any(
        key.startswith(("SCHILY.xattr.", "LIBARCHIVE.xattr."))
        for key in member.pax_headers
    ):
        raise ValueError("package member contains an extended attribute")


def validate_data_member_path(path: PurePosixPath, is_directory: bool) -> None:
    if is_directory:
        if path in ALLOWED_DATA_PARENT_DIRECTORIES or any(
            path == prefix or prefix in path.parents
            for prefix in ALLOWED_DATA_DIRECTORY_PREFIXES
        ):
            return
    elif path in ALLOWED_RUNTIME_FILES or path in ALLOWED_UNIT_FILES or any(
        prefix in path.parents for prefix in ALLOWED_DATA_FILE_PREFIXES
    ):
        return
    raise ValueError("package member is outside the inert payload allowlist")


def validate_package(
    path_value: str,
    expected_sha256: str,
    expected_source_commit: str,
    expected_source_bundle_sha256: str,
    expected_builder_commit: str,
    *,
    expected_version: str | None = None,
) -> PackageInfo:
    path = Path(path_value)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ValueError("package must be an absolute regular non-symlink file")
    size = path.stat().st_size
    if not 0 < size <= MAX_PACKAGE_BYTES:
        raise ValueError("package size is invalid")
    expected = prototype.validate_sha256(expected_sha256)
    expected_bundle = prototype.validate_sha256(expected_source_bundle_sha256)
    if _hash_file(path) != expected:
        raise ValueError("package SHA-256 mismatch")
    for commit in (expected_source_commit, expected_builder_commit):
        if not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise ValueError("package commit binding is invalid")

    package_version = expected_version or debian.PACKAGE_VERSION
    if not re.fullmatch(r"[0-9A-Za-z.+:~_-]{1,64}", package_version):
        raise ValueError("package version binding is invalid")
    control = read_dpkg_deb_archive(path, "--field").decode(
        "utf-8", errors="strict"
    )
    for required in (
        f"Package: {debian.PACKAGE_NAME}\n",
        f"Version: {package_version}\n",
        f"Architecture: {debian.PACKAGE_ARCHITECTURE}\n",
    ):
        if required not in control:
            raise ValueError("package control metadata is invalid")

    with tarfile.open(
        fileobj=io.BytesIO(read_dpkg_deb_archive(path, "--ctrl-tarfile")),
        mode="r:",
    ) as archive:
        members = archive.getmembers()
        control_members: set[str] = set()
        for member in members:
            validate_tar_metadata(member)
            if member.name in (".", "./"):
                continue
            control_members.add(str(package_member_path(member.name)))
            if (
                not member.isfile()
                or member.uid != 0
                or member.gid != 0
                or member.mode != 0o644
            ):
                raise ValueError("package control member metadata is invalid")
    if control_members != {"control"}:
        raise ValueError("package contains maintainer scripts")

    installed_files: dict[str, tuple[str, int]] = {}
    total_size = 0
    manifest: dict[str, object] | None = None
    with tarfile.open(
        fileobj=io.BytesIO(read_dpkg_deb_archive(path, "--fsys-tarfile")),
        mode="r:",
    ) as archive:
        members = archive.getmembers()
        if not members or len(members) > MAX_PACKAGE_MEMBERS:
            raise ValueError("package member count is invalid")
        for member in members:
            if member.name in (".", "./"):
                continue
            validate_tar_metadata(member)
            relative = package_member_path(member.name)
            if member.issym() or member.islnk() or not (member.isdir() or member.isfile()):
                raise ValueError("package member type is invalid")
            validate_data_member_path(relative, member.isdir())
            if member.uid != 0 or member.gid != 0:
                raise ValueError("package member ownership is invalid")
            expected_mode = (
                0o755
                if member.isdir() or relative in ALLOWED_RUNTIME_FILES
                else 0o644
            )
            if member.mode != expected_mode:
                raise ValueError("package member mode is invalid")
            if relative.parts[0] == "etc" or relative.parts[:3] == (
                "usr",
                "lib",
                "sysusers.d",
            ):
                raise ValueError("package entered an active configuration path")
            total_size += member.size
            if total_size > MAX_PACKAGE_UNCOMPRESSED_BYTES:
                raise ValueError("package uncompressed size exceeded the limit")
            if not member.isfile():
                continue
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError("package member is unreadable")
            payload = stream.read(MAX_PACKAGE_UNCOMPRESSED_BYTES + 1)
            if len(payload) != member.size:
                raise ValueError("package member length is invalid")
            absolute = "/" + str(relative)
            installed_files[absolute] = (hashlib.sha256(payload).hexdigest(), member.mode)
            if relative == PurePosixPath(
                "usr/share/doc/hardened-blikvm-runtime/PACKAGE-MANIFEST.json"
            ):
                decoded = cast(object, json.loads(payload))
                if not isinstance(decoded, dict):
                    raise ValueError("package manifest root is invalid")
                manifest = cast(dict[str, object], decoded)

    required_files = {
        "/usr/libexec/hardened-blikvm/bli-kvm-server",
        "/usr/libexec/hardened-blikvm/bli-kvm-platform-broker",
        "/usr/libexec/hardened-blikvm/bli-kvm-video-proxy",
        "/usr/libexec/hardened-blikvm/bli-kvm-control-broker",
        "/lib/systemd/system/hardened-blikvm-platform-broker.service",
        "/lib/systemd/system/hardened-blikvm-video-proxy.service",
        "/lib/systemd/system/hardened-blikvm-control-broker.service",
        "/lib/systemd/system/hardened-blikvm-server.service",
        "/usr/share/hardened-blikvm/web/index.html",
        "/usr/share/doc/hardened-blikvm-runtime/PACKAGE-MANIFEST.json",
    }
    if not required_files.issubset(installed_files):
        raise ValueError("package is missing required runtime files")
    expected_manifest = {
        "schemaVersion": 1,
        "package": debian.PACKAGE_NAME,
        "version": package_version,
        "architecture": debian.PACKAGE_ARCHITECTURE,
        "sourceCommit": expected_source_commit,
        "sourceBundleSha256": expected_bundle,
        "builderCommit": expected_builder_commit,
        "scope": "inert-read-only-runtime",
        "autoStart": False,
        "unitTemplatesActive": False,
        "accountsProvisioned": False,
        "credentialsIncluded": False,
        "nginxConfigActive": False,
        "updateRuntimeIncluded": False,
        "diagnosticPersistenceIncluded": False,
        "hardwareMutationIncluded": False,
    }
    if manifest != expected_manifest:
        raise ValueError("package manifest is invalid")
    return PackageInfo(
        path,
        expected,
        size,
        expected_source_commit,
        expected_bundle,
        expected_builder_commit,
        installed_files,
    )


def remote_paths(package_sha256: str, run_id: str) -> RemotePaths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("package stage identity is invalid")
    root = f"/tmp/hardened-blikvm-deb-{package_sha256[:12]}-{run_id}"
    return RemotePaths(
        root=root,
        marker=f"{root}/.owner",
        package=f"{root}/runtime.deb",
        rollback_script=f"{root}/rollback.sh",
        install_log=f"{root}/install.log",
        purge_log=f"{root}/purge.log",
        run_id=run_id,
        watchdog_unit=f"hardened-blikvm-deb-rollback-{run_id}.service",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


PORT_FUNCTION = r"""
port_in_use() {
  wanted=$1
  awk -v wanted="$wanted" '$2 ~ (":" wanted "$") && $4 == "0A" {found=1} END {exit !found}' /proc/net/tcp /proc/net/tcp6 2>/dev/null
}
""".strip()

STATE_FUNCTIONS = r"""
package_installed() {
  dpkg-query -W -f='${db:Status-Abbrev}' "$package_name" 2>/dev/null | grep -q '^ii '
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
units_inactive() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service \
    hardened-blikvm-control-broker.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" != active ] || return 1
    [ "$(systemctl is-enabled "$unit" 2>/dev/null || true)" != enabled ] || return 1
  done
}
""".strip()


def build_preflight_script(paths: RemotePaths, package: PackageInfo) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
            "required_bytes": package.size_bytes * 4 + 32 * 1024 * 1024,
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
for tool in awk base64 cat chmod df dpkg dpkg-deb dpkg-query find findmnt getent grep mkdir mv pgrep rm rmdir sha256sum sort stat systemctl systemd-run tail tr wc xargs; do
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
available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
[ "$available" -ge "$required_bytes" ]
phase=kvmd
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
phase=stage
[ ! -e "$stage_root" ]
phase=watchdog
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
phase=package
package_installed && exit 1
phase=accounts
accounts_absent
phase=production-paths
production_paths_absent
persistent_roots_absent
phase=units
units_inactive
phase=ports
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
phase=htpasswd
[ -f /etc/kvmd/htpasswd ]
[ ! -L /etc/kvmd/htpasswd ]
[ "$(stat -c '%a' /etc/kvmd/htpasswd)" = 600 ]
phase=complete
emit architecture aarch64
emit rootfs_mode_before "$rootfs_mode"
emit tools_ready yes
emit package_absent_before yes
emit production_paths_absent_before yes
emit accounts_absent_before yes
emit stage_absent_before yes
emit watchdog_absent_before yes
emit dpkg_audit_clean yes
emit ports_free_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def build_upload_script(paths: RemotePaths, package: PackageInfo) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "stage_marker": paths.marker,
            "run_id": paths.run_id,
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
printf '%s\n' "$run_id" > "$stage_marker"
chmod 600 "$stage_marker"
cat > "$package_file.part"
[ "$(wc -c < "$package_file.part" | tr -d ' ')" = "$expected_size" ]
[ "$(sha256sum "$package_file.part" | awk '{print $1}')" = "$expected_sha256" ]
chmod 600 "$package_file.part"
mv "$package_file.part" "$package_file"
""".strip()
    return f"{values}\n{body}"


def build_rollback_script(paths: RemotePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "stage_marker": paths.marker,
            "run_id": paths.run_id,
            "package_name": debian.PACKAGE_NAME,
        }
    )
    body = r"""
sleep 300
if [ -f "$stage_marker" ] && [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]; then
  if dpkg-query -W "$package_name" >/dev/null 2>&1; then
    dpkg --purge "$package_name" >/dev/null 2>&1 || true
    systemctl daemon-reload >/dev/null 2>&1 || true
  fi
  if ! dpkg-query -W "$package_name" >/dev/null 2>&1; then
    rm -f -- "$stage_root/runtime.deb" "$stage_root/install.log" "$stage_root/purge.log" "$stage_root/rollback.sh" "$stage_marker"
    rmdir "$stage_root" 2>/dev/null || true
  fi
fi
""".strip()
    return f"#!/bin/sh\nset -eu\n{values}\n{body}\n"


def installed_checks(package: PackageInfo) -> str:
    lines: list[str] = []
    for path, (digest, mode) in sorted(package.installed_files.items()):
        quoted = shlex.quote(path)
        lines.append(
            f'[ "$(sha256sum {quoted} | awk \'{{print $1}}\')" = "{digest}" ]'
        )
        lines.append(
            f'[ "$(stat -c \'%a:%u:%g\' {quoted})" = "{mode:o}:0:0" ]'
        )
    return "\n".join(lines)


def build_run_script(paths: RemotePaths, package: PackageInfo) -> str:
    rollback_b64 = base64.b64encode(build_rollback_script(paths).encode()).decode("ascii")
    values = _assignments(
        {
            "stage_root": paths.root,
            "stage_marker": paths.marker,
            "run_id": paths.run_id,
            "package_file": paths.package,
            "install_log": paths.install_log,
            "purge_log": paths.purge_log,
            "rollback_script": paths.rollback_script,
            "rollback_b64": rollback_b64,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "expected_sha256": package.sha256,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ -f "$stage_marker" ]
[ "$(cat "$stage_marker")" = "$run_id" ]
[ "$(sha256sum "$package_file" | awk '{print $1}')" = "$expected_sha256" ]
account_hash_before=$(sha256sum /etc/passwd /etc/group /etc/shadow /etc/gshadow | sha256sum | awk '{print $1}')
nginx_hash_before=$(find /etc/nginx -xdev -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')
htpasswd_hash_before=$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
systemd-run --quiet --collect --unit="$watchdog_unit" \
  --property=NoNewPrivileges=yes --property=PrivateDevices=yes \
  --property=ProtectHome=yes --property=ProtectKernelLogs=yes --property=ProtectKernelModules=yes \
  --property=ProtectKernelTunables=yes --property=RestrictAddressFamilies=AF_UNIX \
  /bin/sh "$rollback_script"
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" != active ]; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
dpkg -i "$package_file" > "$install_log" 2>&1
systemctl daemon-reload
package_installed
[ "$(dpkg-deb -f "$package_file" Package)" = "$package_name" ]
[ "$(dpkg-deb -f "$package_file" Version)" = "$package_version" ]
[ "$(dpkg-deb -f "$package_file" Architecture)" = "$package_architecture" ]
""".strip()
    checks = installed_checks(package)
    tail = r"""
accounts_absent
units_inactive
persistent_roots_absent
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$htpasswd_hash_before" ]
[ "$(sha256sum /etc/passwd /etc/group /etc/shadow /etc/gshadow | sha256sum | awk '{print $1}')" = "$account_hash_before" ]
[ "$(find /etc/nginx -xdev -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')" = "$nginx_hash_before" ]
dpkg --purge "$package_name" > "$purge_log" 2>&1
systemctl daemon-reload
package_installed && exit 1
production_paths_absent
persistent_roots_absent
accounts_absent
units_inactive
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$htpasswd_hash_before" ]
[ "$(sha256sum /etc/passwd /etc/group /etc/shadow /etc/gshadow | sha256sum | awk '{print $1}')" = "$account_hash_before" ]
[ "$(find /etc/nginx -xdev -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}')" = "$nginx_hash_before" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
rm -f -- "$stage_marker"
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
rm -f -- "$package_file" "$install_log" "$purge_log" "$rollback_script"
rmdir "$stage_root"
emit package_installed yes
emit installed_hashes_verified yes
emit installed_modes_verified yes
emit accounts_absent_during_install yes
emit unit_templates_inactive yes
emit persistent_runtime_roots_absent yes
emit account_databases_unchanged yes
emit nginx_tree_unchanged yes
emit package_purged yes
emit watchdog_removed yes
emit stage_removed yes
emit ports_free_after_run yes
emit rootfs_mode_after_run "$rootfs_mode"
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}\n{checks}\n{tail}"


def build_cleanup_script(paths: RemotePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "stage_marker": paths.marker,
            "run_id": paths.run_id,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if dpkg-query -W "$package_name" >/dev/null 2>&1; then dpkg --purge "$package_name" >/dev/null 2>&1; fi
systemctl daemon-reload >/dev/null 2>&1
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ]
  [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
  rm -f -- "$stage_root/runtime.deb" "$stage_root/runtime.deb.part" "$stage_root/install.log" \
    "$stage_root/purge.log" "$stage_root/rollback.sh" "$stage_marker"
  rmdir "$stage_root"
fi
! dpkg-query -W "$package_name" >/dev/null 2>&1
production_paths_absent
persistent_roots_absent
accounts_absent
units_inactive
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ ! -e "$stage_root" ]
emit cleanup_removed yes
""".strip()
    return f"{values}\n{STATE_FUNCTIONS}\n{body}"


def build_final_script(paths: RemotePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
! dpkg-query -W "$package_name" >/dev/null 2>&1
production_paths_absent
persistent_roots_absent
accounts_absent
units_inactive
[ ! -e "$stage_root" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit package_absent_final yes
emit production_paths_absent_final yes
emit accounts_absent_final yes
emit stage_absent_final yes
emit watchdog_absent_final yes
emit ports_free_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit rootfs_mode_final "$rootfs_mode"
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("package gate output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in {"yes", "active", "aarch64", "rw"}
        ):
            raise ValueError("package gate returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("package gate result was incomplete")
    return dict(sorted(facts.items()))


def parse_preflight_failure(raw: bytes) -> str:
    if len(raw) > 1024:
        raise ValueError("package preflight failure output exceeded the limit")
    lines = raw.decode("ascii", errors="strict").splitlines()
    if len(lines) != 1 or not lines[0].startswith("failure_phase="):
        raise ValueError("package preflight failure output is invalid")
    phase = lines[0].removeprefix("failure_phase=")
    if not re.fullmatch(r"[a-z-]{1,32}", phase):
        raise ValueError("package preflight failure phase is invalid")
    return phase


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    package = validate_package(
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
            target, secret, build_preflight_script(paths, package), timeout=35
        )
        if preflight_result.returncode != 0:
            phase = parse_preflight_failure(preflight_result.stdout)
            raise ValueError(f"package preflight failed at {phase}")
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
            raise ValueError("package upload failed")
        run_result = prototype.run_ssh(
            target, secret, build_run_script(paths, package), timeout=150
        )
        if run_result.returncode != 0:
            raise ValueError("package install-purge run failed")
        checks = parse_facts(run_result.stdout, RUN_KEYS)
        final_result = prototype.run_ssh(
            target, secret, build_final_script(paths), timeout=35
        )
        if final_result.returncode != 0:
            raise ValueError("package final state failed")
        final_state = parse_facts(final_result.stdout, FINAL_KEYS)
        cleanup_required = False
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-debian-runtime-package-gate",
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
                "maintainerScripts": "absent",
                "autoStart": False,
                "accountProvisioning": False,
                "credentials": "absent",
                "nginxConfig": "documentation-example-only",
                "updateRuntime": "absent",
                "diagnosticPersistence": "absent",
                "hardwareMutation": "absent",
                "dpkgAuditTrailRetained": True,
                "physicalActionsInvoked": False,
            },
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
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
                    target, secret, build_cleanup_script(paths), timeout=90
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
            "probe": "hardened-blikvm-debian-runtime-package-gate",
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
