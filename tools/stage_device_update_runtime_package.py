#!/usr/bin/env python3
"""Install and retain the inert application-update runtime package safely."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import io
import json
from pathlib import Path, PurePosixPath
import re
import secrets
import shlex
import subprocess
import sys
import tarfile
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as runtime_builder
from tools import build_debian_update_runtime_package as update_builder
from tools import stage_device_debian_runtime_package as package_gate
from tools import stage_device_prototype as prototype
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


MAX_PACKAGE_BYTES = 16 * 1024 * 1024
GUARD_DELAY_SECONDS = 600
STATE_ROOT = "/var/lib/hardened-blikvm"
UPDATE_CONFIG = "/etc/hardened-blikvm/update-supervisor.json"
UPDATE_STATE = "/var/lib/hardened-blikvm-update"
SLOT_ROOT = "/var/lib/hardened-blikvm-slots"
UPDATE_RUNTIME = "/run/hardened-blikvm-update"
UPDATE_SOCKET_RUNTIME = "/run/hardened-blikvm-update-confirm"
UNIT_NAMES = (
    "hardened-blikvm-update-watchdog.service",
    "hardened-blikvm-update-watchdog.timer",
    "hardened-blikvm-update-confirm.socket",
    "hardened-blikvm-update-confirm@.service",
    "hardened-blikvm-update-activate@.service",
    "hardened-blikvm-update-apply-candidate.service",
    "hardened-blikvm-update-apply-confirmed.service",
    "hardened-blikvm-server-current.service",
    "hardened-blikvm-server-candidate.service",
)
UNIT_PATHS = tuple(
    "/" + destination
    for destination in update_builder.FILE_MAP.values()
    if destination.startswith("lib/systemd/system/")
)
EXECUTABLE_PATHS = tuple(
    "/" + destination for destination in update_builder.EXECUTABLE_DESTINATIONS
)
PRE_KEYS = {
    "account_sha256",
    "architecture",
    "broker_pid",
    "broker_restarts",
    "config_sha256",
    "control_boundary_absent",
    "dpkg_audit_clean",
    "guards_absent",
    "hardened_nginx_pid",
    "hardened_nginx_restarts",
    "key_sha256",
    "package_absent",
    "persistent_runtime_healthy",
    "rootfs_mode",
    "server_pid",
    "server_restarts",
    "tls_sha256",
    "unit_paths_absent",
    "update_authority_absent",
    "video_pid",
    "video_restarts",
}
PREPARE_KEYS = {
    "boot_rollback_armed",
    "package_staged",
    "rollback_synced",
    "watchdog_active",
}
APPLY_KEYS = {
    "account_unchanged",
    "config_unchanged",
    "control_boundary_absent",
    "dpkg_audit_clean",
    "package_installed",
    "package_tree_verified",
    "persistent_runtime_unchanged",
    "tls_unchanged",
    "unit_templates_inert",
    "update_authority_absent",
}
FINAL_KEYS = {
    "account_unchanged",
    "config_unchanged",
    "control_boundary_absent",
    "dpkg_audit_clean",
    "guards_removed",
    "package_retained",
    "package_tree_verified",
    "persistent_runtime_unchanged",
    "tls_unchanged",
    "unit_templates_inert",
    "update_authority_absent",
}
CLEANUP_KEYS = {
    "account_unchanged",
    "config_unchanged",
    "guards_removed",
    "package_removed",
    "persistent_runtime_healthy",
    "tls_unchanged",
    "update_authority_absent",
}
ABANDON_KEYS = {"stage_removed"}
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:+~-]{1,96}$")


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
class Paths:
    run_id: str
    root: str
    marker: str
    package: str
    install_log: str
    rollback_script: str
    boot_service: str
    boot_service_path: str
    boot_service_link: str
    rollback_service: str
    rollback_service_path: str
    timer: str
    timer_path: str
    timer_link: str


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
    baseline: str
    output: str
    preflight_only: bool
    verify_only: bool
    accepted_receipt: str | None


class InstallReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    packageSha256: str
    sourceCommit: str
    preflight: dict[str, str]
    prepared: dict[str, str]
    applied: dict[str, str]
    finalState: dict[str, str]
    policy: dict[str, object]


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--package", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-source-commit", required=True)
    parser.add_argument("--expected-source-bundle-sha256", required=True)
    parser.add_argument("--expected-builder-commit", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--preflight-only", action="store_true")
    mode.add_argument("--verify-only", action="store_true")
    parser.add_argument("--accepted-receipt")
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
        baseline=cast(str, namespace.baseline),
        output=cast(str, namespace.output),
        preflight_only=cast(bool, namespace.preflight_only),
        verify_only=cast(bool, namespace.verify_only),
        accepted_receipt=cast(str | None, namespace.accepted_receipt),
    )


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_package(
    path_value: str,
    expected_sha256: str,
    expected_source_commit: str,
    expected_source_bundle_sha256: str,
    expected_builder_commit: str,
    *,
    expected_version: str = update_builder.PACKAGE_VERSION,
) -> PackageInfo:
    if expected_version not in update_builder.SUPPORTED_PACKAGE_VERSIONS:
        raise ValueError("unsupported update package version")
    path = Path(path_value)
    if not path.is_absolute() or path.is_symlink() or not path.is_file():
        raise ValueError("update package must be an absolute regular file")
    size = path.stat().st_size
    if not 0 < size <= MAX_PACKAGE_BYTES:
        raise ValueError("update package size is invalid")
    expected = prototype.validate_sha256(expected_sha256)
    expected_bundle = prototype.validate_sha256(expected_source_bundle_sha256)
    if _hash_file(path) != expected:
        raise ValueError("update package SHA-256 mismatch")
    for commit in (expected_source_commit, expected_builder_commit):
        if not re.fullmatch(r"[0-9a-f]{40}", commit):
            raise ValueError("update package commit binding is invalid")
    control = package_gate.read_dpkg_deb_archive(path, "--field").decode(
        "utf-8", errors="strict"
    )
    for required in (
        f"Package: {update_builder.PACKAGE_NAME}\n",
        f"Version: {expected_version}\n",
        f"Architecture: {update_builder.PACKAGE_ARCHITECTURE}\n",
        f"Depends: systemd (>= 249), {update_builder.RUNTIME_DEPENDENCY}\n",
    ):
        if required not in control:
            raise ValueError("update package control metadata is invalid")
    with tarfile.open(
        fileobj=io.BytesIO(
            package_gate.read_dpkg_deb_archive(path, "--ctrl-tarfile")
        ),
        mode="r:",
    ) as archive:
        control_members: list[str] = []
        for member in archive.getmembers():
            package_gate.validate_tar_metadata(member)
            if member.name in (".", "./"):
                if not member.isdir() or member.mode != 0o755:
                    raise ValueError("update control root metadata is invalid")
                continue
            control_members.append(str(package_gate.package_member_path(member.name)))
            if (
                not member.isfile()
                or member.uid != 0
                or member.gid != 0
                or member.mode != 0o644
            ):
                raise ValueError("update control member metadata is invalid")
        if control_members != ["control"]:
            raise ValueError("update package contains maintainer scripts")
    expected_files = set(update_builder.FILE_MAP.values())
    manifest_path = (
        "usr/share/doc/hardened-blikvm-update-runtime/PACKAGE-MANIFEST.json"
    )
    expected_files.add(manifest_path)
    allowed_directories: set[PurePosixPath] = set()
    for name in expected_files:
        current = PurePosixPath(name).parent
        while current != PurePosixPath("."):
            allowed_directories.add(current)
            current = current.parent
    installed_files: dict[str, tuple[str, int]] = {}
    manifest: object = None
    total_size = 0
    with tarfile.open(
        fileobj=io.BytesIO(
            package_gate.read_dpkg_deb_archive(path, "--fsys-tarfile")
        ),
        mode="r:",
    ) as archive:
        members = archive.getmembers()
        if not members or len(members) > 128:
            raise ValueError("update package member count is invalid")
        for member in members:
            package_gate.validate_tar_metadata(member)
            if member.name in (".", "./"):
                if not member.isdir() or member.mode != 0o755:
                    raise ValueError("update data root metadata is invalid")
                continue
            relative = package_gate.package_member_path(member.name)
            if member.uid != 0 or member.gid != 0 or member.issym() or member.islnk():
                raise ValueError("update package member metadata is invalid")
            if member.isdir():
                if relative not in allowed_directories or member.mode != 0o755:
                    raise ValueError("update package directory is invalid")
                continue
            name = str(relative)
            expected_mode = (
                0o755 if name in update_builder.EXECUTABLE_DESTINATIONS else 0o644
            )
            if not member.isfile() or name not in expected_files or member.mode != expected_mode:
                raise ValueError("update package file is invalid")
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError("update package file is unreadable")
            payload = stream.read(MAX_PACKAGE_BYTES + 1)
            if len(payload) != member.size:
                raise ValueError("update package member length is invalid")
            total_size += len(payload)
            if total_size > 32 * 1024 * 1024:
                raise ValueError("update package uncompressed size is invalid")
            installed_files["/" + name] = (
                hashlib.sha256(payload).hexdigest(),
                expected_mode,
            )
            if name == manifest_path:
                manifest = json.loads(payload)
    if set(path.removeprefix("/") for path in installed_files) != expected_files:
        raise ValueError("update package file tree is incomplete")
    bundle = prototype.BundleInfo(
        Path("/validated-source-bundle"),
        expected_bundle,
        1,
        expected_source_commit,
    )
    if manifest != update_builder.package_manifest(bundle, expected_builder_commit, version=expected_version):
        raise ValueError("update package manifest is invalid")
    return PackageInfo(
        path=path,
        sha256=expected,
        size_bytes=size,
        source_commit=expected_source_commit,
        source_bundle_sha256=expected_bundle,
        builder_commit=expected_builder_commit,
        installed_files=installed_files,
    )


def paths(package_sha256: str, run_id: str) -> Paths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("update package stage identity is invalid")
    stem = f"hardened-blikvm-update-package-{package_sha256[:12]}-{run_id}"
    root = f"/var/lib/{stem}"
    boot = f"{stem}-boot.service"
    rollback = f"{stem}-rollback.service"
    timer = f"{stem}-rollback.timer"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        package=f"{root}/package.deb",
        install_log=f"{root}/install.log",
        rollback_script=f"{root}/rollback.sh",
        boot_service=boot,
        boot_service_path=f"/etc/systemd/system/{boot}",
        boot_service_link=f"/etc/systemd/system/sysinit.target.wants/{boot}",
        rollback_service=rollback,
        rollback_service_path=f"/etc/systemd/system/{rollback}",
        timer=timer,
        timer_path=f"/etc/systemd/system/{timer}",
        timer_link=f"/etc/systemd/system/timers.target.wants/{timer}",
    )


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def _encoded(value: str) -> str:
    return base64.b64encode(value.encode()).decode("ascii")


def parse_facts(raw: bytes, expected: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("update package gate output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected
            or key in facts
            or not SAFE_VALUE.fullmatch(value)
        ):
            raise ValueError("update package gate returned an invalid fact")
        facts[key] = value
    if set(facts) != expected:
        raise ValueError("update package gate result was incomplete")
    return dict(sorted(facts.items()))


def load_accepted_receipt(path_value: str | None, package: PackageInfo) -> dict[str, str]:
    if not path_value:
        raise ValueError("accepted update package receipt is required")
    path = Path(path_value)
    private_root = (Path(__file__).resolve().parent.parent / "evidence/private").resolve()
    resolved = path.resolve()
    if (
        resolved.parent != private_root
        or path.is_symlink()
        or not path.is_file()
        or path.stat().st_mode & 0o777 != 0o600
        or path.stat().st_size > 64 * 1024
    ):
        raise ValueError("accepted update package receipt is unsafe")
    decoded = cast(object, json.loads(path.read_text(encoding="utf-8")))
    if not isinstance(decoded, dict):
        raise ValueError("accepted update package receipt root is invalid")
    receipt = cast(dict[str, object], decoded)
    if (
        receipt.get("schemaVersion") != 1
        or receipt.get("probe")
        != "hardened-blikvm-update-runtime-package-install"
        or receipt.get("result") != "passed"
        or receipt.get("packageSha256") != package.sha256
        or receipt.get("sourceCommit") != package.source_commit
    ):
        raise ValueError("accepted update package receipt identity is invalid")
    preflight_value = receipt.get("preflight")
    if not isinstance(preflight_value, dict):
        raise ValueError("accepted update package receipt preflight is invalid")
    lines: list[str] = []
    for key, value in cast(dict[object, object], preflight_value).items():
        if not isinstance(key, str) or not isinstance(value, str):
            raise ValueError("accepted update package receipt fact is invalid")
        lines.append(f"{key}={value}\n")
    return parse_facts("".join(lines).encode("ascii", errors="strict"), PRE_KEYS)


REMOTE_HELPERS = rf"""
{cutover.REMOTE_HELPERS}
account_valid() {{
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{{print $3}}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{{print $3}}')
  [ -f "$state_root/account.json" ] && [ ! -L "$state_root/account.json" ]
  [ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
}}
setup_closed() {{
  account_valid
  response=$(curl --noproxy '*' --insecure --silent --max-time 5 https://127.0.0.1/api/auth/setup 2>/dev/null || true)
  case "$response" in *'"required":false'*) return 0 ;; *) return 1 ;; esac
}}
runtime_package_ready() {{
  [ "$(dpkg-query -W -f='${{db:Status-Abbrev}}:${{Version}}:${{Architecture}}' "$runtime_package_name" 2>/dev/null || true)" = "ii :$runtime_package_version:arm64" ]
}}
persistent_runtime_healthy() {{
  product_active_enabled &&
  [ "$(systemctl is-active hardened-blikvm-nginx.service 2>/dev/null || true)" = active ] &&
  [ "$(service_enabled hardened-blikvm-nginx.service)" = enabled ] &&
  [ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ] &&
  [ "$(service_enabled kvmd-nginx.service)" = disabled ] &&
  [ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ] &&
  runtime_package_ready && control_boundary_absent && ports_exact && setup_closed &&
  curl --noproxy '*' --insecure --silent --fail --max-time 5 --output /dev/null https://127.0.0.1/api/health
}}
update_authority_absent() {{
  [ ! -e "$update_config" ] && [ ! -e "$update_state" ] && [ ! -e "$slot_root" ] &&
  [ ! -e "$update_runtime" ] && [ ! -e "$update_socket_runtime" ]
}}
update_unit_paths_absent() {{
  for path in $update_unit_paths; do [ ! -e "$path" ] || return 1; done
}}
update_units_inert() {{
  for unit in $update_units; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" != active ] || return 1
    state=$(systemctl is-enabled "$unit" 2>/dev/null || true)
    case "$state" in enabled|enabled-runtime|linked|linked-runtime) return 1 ;; esac
  done
  return 0
}}
guards_absent() {{
  [ -z "$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-update-package-*' -print -quit)" ]
  [ -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-update-package-*' -print -quit)" ]
  [ -z "$(find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-reboot-guard-*' -print -quit)" ]
  [ -z "$(find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-web-cutover-*' -print -quit)" ]
  [ -z "$(find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-runtime-upgrade-*' -print -quit)" ]
}}
unit_number() {{
  value=$(systemctl show -p "$2" --value "$1")
  case "$value" in ''|*[!0-9]*) return 1 ;; *) printf '%s' "$value" ;; esac
}}
""".strip()


def _common_values(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str] | None = None,
) -> dict[str, str | int]:
    config = cutover.render_nginx(baseline)
    nginx_unit = cutover.render_unit(baseline)
    values: dict[str, str | int] = {
        "run_id": paths_value.run_id,
        "stage_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": f"{paths_value.run_id}:{package.sha256}",
        "package_file": paths_value.package,
        "install_log": paths_value.install_log,
        "rollback_script": paths_value.rollback_script,
        "boot_service": paths_value.boot_service,
        "boot_service_path": paths_value.boot_service_path,
        "boot_service_link": paths_value.boot_service_link,
        "rollback_service": paths_value.rollback_service,
        "rollback_service_path": paths_value.rollback_service_path,
        "timer": paths_value.timer,
        "timer_path": paths_value.timer_path,
        "timer_link": paths_value.timer_link,
        "package_name": update_builder.PACKAGE_NAME,
        "package_version": update_builder.PACKAGE_VERSION,
        "package_architecture": update_builder.PACKAGE_ARCHITECTURE,
        "package_sha256": package.sha256,
        "package_size": package.size_bytes,
        "runtime_package_name": runtime_builder.PACKAGE_NAME,
        "runtime_package_version": runtime_builder.PACKAGE_VERSION,
        "state_root": STATE_ROOT,
        "update_config": UPDATE_CONFIG,
        "update_state": UPDATE_STATE,
        "slot_root": SLOT_ROOT,
        "update_runtime": UPDATE_RUNTIME,
        "update_socket_runtime": UPDATE_SOCKET_RUNTIME,
        "update_units": " ".join(UNIT_NAMES),
        "update_unit_paths": " ".join(UNIT_PATHS),
        "config_target": cutover.CONFIG_TARGET,
        "config_sha256": _digest(config),
        "nginx_unit_target": cutover.UNIT_TARGET,
        "nginx_unit_sha256": _digest(nginx_unit),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    if preflight:
        values.update(preflight)
    return values


def installed_checks(package: PackageInfo) -> str:
    lines: list[str] = []
    for path, (digest, mode) in sorted(package.installed_files.items()):
        quoted = shlex.quote(path)
        lines.append(
            f'[ "$(sha256sum {quoted} | awk \'{{print $1}}\')" = "{digest}" ]'
        )
        lines.append(
            f'[ "$(stat -c \'%a:%u:%g:%h\' {quoted})" = "{mode:o}:0:0:1" ]'
        )
    for path in EXECUTABLE_PATHS:
        lines.append(f'[ -z "$(getcap {shlex.quote(path)})" ]')
    return "\n".join(lines)


def runtime_unchanged_checks() -> str:
    units = (
        ("hardened-blikvm-platform-broker.service", "broker"),
        ("hardened-blikvm-video-proxy.service", "video"),
        ("hardened-blikvm-server.service", "server"),
        ("hardened-blikvm-nginx.service", "hardened_nginx"),
    )
    lines: list[str] = []
    for unit, prefix in units:
        lines.append(
            f'[ "$(systemctl show -p MainPID --value {unit})" = "${prefix}_pid" ]'
        )
        lines.append(
            f'[ "$(unit_number {unit} NRestarts)" = "${prefix}_restarts" ]'
        )
    return "\n".join(lines)


def build_preflight_script(
    paths_value: Paths, package: PackageInfo, baseline: Mapping[str, str]
) -> str:
    values = _common_values(paths_value, package, baseline)
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
for tool in awk base64 cat chmod curl df dpkg dpkg-query find findmnt getcap getent grep mkdir mv openssl pgrep rm rmdir sha256sum ss stat sync systemctl tail tr wc; do command -v "$tool" >/dev/null 2>&1; done
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
[ -z "$(dpkg --audit 2>/dev/null)" ]
case ",$(findmnt -n -o OPTIONS /)," in *,rw,*) ;; *) exit 1 ;; esac
if dpkg-query -W "$package_name" >/dev/null 2>&1; then exit 1; fi
update_unit_paths_absent
update_authority_absent
guards_absent
persistent_runtime_healthy
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
required=$((package_size + 33554432))
[ "$(df -B1 --output=avail /var/lib | tail -n 1 | tr -d ' ')" -ge "$required" ]
account_hash=$(sha256sum "$state_root/account.json" | awk '{print $1}')
broker_pid=$(systemctl show -p MainPID --value hardened-blikvm-platform-broker.service)
video_pid=$(systemctl show -p MainPID --value hardened-blikvm-video-proxy.service)
server_pid=$(systemctl show -p MainPID --value hardened-blikvm-server.service)
nginx_pid=$(systemctl show -p MainPID --value hardened-blikvm-nginx.service)
for value in "$broker_pid" "$video_pid" "$server_pid" "$nginx_pid"; do case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac; done
emit architecture aarch64
emit rootfs_mode rw
emit package_absent yes
emit unit_paths_absent yes
emit update_authority_absent yes
emit guards_absent yes
emit persistent_runtime_healthy yes
emit control_boundary_absent yes
emit dpkg_audit_clean yes
emit account_sha256 "$account_hash"
emit config_sha256 "$config_sha256"
emit tls_sha256 "$certificate_sha256"
emit key_sha256 "$key_sha256"
emit broker_pid "$broker_pid"
emit broker_restarts "$(unit_number hardened-blikvm-platform-broker.service NRestarts)"
emit video_pid "$video_pid"
emit video_restarts "$(unit_number hardened-blikvm-video-proxy.service NRestarts)"
emit server_pid "$server_pid"
emit server_restarts "$(unit_number hardened-blikvm-server.service NRestarts)"
emit hardened_nginx_pid "$nginx_pid"
emit hardened_nginx_restarts "$(unit_number hardened-blikvm-nginx.service NRestarts)"
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_upload_script(paths_value: Paths, package: PackageInfo) -> str:
    values = {
        "stage_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": f"{paths_value.run_id}:{package.sha256}",
        "package_file": paths_value.package,
        "package_sha256": package.sha256,
        "package_size": package.size_bytes,
    }
    body = r"""
