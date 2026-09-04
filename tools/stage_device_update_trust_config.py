#!/usr/bin/env python3
"""Validate and provision one reviewed public update-trust configuration."""

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
import subprocess
import sys
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as runtime_builder
from tools import build_debian_update_runtime_package as update_builder
from tools import stage_device_diagnostic_collector as elf
from tools import stage_device_prototype as prototype
from tools import stage_device_update_runtime_package as update_package
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


MAX_REQUEST_BYTES = 4096
MAX_CONFIG_BYTES = 4096
GUARD_DELAY_SECONDS = 600
STATE_ROOT = "/var/lib/hardened-blikvm"
CONFIG_TARGET = "/etc/hardened-blikvm/update-supervisor.json"
VALIDATOR_NAME = re.compile(
    r"^hardened-blikvm-update-trust-config-[0-9a-f]{64}$"
)
REQUEST_NAME = re.compile(
    r"^hardened-blikvm-update-trust-request-[0-9a-f]{64}[.]json$"
)
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
PRE_KEYS = {
    "account_sha256",
    "architecture",
    "broker_pid",
    "broker_restarts",
    "config_absent",
    "config_parent_ready",
    "control_boundary_absent",
    "guards_absent",
    "hardened_nginx_pid",
    "hardened_nginx_restarts",
    "nginx_config_sha256",
    "persistent_runtime_healthy",
    "rootfs_mode",
    "server_pid",
    "server_restarts",
    "tls_key_sha256",
    "tls_sha256",
    "update_package_ready",
    "update_runtime_absent",
    "update_units_inert",
    "video_pid",
    "video_restarts",
}
VALIDATE_KEYS = {
    "canonical_config_bytes",
    "canonical_config_sha256",
    "request_sha256",
    "request_validated",
    "validator_sha256",
}
PREPARE_KEYS = {"boot_rollback_armed", "rollback_synced", "watchdog_active"}
APPLY_KEYS = {
    "account_unchanged",
    "config_published",
    "control_boundary_absent",
    "persistent_runtime_unchanged",
    "tls_unchanged",
    "update_runtime_absent",
    "update_units_inert",
}
FINAL_KEYS = {
    "account_unchanged",
    "config_persistent",
    "control_boundary_absent",
    "guards_removed",
    "persistent_runtime_unchanged",
    "staging_removed",
    "tls_unchanged",
    "update_runtime_absent",
    "update_units_inert",
}
CLEANUP_KEYS = {
    "account_unchanged",
    "config_absent",
    "guards_removed",
    "persistent_runtime_healthy",
    "staging_removed",
    "tls_unchanged",
    "update_runtime_absent",
    "update_units_inert",
}
ABANDON_KEYS = {"staging_removed"}
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:+~-]{1,96}$")


@dataclass(frozen=True)
class Asset:
    path: Path
    sha256: str
    size_bytes: int


@dataclass(frozen=True)
class Assets:
    validator: Asset
    request: Asset
    source_commit: str


@dataclass(frozen=True)
class Paths:
    run_id: str
    stage_root: str
    stage_marker: str
    validator: str
    request: str
    config: str
    config_part: str
    rollback_root: str
    rollback_marker: str
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
    validator: str
    expected_validator_sha256: str
    source_commit: str
    request: str
    expected_request_sha256: str
    confirm_reviewed_public_key: bool
    endpoint: str
    username: str
    known_hosts: str
    baseline: str
    output: str
    preflight_only: bool


class EnrollmentReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    sourceCommit: str
    requestSha256: str
    validatorSha256: str
    configSha256: str
    preflight: dict[str, str]
    validated: dict[str, str]
    prepared: dict[str, str]
    applied: dict[str, str]
    finalState: dict[str, str]
    policy: dict[str, object]


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--validator", required=True)
    parser.add_argument("--expected-validator-sha256", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--request", required=True)
    parser.add_argument("--expected-request-sha256", required=True)
    parser.add_argument("--confirm-reviewed-public-key", action="store_true")
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--preflight-only", action="store_true")
    namespace = parser.parse_args()
    return Options(
        validator=cast(str, namespace.validator),
        expected_validator_sha256=cast(str, namespace.expected_validator_sha256),
        source_commit=cast(str, namespace.source_commit),
        request=cast(str, namespace.request),
        expected_request_sha256=cast(str, namespace.expected_request_sha256),
        confirm_reviewed_public_key=cast(
            bool, namespace.confirm_reviewed_public_key
        ),
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        baseline=cast(str, namespace.baseline),
        output=cast(str, namespace.output),
        preflight_only=cast(bool, namespace.preflight_only),
    )


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_request(path_value: str, expected_sha256: str) -> Asset:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("trust request must not be a symlink")
    path = source.resolve(strict=True)
    expected = prototype.validate_sha256(expected_sha256)
    if path.parent != Path("/tmp") or REQUEST_NAME.fullmatch(path.name) is None:
        raise ValueError("trust request must be an exact hash-named snapshot")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_REQUEST_BYTES
        or metadata.st_mode & 0o077
        or metadata.st_nlink != 1
        or metadata.st_uid != os.geteuid()
        or hash_file(path) != expected
        or path.name != f"hardened-blikvm-update-trust-request-{expected}.json"
    ):
        raise ValueError("trust request metadata or hash is invalid")
    return Asset(path, expected, metadata.st_size)


def validate_assets(options: Options) -> Assets:
    if not options.confirm_reviewed_public_key:
        raise ValueError("reviewed public-key confirmation is required")
    if SOURCE_COMMIT.fullmatch(options.source_commit) is None:
        raise ValueError("trust validator source commit is invalid")
    validator = elf.validate_elf_binary(
        options.validator,
        VALIDATOR_NAME,
        "hardened-blikvm-update-trust-config",
    )
    if validator.sha256 != prototype.validate_sha256(
        options.expected_validator_sha256
    ):
        raise ValueError("trust validator SHA-256 mismatch")
    return Assets(
        validator=Asset(validator.path, validator.sha256, validator.size_bytes),
        request=validate_request(options.request, options.expected_request_sha256),
        source_commit=options.source_commit,
    )