set -eu
umask 077
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
chmod 600 "$marker"
cat > "$package_file.part"
[ "$(wc -c < "$package_file.part" | tr -d ' ')" = "$package_size" ]
[ "$(sha256sum "$package_file.part" | awk '{print $1}')" = "$package_sha256" ]
chmod 600 "$package_file.part"
mv "$package_file.part" "$package_file"
""".strip()
    return f"{_assignments(values)}\n{body}"


def build_boot_service(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM inert update package early rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=multi-user.target",
            f"ConditionPathExists={paths_value.marker}",
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
            f"ExecStart=/bin/sh {paths_value.rollback_script} boot",
            "TimeoutStartSec=5min",
            "",
            "[Install]",
            "WantedBy=sysinit.target",
            "",
        )
    )


def build_rollback_service(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM inert update package timed rollback",
            f"ConditionPathExists={paths_value.marker}",
            "",
            "[Service]",
            "Type=oneshot",
            "User=root",
            "Group=root",
            "UMask=0077",
            "NoNewPrivileges=yes",
            "PrivateDevices=yes",
            "PrivateNetwork=no",
            "ProtectHome=yes",
            "ProtectKernelLogs=yes",
            "ProtectKernelModules=yes",
            "ProtectKernelTunables=yes",
            "RestrictNamespaces=yes",
            f"ExecStart=/bin/sh {paths_value.rollback_script} timer",
            "TimeoutStartSec=5min",
            "",
        )
    )


def build_timer(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM inert update package deadline",
            f"ConditionPathExists={paths_value.marker}",
            "",
            "[Timer]",
            f"OnActiveSec={GUARD_DELAY_SECONDS}s",
            "AccuracySec=1s",
            f"Unit={paths_value.rollback_service}",
            "",
            "[Install]",
            "WantedBy=timers.target",
            "",
        )
    )


def build_rollback_script(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    values = _common_values(paths_value, package, baseline, preflight)
    body = r"""
set -eu
[ "$#" = 1 ]
case "$1" in boot|timer|explicit) ;; *) exit 2 ;; esac
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
if dpkg-query -W "$package_name" >/dev/null 2>&1; then
  dpkg --purge "$package_name" > "$install_log" 2>&1
fi
systemctl daemon-reload
if dpkg-query -W "$package_name" >/dev/null 2>&1; then exit 1; fi
update_unit_paths_absent
update_authority_absent
persistent_runtime_healthy
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
[ -z "$(dpkg --audit 2>/dev/null)" ]
systemctl disable --now "$timer" >/dev/null 2>&1 || true
systemctl disable "$boot_service" >/dev/null 2>&1 || true
rm -f -- "$timer_link" "$boot_service_link"
rm -f -- "$timer_path" "$rollback_service_path" "$boot_service_path"
systemctl daemon-reload
rm -f -- "$package_file" "$package_file.part" "$install_log" "$marker" "$rollback_script"
rmdir "$stage_root"
""".strip()
    return f"#!/bin/sh\n{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n"


def build_prepare_script(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    rollback = build_rollback_script(paths_value, package, baseline, preflight)
    boot = build_boot_service(paths_value)
    service = build_rollback_service(paths_value)
    timer = build_timer(paths_value)
    values = {
        **_common_values(paths_value, package, baseline, preflight),
        "rollback_b64": _encoded(rollback),
        "rollback_sha256": _digest(rollback),
        "boot_b64": _encoded(boot),
        "boot_sha256": _digest(boot),
        "service_b64": _encoded(service),
        "service_sha256": _digest(service),
        "timer_b64": _encoded(timer),
        "timer_sha256": _digest(timer),
    }
    body = r"""