def paths(request_sha256: str, run_id: str) -> Paths:
    if SHA256.fullmatch(request_sha256) is None or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("trust enrollment stage identity is invalid")
    suffix = f"{request_sha256[:12]}-{run_id}"
    stage = f"/tmp/hardened-blikvm-trust-enrollment-{suffix}"
    rollback = f"/var/lib/hardened-blikvm-trust-enrollment-{suffix}"
    stem = f"hardened-blikvm-trust-enrollment-{suffix}"
    boot = f"{stem}-boot.service"
    service = f"{stem}-rollback.service"
    timer = f"{stem}-rollback.timer"
    return Paths(
        run_id=run_id,
        stage_root=stage,
        stage_marker=f"{stage}/.owner",
        validator=f"{stage}/validator",
        request=f"{stage}/request.json",
        config=f"{stage}/update-supervisor.json",
        config_part=f"/etc/hardened-blikvm/.update-supervisor-{run_id}.part",
        rollback_root=rollback,
        rollback_marker=f"{rollback}/.owner",
        rollback_script=f"{rollback}/rollback.sh",
        boot_service=boot,
        boot_service_path=f"/etc/systemd/system/{boot}",
        boot_service_link=f"/etc/systemd/system/sysinit.target.wants/{boot}",
        rollback_service=service,
        rollback_service_path=f"/etc/systemd/system/{service}",
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
        raise ValueError("trust enrollment output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected
            or key in facts
            or not SAFE_VALUE.fullmatch(value)
        ):
            raise ValueError("trust enrollment returned an invalid fact")
        facts[key] = value
    if set(facts) != expected:
        raise ValueError("trust enrollment result was incomplete")
    return dict(sorted(facts.items()))


REMOTE_HELPERS = rf"""
{update_package.REMOTE_HELPERS}
trust_guards_absent() {{
  [ -z "$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-trust-enrollment-*' -print -quit)" ]
  [ -z "$(find /tmp -maxdepth 1 -type d -name 'hardened-blikvm-trust-enrollment-*' -print -quit)" ]
  [ -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-trust-enrollment-*' -print -quit)" ]
}}
update_runtime_absent() {{
  [ ! -e "$update_state" ] && [ ! -e "$slot_root" ] && [ ! -e "$update_runtime" ] && [ ! -e "$update_socket_runtime" ]
}}
config_parent_ready() {{
  [ -d /etc/hardened-blikvm ] && [ ! -L /etc/hardened-blikvm ]
  [ "$(stat -c '%a:%u:%g' /etc/hardened-blikvm)" = 700:0:0 ]
}}
""".strip()


def _common_values(
    paths_value: Paths,
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str] | None = None,
    config_sha256: str = "",
) -> dict[str, str | int]:
    nginx_config = cutover.render_nginx(baseline)
    values: dict[str, str | int] = {
        "run_id": paths_value.run_id,
        "stage_root": paths_value.stage_root,
        "stage_marker": paths_value.stage_marker,
        "stage_marker_value": (
            f"{paths_value.run_id}:{assets.request.sha256}:{assets.validator.sha256}"
        ),
        "validator": paths_value.validator,
        "request": paths_value.request,
        "canonical_config": paths_value.config,
        "config_part": paths_value.config_part,
        "config_target": CONFIG_TARGET,
        "config_sha256": config_sha256,
        "request_sha256": assets.request.sha256,
        "request_size": assets.request.size_bytes,
        "validator_sha256": assets.validator.sha256,
        "validator_size": assets.validator.size_bytes,
        "rollback_root": paths_value.rollback_root,
        "rollback_marker": paths_value.rollback_marker,
        "rollback_marker_value": (
            f"{paths_value.run_id}:{assets.request.sha256}:{config_sha256}"
        ),
        "rollback_script": paths_value.rollback_script,
        "boot_service": paths_value.boot_service,
        "boot_service_path": paths_value.boot_service_path,
        "boot_service_link": paths_value.boot_service_link,
        "rollback_service": paths_value.rollback_service,
        "rollback_service_path": paths_value.rollback_service_path,
        "timer": paths_value.timer,
        "timer_path": paths_value.timer_path,
        "timer_link": paths_value.timer_link,
        "runtime_package_name": runtime_builder.PACKAGE_NAME,
        "runtime_package_version": runtime_builder.PACKAGE_VERSION,
        "package_name": update_builder.PACKAGE_NAME,
        "package_version": update_builder.PACKAGE_VERSION,
        "state_root": STATE_ROOT,
        "update_config": CONFIG_TARGET,
        "update_state": update_package.UPDATE_STATE,
        "slot_root": update_package.SLOT_ROOT,
        "update_runtime": update_package.UPDATE_RUNTIME,
        "update_socket_runtime": update_package.UPDATE_SOCKET_RUNTIME,
        "update_units": " ".join(update_package.UNIT_NAMES),
        "update_unit_paths": " ".join(update_package.UNIT_PATHS),
        "nginx_config_sha256": _digest(nginx_config),
        "nginx_unit_target": cutover.UNIT_TARGET,
        "nginx_unit_sha256": _digest(cutover.render_unit(baseline)),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    if preflight:
        values.update(preflight)
    return values


def runtime_unchanged_checks() -> str:
    return update_package.runtime_unchanged_checks()


def build_preflight_script(
    paths_value: Paths, assets: Assets, baseline: Mapping[str, str]
) -> str:
    values = _common_values(paths_value, assets, baseline)
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
for tool in awk base64 cat chmod cp curl df dpkg-query find findmnt getent grep mkdir mv openssl pgrep rm rmdir sha256sum ss stat sync systemctl tail tr wc; do command -v "$tool" >/dev/null 2>&1; done
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
case ",$(findmnt -n -o OPTIONS /)," in *,rw,*) ;; *) exit 1 ;; esac
[ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name" 2>/dev/null || true)" = "ii :$package_version:arm64" ]
[ ! -e "$config_target" ] && [ ! -e "$config_part" ]
update_runtime_absent
update_units_inert
persistent_runtime_healthy
config_parent_ready
trust_guards_absent
[ "$(sha256sum /etc/hardened-blikvm/nginx.conf | awk '{print $1}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
required=$((validator_size + request_size + 16777216))
[ "$(df -B1 --output=avail /var/lib | tail -n 1 | tr -d ' ')" -ge "$required" ]
account_hash=$(sha256sum "$state_root/account.json" | awk '{print $1}')
broker_pid=$(systemctl show -p MainPID --value hardened-blikvm-platform-broker.service)
video_pid=$(systemctl show -p MainPID --value hardened-blikvm-video-proxy.service)
server_pid=$(systemctl show -p MainPID --value hardened-blikvm-server.service)
nginx_pid=$(systemctl show -p MainPID --value hardened-blikvm-nginx.service)
for value in "$broker_pid" "$video_pid" "$server_pid" "$nginx_pid"; do case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac; done
emit architecture aarch64
emit rootfs_mode rw
emit update_package_ready yes
emit config_absent yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_healthy yes
emit config_parent_ready yes
emit guards_absent yes
emit control_boundary_absent yes
emit account_sha256 "$account_hash"
emit nginx_config_sha256 "$nginx_config_sha256"
emit tls_sha256 "$certificate_sha256"
emit tls_key_sha256 "$key_sha256"
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


def build_upload_script(paths_value: Paths, assets: Assets, *, request: bool) -> str:
    values = {
        "stage_root": paths_value.stage_root,
        "stage_marker": paths_value.stage_marker,
        "stage_marker_value": (
            f"{paths_value.run_id}:{assets.request.sha256}:{assets.validator.sha256}"
        ),
    }
    target = paths_value.request if request else paths_value.validator
    expected_hash = assets.request.sha256 if request else assets.validator.sha256
    expected_size = assets.request.size_bytes if request else assets.validator.size_bytes
    initialize = "" if request else r"""
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$stage_marker_value" > "$stage_marker"
chmod 600 "$stage_marker"
""".strip()
    mode = 600 if request else 700
    body = rf"""
set -eu
umask 077
{initialize}
[ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
cat > {shlex.quote(target)}.part
[ "$(wc -c < {shlex.quote(target)}.part | tr -d ' ')" = {expected_size} ]
[ "$(sha256sum {shlex.quote(target)}.part | awk '{{print $1}}')" = {expected_hash} ]
chmod {mode} {shlex.quote(target)}.part
mv {shlex.quote(target)}.part {shlex.quote(target)}
""".strip()
    return f"{_assignments(values)}\n{body}"


def build_validate_script(paths_value: Paths, assets: Assets) -> str:
    values = {
        "stage_marker": paths_value.stage_marker,
        "stage_marker_value": (
            f"{paths_value.run_id}:{assets.request.sha256}:{assets.validator.sha256}"
        ),
        "validator": paths_value.validator,
        "request": paths_value.request,
        "canonical_config": paths_value.config,
        "request_sha256": assets.request.sha256,
        "validator_sha256": assets.validator.sha256,
    }
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
[ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
[ "$(sha256sum "$validator" | awk '{{print $1}}')" = "$validator_sha256" ]
[ "$(sha256sum "$request" | awk '{{print $1}}')" = "$request_sha256" ]
"$validator" --expected-sha256 "$request_sha256" --confirm-reviewed-public-key < "$request" > "$canonical_config.part"
[ -s "$canonical_config.part" ]
[ "$(wc -c < "$canonical_config.part" | tr -d ' ')" -le {MAX_CONFIG_BYTES} ]
chmod 600 "$canonical_config.part"
mv "$canonical_config.part" "$canonical_config"
config_hash=$(sha256sum "$canonical_config" | awk '{{print $1}}')
emit request_validated yes
emit request_sha256 "$request_sha256"
emit validator_sha256 "$validator_sha256"
emit canonical_config_sha256 "$config_hash"
emit canonical_config_bytes "$(wc -c < "$canonical_config" | tr -d ' ')"
""".strip()
    return f"{_assignments(values)}\n{body}"


def build_boot_service(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM trust enrollment early rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=multi-user.target",
            f"ConditionPathExists={paths_value.rollback_marker}",
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
            "Description=Hardened BliKVM trust enrollment timed rollback",
            f"ConditionPathExists={paths_value.rollback_marker}",
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
            "Description=Hardened BliKVM trust enrollment deadline",
            f"ConditionPathExists={paths_value.rollback_marker}",
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
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    config_sha256: str,
) -> str:
    values = _common_values(
        paths_value, assets, baseline, preflight, config_sha256
    )
    body = r"""
set -eu
[ "$#" = 1 ]
case "$1" in boot|timer|explicit) ;; *) exit 2 ;; esac
[ -f "$rollback_marker" ] && [ "$(cat "$rollback_marker")" = "$rollback_marker_value" ]
if [ -e "$config_target" ]; then
  [ -f "$config_target" ] && [ ! -L "$config_target" ]
  [ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ]
  [ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
  rm -f -- "$config_target"
fi
if [ -e "$config_part" ]; then
  [ -f "$config_part" ] && [ ! -L "$config_part" ]
  [ "$(stat -c '%u:%g:%h' "$config_part")" = 0:0:1 ]
  rm -f -- "$config_part"
fi
update_runtime_absent
update_units_inert
persistent_runtime_healthy
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum /etc/hardened-blikvm/nginx.conf | awk '{print $1}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$tls_key_sha256" ]
systemctl disable --now "$timer" >/dev/null 2>&1 || true
systemctl disable "$boot_service" >/dev/null 2>&1 || true
rm -f -- "$timer_link" "$boot_service_link" "$timer_path" "$timer_path.part" "$rollback_service_path" "$rollback_service_path.part" "$boot_service_path" "$boot_service_path.part"
systemctl daemon-reload
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
  rm -f -- "$canonical_config" "$canonical_config.part" "$request" "$request.part" "$validator" "$validator.part" "$stage_marker"
  rmdir "$stage_root"
fi
rm -f -- "$rollback_marker" "$rollback_script"
rmdir "$rollback_root"
""".strip()
    return f"#!/bin/sh\n{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n"


def build_prepare_script(
    paths_value: Paths,
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    config_sha256: str,
) -> str:
    rollback = build_rollback_script(
        paths_value, assets, baseline, preflight, config_sha256
    )
    boot = build_boot_service(paths_value)
    service = build_rollback_service(paths_value)
    timer = build_timer(paths_value)
    values = {
        **_common_values(
            paths_value, assets, baseline, preflight, config_sha256
        ),
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
[ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
[ -f "$canonical_config" ] && [ "$(sha256sum "$canonical_config" | awk '{print $1}')" = "$config_sha256" ]
[ ! -e "$rollback_root" ]
mkdir -m 700 "$rollback_root"
printf '%s\n' "$rollback_marker_value" > "$rollback_marker"
chmod 600 "$rollback_marker"
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
emit boot_rollback_armed yes
emit watchdog_active yes
emit rollback_synced yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def persistent_config_checks(config_sha256: str) -> str:
    return rf"""
[ -f "$config_target" ] && [ ! -L "$config_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ]
[ "$(sha256sum "$config_target" | awk '{{print $1}}')" = "{config_sha256}" ]
update_runtime_absent
update_units_inert
persistent_runtime_healthy
{runtime_unchanged_checks()}
[ "$(sha256sum "$state_root/account.json" | awk '{{print $1}}')" = "$account_sha256" ]
[ "$(sha256sum /etc/hardened-blikvm/nginx.conf | awk '{{print $1}}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{{print $1}}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{{print $1}}')" = "$tls_key_sha256" ]
control_boundary_absent
""".strip()


def build_apply_script(
    paths_value: Paths,
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    config_sha256: str,
) -> str:
    values = _common_values(
        paths_value, assets, baseline, preflight, config_sha256
    )
    checks = persistent_config_checks(config_sha256)
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
on_exit() {{
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$code" -ne 0 ]; then /bin/sh "$rollback_script" explicit >/dev/null 2>&1 || true; fi
  exit "$code"
}}
trap on_exit EXIT HUP INT TERM
[ ! -e "$config_target" ] && [ ! -e "$config_part" ]
[ -f "$rollback_marker" ] && [ "$(cat "$rollback_marker")" = "$rollback_marker_value" ]
[ "$(systemctl is-active "$timer" 2>/dev/null || true)" = active ]
cp "$canonical_config" "$config_part"
chmod 600 "$config_part"
[ "$(stat -c '%a:%u:%g:%h' "$config_part")" = 600:0:0:1 ]
[ "$(sha256sum "$config_part" | awk '{{print $1}}')" = "$config_sha256" ]
sync "$config_part"
mv "$config_part" "$config_target"
sync /etc/hardened-blikvm
{checks}
trap - EXIT HUP INT TERM
emit config_published yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit tls_unchanged yes
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_final_script(
    paths_value: Paths,
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    config_sha256: str,
    *,
    remove_guard: bool,
) -> str:
    values = _common_values(
        paths_value, assets, baseline, preflight, config_sha256
    )
    checks = persistent_config_checks(config_sha256)
    cleanup = ""
    if remove_guard:
        cleanup = r"""
[ -f "$rollback_marker" ] && [ "$(cat "$rollback_marker")" = "$rollback_marker_value" ]
systemctl disable --now "$timer" >/dev/null 2>&1
systemctl disable "$boot_service" >/dev/null 2>&1
rm -f -- "$timer_link" "$boot_service_link" "$timer_path" "$timer_path.part" "$rollback_service_path" "$rollback_service_path.part" "$boot_service_path" "$boot_service_path.part"
systemctl daemon-reload
[ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
rm -f -- "$canonical_config" "$request" "$validator" "$stage_marker"
rmdir "$stage_root"
rm -f -- "$rollback_marker" "$rollback_script"
rmdir "$rollback_root"
""".strip()
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
{checks}
{cleanup}
{checks}
trust_guards_absent
emit config_persistent yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit tls_unchanged yes
emit control_boundary_absent yes
emit staging_removed yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_cleanup_script(
    paths_value: Paths,
    assets: Assets,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
    config_sha256: str,
) -> str:
    values = _common_values(
        paths_value, assets, baseline, preflight, config_sha256
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$rollback_root" ]; then
  [ -f "$rollback_script" ] && [ ! -L "$rollback_script" ]
  /bin/sh "$rollback_script" explicit
fi
[ ! -e "$config_target" ] && [ ! -e "$config_part" ]
update_runtime_absent
update_units_inert
persistent_runtime_healthy
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$tls_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$tls_key_sha256" ]
trust_guards_absent
emit config_absent yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_healthy yes
emit account_unchanged yes
emit tls_unchanged yes
emit staging_removed yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_abandon_script(paths_value: Paths, assets: Assets) -> str:
    values = {
        "stage_root": paths_value.stage_root,
        "stage_marker": paths_value.stage_marker,
        "stage_marker_value": (
            f"{paths_value.run_id}:{assets.request.sha256}:{assets.validator.sha256}"
        ),
        "validator": paths_value.validator,
        "request": paths_value.request,
        "canonical_config": paths_value.config,
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$stage_root" ]; then
  [ -d "$stage_root" ] && [ ! -L "$stage_root" ]
  [ -f "$stage_marker" ] && [ "$(cat "$stage_marker")" = "$stage_marker_value" ]
  unexpected=$(find "$stage_root" -mindepth 1 -maxdepth 1 ! -name .owner ! -name validator ! -name validator.part ! -name request.json ! -name request.json.part ! -name update-supervisor.json ! -name update-supervisor.json.part -print -quit)
  [ -z "$unexpected" ]
  rm -f -- "$canonical_config" "$canonical_config.part" "$request" "$request.part" "$validator" "$validator.part" "$stage_marker"
  rmdir "$stage_root"
fi
emit staging_removed yes
""".strip()
    return f"{_assignments(values)}\n{body}"


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    baseline = cutover.load_baseline(options.baseline)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    paths_value = paths(assets.request.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    stage_created = False
    guard_prepared = False
    preflight: dict[str, str] = {}
    validated: dict[str, str] = {}
    try:
        result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(paths_value, assets, baseline),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("trust enrollment preflight failed")
        preflight = parse_facts(result.stdout, PRE_KEYS)
        if options.preflight_only:
            write_json_0600_atomic(
                output,
                {
                    "schemaVersion": 1,
                    "probe": "hardened-blikvm-update-trust-preflight",
                    "result": "passed",
                    "sourceCommit": assets.source_commit,
                    "requestSha256": assets.request.sha256,
                    "validatorSha256": assets.validator.sha256,
                    "facts": preflight,
                    "policy": {
                        "requestReviewed": True,
                        "deviceChanged": False,
                        "configPublished": False,
                        "stateInitialized": False,
                        "unitsEnabled": False,
                        "privateKeyAccepted": False,
                        "deviceReboot": False,
                        "targetPowerControl": False,
                        "secretEmitted": False,
                    },
                },
            )
            return 0
        stage = "upload-validator"
        with assets.validator.path.open("rb") as stream:
            result = prototype.run_ssh(
                target,
                secret,
                build_upload_script(paths_value, assets, request=False),
                stdin=stream,
                timeout=90,
            )
        stage_created = True
        if result.returncode != 0:
            raise ValueError("trust validator upload failed")
        stage = "upload-request"
        with assets.request.path.open("rb") as stream:
            result = prototype.run_ssh(
                target,
                secret,
                build_upload_script(paths_value, assets, request=True),
                stdin=stream,
                timeout=60,
            )
        if result.returncode != 0:
            raise ValueError("trust request upload failed")
        stage = "validate-request"
        result = prototype.run_ssh(
            target, secret, build_validate_script(paths_value, assets), timeout=30
        )
        if result.returncode != 0:
            raise ValueError("trust request validation failed")
        validated = parse_facts(result.stdout, VALIDATE_KEYS)
        config_sha256 = validated["canonical_config_sha256"]
        stage = "prepare-rollback"
        result = prototype.run_ssh(
            target,
            secret,
            build_prepare_script(
                paths_value, assets, baseline, preflight, config_sha256
            ),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("trust enrollment guard prepare failed")
        prepared = parse_facts(result.stdout, PREPARE_KEYS)
        guard_prepared = True
        stage = "publish-config"
        result = prototype.run_ssh(
            target,
            secret,
            build_apply_script(
                paths_value, assets, baseline, preflight, config_sha256
            ),
            timeout=120,
        )
        if result.returncode != 0:
            raise ValueError("trust configuration publication failed")
        applied = parse_facts(result.stdout, APPLY_KEYS)
        stage = "commit"
        result = prototype.run_ssh(
            target,
            secret,
            build_final_script(
                paths_value,
                assets,
                baseline,
                preflight,
                config_sha256,
                remove_guard=True,
            ),
            timeout=120,
        )
        if result.returncode != 0:
            raise ValueError("trust enrollment commit failed")
        _ = parse_facts(result.stdout, FINAL_KEYS)
        stage_created = False
        guard_prepared = False
        stage = "independent-final"
        result = prototype.run_ssh(
            target,
            secret,
            build_final_script(
                paths_value,
                assets,
                baseline,
                preflight,
                config_sha256,
                remove_guard=False,
            ),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("trust enrollment final verification failed")
        final_state = parse_facts(result.stdout, FINAL_KEYS)
        receipt: EnrollmentReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-update-trust-enrollment",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "requestSha256": assets.request.sha256,
            "validatorSha256": assets.validator.sha256,
            "configSha256": config_sha256,
            "preflight": preflight,
            "validated": validated,
            "prepared": prepared,
            "applied": applied,
            "finalState": final_state,
            "policy": {
                "requestReviewed": True,
                "publicKeyOnly": True,
                "privateKeyAccepted": False,
                "configPublished": True,
                "stateInitialized": False,
                "unitsEnabled": False,
                "persistentRuntimeRestarted": False,
                "deviceReboot": False,
                "targetPowerControl": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup_confirmed = not stage_created
        config_sha256 = validated.get("canonical_config_sha256", "")
        if guard_prepared and preflight and SHA256.fullmatch(config_sha256):
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_cleanup_script(
                        paths_value,
                        assets,
                        baseline,
                        preflight,
                        config_sha256,
                    ),
                    timeout=180,
                )
                if result.returncode == 0:
                    parse_facts(result.stdout, CLEANUP_KEYS)
                    cleanup_confirmed = True
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup_confirmed = False
        elif stage_created:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_abandon_script(paths_value, assets),
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
                "probe": "hardened-blikvm-update-trust-enrollment",
                "result": "failed",
                "failure": {
                    "stage": stage,
                    "failureType": type(error).__name__,
                    "cleanupConfirmed": cleanup_confirmed,
                },
                "policy": {
                    "privateKeyAccepted": False,
                    "stateInitialized": False,
                    "unitsEnabled": False,
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