set -eu
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
rollback_ready=no
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$code" -ne 0 ] && [ "$rollback_ready" = yes ]; then /bin/sh "$rollback_script" explicit >/dev/null 2>&1 || true; fi
  exit "$code"
}
trap on_exit EXIT HUP INT TERM
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ "$(sha256sum "$package_file" | awk '{print $1}')" = "$package_sha256" ]
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
[ "$(sha256sum "$rollback_script" | awk '{print $1}')" = "$rollback_sha256" ]
chmod 700 "$rollback_script"
rollback_ready=yes
printf '%s' "$boot_b64" | base64 -d > "$boot_service_path.part"
printf '%s' "$service_b64" | base64 -d > "$rollback_service_path.part"
printf '%s' "$timer_b64" | base64 -d > "$timer_path.part"
[ "$(sha256sum "$boot_service_path.part" | awk '{print $1}')" = "$boot_sha256" ]
[ "$(sha256sum "$rollback_service_path.part" | awk '{print $1}')" = "$service_sha256" ]
[ "$(sha256sum "$timer_path.part" | awk '{print $1}')" = "$timer_sha256" ]
chmod 644 "$boot_service_path.part" "$rollback_service_path.part" "$timer_path.part"
mv "$boot_service_path.part" "$boot_service_path"
mv "$rollback_service_path.part" "$rollback_service_path"
mv "$timer_path.part" "$timer_path"
systemctl daemon-reload
systemctl enable "$boot_service" "$timer" >/dev/null
systemctl start "$timer" >/dev/null
[ "$(systemctl is-active "$timer" 2>/dev/null || true)" = active ]
[ "$(service_enabled "$boot_service")" = enabled ]
[ "$(service_enabled "$timer")" = enabled ]
sync
trap - EXIT HUP INT TERM
emit package_staged yes
emit boot_rollback_armed yes
emit watchdog_active yes
emit rollback_synced yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def verified_state_checks(package: PackageInfo) -> str:
    return f"""
[ "$(dpkg-query -W -f='${{db:Status-Abbrev}}:${{Version}}:${{Architecture}}' "$package_name" 2>/dev/null || true)" = "ii :$package_version:$package_architecture" ]
{installed_checks(package)}
update_units_inert
update_authority_absent
persistent_runtime_healthy
{runtime_unchanged_checks()}
[ "$(sha256sum "$state_root/account.json" | awk '{{print $1}}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{{print $1}}')" = "$config_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{{print $1}}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{{print $1}}')" = "$key_sha256" ]
[ -z "$(dpkg --audit 2>/dev/null)" ]
""".strip()


def build_apply_script(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    values = _common_values(paths_value, package, baseline, preflight)
    checks = verified_state_checks(package)
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
phase=validate
on_exit() {{
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$code" -ne 0 ]; then /bin/sh "$rollback_script" explicit >/dev/null 2>&1 || true; fi
  exit "$code"
}}
trap on_exit EXIT HUP INT TERM
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ "$(systemctl is-active "$timer" 2>/dev/null || true)" = active ]
phase=install
dpkg -i "$package_file" > "$install_log" 2>&1
systemctl daemon-reload
phase=verify
{checks}
sync
trap - EXIT HUP INT TERM
emit package_installed yes
emit package_tree_verified yes
emit unit_templates_inert yes
emit update_authority_absent yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit config_unchanged yes
emit tls_unchanged yes
emit control_boundary_absent yes
emit dpkg_audit_clean yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_final_script(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    *,
    remove_guard: bool,
) -> str:
    values = _common_values(paths_value, package, baseline, preflight)
    checks = verified_state_checks(package)
    cleanup = ""
    if remove_guard:
        cleanup = r"""
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
systemctl disable --now "$timer" >/dev/null 2>&1
systemctl disable "$boot_service" >/dev/null 2>&1
rm -f -- "$timer_link" "$boot_service_link"
rm -f -- "$timer_path" "$rollback_service_path" "$boot_service_path"
systemctl daemon-reload
rm -f -- "$package_file" "$install_log" "$marker" "$rollback_script"
rmdir "$stage_root"
""".strip()
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
{checks}
{cleanup}
{checks}
guards_absent
emit package_retained yes
emit package_tree_verified yes
emit unit_templates_inert yes
emit update_authority_absent yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit config_unchanged yes
emit tls_unchanged yes
emit control_boundary_absent yes
emit dpkg_audit_clean yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_cleanup_script(
    paths_value: Paths,
    package: PackageInfo,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    values = _common_values(paths_value, package, baseline, preflight)
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$stage_root" ]; then
  [ -f "$rollback_script" ] && [ ! -L "$rollback_script" ]
  /bin/sh "$rollback_script" explicit
fi
if dpkg-query -W "$package_name" >/dev/null 2>&1; then exit 1; fi
update_unit_paths_absent
update_authority_absent
persistent_runtime_healthy
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
guards_absent
emit package_removed yes
emit update_authority_absent yes
emit persistent_runtime_healthy yes
emit account_unchanged yes
emit config_unchanged yes
emit tls_unchanged yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_abandon_script(paths_value: Paths, package: PackageInfo) -> str:
    values = {
        "stage_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": f"{paths_value.run_id}:{package.sha256}",
        "package_file": paths_value.package,
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$stage_root" ]; then
  [ -d "$stage_root" ] && [ ! -L "$stage_root" ]
  [ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
  unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 ! -name .owner ! -name package.deb ! -name package.deb.part -print -quit)
  [ -z "$unexpected" ]
  rm -f -- "$package_file" "$package_file.part" "$marker"
  rmdir "$stage_root"
fi
emit stage_removed yes
""".strip()
    return f"{_assignments(values)}\n{body}"


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    if options.accepted_receipt and not options.verify_only:
        raise ValueError("accepted receipt is valid only for verify-only mode")
    output = prototype.validate_output_path(options.output)
    package = validate_package(
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
    )
    baseline = cutover.load_baseline(options.baseline)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    paths_value = paths(package.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    root_created = False
    guard_prepared = False
    stage = "preflight"
    preflight: dict[str, str] = {}
    try:
        if options.verify_only:
            preflight = load_accepted_receipt(options.accepted_receipt, package)
            result = prototype.run_ssh(
                target,
                secret,
                build_final_script(
                    paths_value,
                    package,
                    baseline,
                    preflight,
                    remove_guard=False,
                ),
                timeout=90,
            )
            if result.returncode != 0:
                raise ValueError("update package independent verification failed")
            final_state = parse_facts(result.stdout, FINAL_KEYS)
            write_json_0600_atomic(
                output,
                {
                    "schemaVersion": 1,
                    "probe": "hardened-blikvm-update-runtime-package-verification",
                    "result": "passed",
                    "packageSha256": package.sha256,
                    "sourceCommit": package.source_commit,
                    "facts": final_state,
                    "policy": {
                        "packageRetained": True,
                        "deviceChanged": False,
                        "updateAuthorityCreated": False,
                        "deviceReboot": False,
                        "targetPowerControl": False,
                        "secretEmitted": False,
                    },
                },
            )
            return 0
        result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(paths_value, package, baseline),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("update package preflight failed")
        preflight = parse_facts(result.stdout, PRE_KEYS)
        if options.preflight_only:
            write_json_0600_atomic(
                output,
                {
                    "schemaVersion": 1,
                    "probe": "hardened-blikvm-update-runtime-package-preflight",
                    "result": "passed",
                    "packageSha256": package.sha256,
                    "sourceCommit": package.source_commit,
                    "facts": preflight,
                    "policy": {
                        "packageUploaded": False,
                        "deviceChanged": False,
                        "updateAuthorityCreated": False,
                        "deviceReboot": False,
                        "targetPowerControl": False,
                        "secretEmitted": False,
                    },
                },
            )
            return 0
        stage = "upload"
        with package.path.open("rb") as stream:
            result = prototype.run_ssh(
                target,
                secret,
                build_upload_script(paths_value, package),
                stdin=stream,
                timeout=90,
            )
        root_created = True
        if result.returncode != 0:
            raise ValueError("update package upload failed")
        stage = "prepare"
        result = prototype.run_ssh(
            target,
            secret,
            build_prepare_script(paths_value, package, baseline, preflight),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("update package guard prepare failed")
        prepared = parse_facts(result.stdout, PREPARE_KEYS)
        guard_prepared = True
        stage = "apply"
        result = prototype.run_ssh(
            target,
            secret,
            build_apply_script(paths_value, package, baseline, preflight),
            timeout=180,
        )
        if result.returncode != 0:
            raise ValueError("update package install failed")
        applied = parse_facts(result.stdout, APPLY_KEYS)
        stage = "commit"
        result = prototype.run_ssh(
            target,
            secret,
            build_final_script(
                paths_value,
                package,
                baseline,
                preflight,
                remove_guard=True,
            ),
            timeout=120,
        )
        if result.returncode != 0:
            raise ValueError("update package commit failed")
        _ = parse_facts(result.stdout, FINAL_KEYS)
        root_created = False
        guard_prepared = False
        stage = "independent-final"
        result = prototype.run_ssh(
            target,
            secret,
            build_final_script(
                paths_value,
                package,
                baseline,
                preflight,
                remove_guard=False,
            ),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("update package final verification failed")
        final_state = parse_facts(result.stdout, FINAL_KEYS)
        receipt: InstallReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-update-runtime-package-install",
            "result": "passed",
            "packageSha256": package.sha256,
            "sourceCommit": package.source_commit,
            "preflight": preflight,
            "prepared": prepared,
            "applied": applied,
            "finalState": final_state,
            "policy": {
                "packageRetained": True,
                "unitsEnabled": False,
                "unitsActive": False,
                "trustConfigured": False,
                "updateAuthorityCreated": False,
                "persistentRuntimeRestarted": False,
                "deviceReboot": False,
                "targetPowerControl": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup_confirmed = not root_created
        if root_created and guard_prepared and preflight:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_cleanup_script(paths_value, package, baseline, preflight),
                    timeout=180,
                )
                if result.returncode == 0:
                    parse_facts(result.stdout, CLEANUP_KEYS)
                    cleanup_confirmed = True
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup_confirmed = False
        elif root_created:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_abandon_script(paths_value, package),
                    timeout=60,
                )
                if result.returncode == 0:
                    parse_facts(result.stdout, ABANDON_KEYS)
                    cleanup_confirmed = True
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup_confirmed = False
        write_json_0600_atomic(
            output,
            {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-update-runtime-package-install",
                "result": "failed",
                "failure": {
                    "stage": stage,
                    "failureType": type(error).__name__,
                    "cleanupConfirmed": cleanup_confirmed,
                },
                "policy": {
                    "updateAuthorityCreated": False,
                    "deviceReboot": False,
                    "targetPowerControl": False,
                    "secretEmitted": False,
                },
            },
        )
        return 1
    finally:
        _zero(secret)


if __name__ == "__main__":
    raise SystemExit(main())
