#!/usr/bin/env python3
"""Upgrade the persistent runtime package and restore the Hardened web safely."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import io
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tarfile
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_debian_runtime_package as package_gate
from tools import stage_device_prototype as prototype
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


ROLLBACK_VERSION = "0.1.0~dev1"
UPGRADE_DELAY_SECONDS = 600
BROKER_UNIT = "hardened-blikvm-platform-broker.service"
VIDEO_UNIT = "hardened-blikvm-video-proxy.service"
SERVER_UNIT = "hardened-blikvm-server.service"
PRODUCT_UNITS = (BROKER_UNIT, VIDEO_UNIT, SERVER_UNIT)
PRE_KEYS = {
    "account_sha256",
    "architecture",
    "boot_race_confirmed",
    "config_preserved",
    "control_boundary_absent",
    "dpkg_audit_clean",
    "guards_absent",
    "kvmd_ready",
    "old_package_verified",
    "rootfs_mode",
    "vendor_fallback_ready",
}
PREPARE_KEYS = {
    "boot_rollback_armed",
    "packages_staged",
    "rollback_synced",
    "watchdog_active",
}
APPLY_KEYS = {
    "account_unchanged",
    "boot_conditions_absent",
    "config_unchanged",
    "control_boundary_absent",
    "hardened_nginx_active",
    "hardened_services_active",
    "kvmd_active",
    "new_package_installed",
    "ports_exact",
    "tls12",
    "tls13",
    "vendor_nginx_disabled",
}
FINAL_KEYS = {
    "account_unchanged",
    "boot_conditions_absent",
    "config_unchanged",
    "control_boundary_absent",
    "guards_removed",
    "hardened_nginx_persistent",
    "hardened_services_persistent",
    "kvmd_active",
    "new_package_verified",
    "ports_exact",
    "vendor_nginx_disabled",
}
CLEANUP_KEYS = {
    "account_unchanged",
    "config_unchanged",
    "guards_removed",
    "old_package_restored",
    "product_services_disabled",
    "vendor_nginx_restored",
}
ABANDON_KEYS = {"stage_removed"}
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:+~-]{1,96}$")


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
    rollback_package: str
    expected_rollback_sha256: str
    expected_rollback_source_commit: str
    expected_rollback_source_bundle_sha256: str
    expected_rollback_builder_commit: str
    baseline: str
    output: str


@dataclass(frozen=True)
class Paths:
    run_id: str
    root: str
    marker: str
    new_package: str
    old_package: str
    install_log: str
    rollback_log: str
    rollback_script: str
    boot_service: str
    boot_service_path: str
    boot_service_link: str
    rollback_service: str
    rollback_service_path: str
    timer: str
    timer_path: str
    timer_link: str


class UpgradeReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    sourceCommit: str
    oldPackageSha256: str
    newPackageSha256: str
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
    parser.add_argument("--rollback-package", required=True)
    parser.add_argument("--expected-rollback-sha256", required=True)
    parser.add_argument("--expected-rollback-source-commit", required=True)
    parser.add_argument("--expected-rollback-source-bundle-sha256", required=True)
    parser.add_argument("--expected-rollback-builder-commit", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    values = vars(parser.parse_args())
    return Options(**cast(dict[str, str], values))


def paths(package_sha256: str, run_id: str) -> Paths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("runtime upgrade identity is invalid")
    stem = f"hardened-blikvm-runtime-upgrade-{package_sha256[:12]}-{run_id}"
    root = f"/var/lib/{stem}"
    boot_service = f"{stem}-boot.service"
    rollback_service = f"{stem}-rollback.service"
    timer = f"{stem}-rollback.timer"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        new_package=f"{root}/new.deb",
        old_package=f"{root}/old.deb",
        install_log=f"{root}/install.log",
        rollback_log=f"{root}/rollback.log",
        rollback_script=f"{root}/rollback.sh",
        boot_service=boot_service,
        boot_service_path=f"/etc/systemd/system/{boot_service}",
        boot_service_link=f"/etc/systemd/system/sysinit.target.wants/{boot_service}",
        rollback_service=rollback_service,
        rollback_service_path=f"/etc/systemd/system/{rollback_service}",
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


def _package_member(package: package_gate.PackageInfo, wanted: str) -> bytes:
    archive_bytes = package_gate.read_dpkg_deb_archive(
        package.path, "--fsys-tarfile"
    )
    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:") as archive:
        for member in archive.getmembers():
            if member.name in (".", "./"):
                continue
            if str(package_gate.package_member_path(member.name)) != wanted:
                continue
            if not member.isfile():
                break
            stream = archive.extractfile(member)
            if stream is None:
                break
            return stream.read()
    raise ValueError("runtime package unit member is missing")


def validate_unit_semantics(
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
) -> None:
    for name, socket in (
        ("lib/systemd/system/" + BROKER_UNIT, "/run/kvmd/kvmd.sock"),
        ("lib/systemd/system/" + VIDEO_UNIT, "/run/kvmd/ustreamer.sock"),
    ):
        new_unit = _package_member(new_package, name).decode("utf-8", errors="strict")
        old_unit = _package_member(old_package, name).decode("utf-8", errors="strict")
        for unit in (new_unit, old_unit):
            if "Requires=kvmd.service\n" not in unit or "After=kvmd.service\n" not in unit:
                raise ValueError("runtime package lost KVMD ordering")
        if f"ConditionPathExists={socket}\n" not in old_unit:
            raise ValueError("rollback package does not contain the observed boot condition")
        if "ConditionPathExists=/run/kvmd/" in new_unit:
            raise ValueError("new runtime package still contains the boot race")


def parse_facts(raw: bytes, expected: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("runtime upgrade output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected
            or key in facts
            or not SAFE_VALUE.fullmatch(value)
        ):
            raise ValueError("runtime upgrade returned an invalid fact")
        facts[key] = value
    if set(facts) != expected:
        raise ValueError("runtime upgrade result was incomplete")
    return dict(sorted(facts.items()))


REMOTE_HELPERS = r"""
service_enabled() { systemctl is-enabled "$1" 2>/dev/null || true; }
control_boundary_absent() {
  [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]
  [ "$(service_enabled hardened-blikvm-control-broker.service)" = disabled ]
  [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]
}
product_active_enabled() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = active ] || return 1
    [ "$(service_enabled "$unit")" = enabled ] || return 1
  done
}
product_inactive_disabled() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
    [ "$(service_enabled "$unit")" = disabled ] || return 1
  done
}
account_valid() {
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{print $3}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{print $3}')
  [ -f "$state_root/account.json" ] && [ ! -L "$state_root/account.json" ]
  [ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
  [ "$(stat -c '%s' "$state_root/account.json")" -le 4096 ]
}
setup_closed() {
  account_valid
  response=$(curl --noproxy '*' --insecure --silent --max-time 5 https://127.0.0.1/api/auth/setup 2>/dev/null || true)
  case "$response" in *'"required":false'*) return 0 ;; *) return 1 ;; esac
}
listener_count() {
  ss -H -ltn | awk -v suffix=":"$1 '$4 ~ (suffix "$") {count++} END {print count+0}'
}
listener_scope() {
  ss -H -ltn | awk -v suffix=":"$1 '
    $4 ~ (suffix "$") {seen=1; if ($4 ~ /^0[.]0[.]0[.]0:/ || $4 ~ /^\[::\]:/ || $4 ~ /^\*:/) wildcard=1; else other=1}
    END {if (!seen) print "absent"; else if (wildcard && !other) print "wildcard"; else print "mixed"}'
}
ports_exact() {
  [ "$(listener_count 80)" = 2 ] && [ "$(listener_scope 80)" = wildcard ] &&
  [ "$(listener_count 443)" = 2 ] && [ "$(listener_scope 443)" = wildcard ]
}
served_certificate_hash() {
  certificate=$(printf '\n' | openssl s_client -connect 127.0.0.1:443 -servername localhost -showcerts 2>/dev/null | awk '/-----BEGIN CERTIFICATE-----/ {capture=1} capture {print} /-----END CERTIFICATE-----/ {exit}')
  [ -n "$certificate" ]
  printf '%s\n' "$certificate" | openssl x509 -outform DER 2>/dev/null | sha256sum | awk '{print $1}'
}
source_certificate_hash() {
  openssl x509 -in "$certificate_path" -outform DER 2>/dev/null | sha256sum | awk '{print $1}'
}
guards_absent() {
  [ -z "$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-runtime-upgrade-*' -print -quit)" ]
  [ -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-runtime-upgrade-*' -print -quit)" ]
  [ -z "$(find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-reboot-guard-*' -print -quit)" ]
  [ -z "$(find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-web-cutover-*' -print -quit)" ]
}
""".strip()


def _installed_checks(package: package_gate.PackageInfo) -> str:
    return package_gate.installed_checks(package)


def removed_on_rollback_checks(
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
) -> str:
    return "\n".join(
        f"[ ! -e {shlex.quote(path)} ]"
        for path in sorted(
            set(new_package.installed_files) - set(old_package.installed_files)
        )
    )


def _common_values(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str = "",
) -> dict[str, str | int]:
    config = cutover.render_nginx(baseline)
    unit = cutover.render_unit(baseline)
    return {
        "run_id": paths_value.run_id,
        "upgrade_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": f"{paths_value.run_id}:{new_package.sha256}:{old_package.sha256}",
        "new_package": paths_value.new_package,
        "old_package": paths_value.old_package,
        "install_log": paths_value.install_log,
        "rollback_log": paths_value.rollback_log,
        "rollback_script": paths_value.rollback_script,
        "boot_service": paths_value.boot_service,
        "boot_service_path": paths_value.boot_service_path,
        "boot_service_link": paths_value.boot_service_link,
        "rollback_service": paths_value.rollback_service,
        "rollback_service_path": paths_value.rollback_service_path,
        "timer": paths_value.timer,
        "timer_path": paths_value.timer_path,
        "timer_link": paths_value.timer_link,
        "new_sha256": new_package.sha256,
        "new_size": new_package.size_bytes,
        "new_version": package_gate.debian.PACKAGE_VERSION,
        "old_sha256": old_package.sha256,
        "old_size": old_package.size_bytes,
        "old_version": ROLLBACK_VERSION,
        "package_name": package_gate.debian.PACKAGE_NAME,
        "state_root": cutover.STATE_ROOT,
        "account_sha256": account_sha256,
        "config_target": cutover.CONFIG_TARGET,
        "config_sha256": _digest(config),
        "nginx_unit_target": cutover.UNIT_TARGET,
        "nginx_unit_sha256": _digest(unit),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }


def build_preflight_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
) -> str:
    values = _common_values(paths_value, new_package, old_package, baseline)
    old_checks = _installed_checks(old_package)
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
for tool in awk base64 cat chmod curl df dpkg dpkg-query find findmnt getent grep mkdir mv openssl pgrep rm rmdir sha256sum ss stat sync systemctl tail tr wc; do command -v "$tool" >/dev/null 2>&1; done
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
[ -z "$(dpkg --audit 2>/dev/null)" ]
case ",$(findmnt -n -o OPTIONS /)," in *,rw,*) ;; *) exit 1 ;; esac
[ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name")" = "ii :$old_version:arm64" ]
guards_absent
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd.service)" = enabled ]
[ -S /run/kvmd/kvmd.sock ] && [ -S /run/kvmd/ustreamer.sock ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
[ "$(systemctl is-active hardened-blikvm-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled hardened-blikvm-nginx.service)" = disabled ]
[ "$(systemctl is-active hardened-blikvm-platform-broker.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled hardened-blikvm-platform-broker.service)" = enabled ]
[ "$(systemctl show -p ConditionResult --value hardened-blikvm-platform-broker.service)" = no ]
[ "$(systemctl is-active hardened-blikvm-video-proxy.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled hardened-blikvm-video-proxy.service)" = enabled ]
[ "$(systemctl show -p ConditionResult --value hardened-blikvm-video-proxy.service)" = no ]
[ "$(systemctl is-active hardened-blikvm-server.service 2>/dev/null || true)" = active ]
[ "$(service_enabled hardened-blikvm-server.service)" = enabled ]
control_boundary_absent
ports_exact
account_valid
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
required=$((new_size + old_size + 33554432))
[ "$(df -B1 --output=avail /var/lib | tail -n 1 | tr -d ' ')" -ge "$required" ]
account_hash=$(sha256sum "$state_root/account.json" | awk '{print $1}')
emit architecture aarch64
emit rootfs_mode rw
emit old_package_verified yes
emit boot_race_confirmed yes
emit vendor_fallback_ready yes
emit kvmd_ready yes
emit config_preserved yes
emit account_sha256 "$account_hash"
emit control_boundary_absent yes
emit guards_absent yes
emit dpkg_audit_clean yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{old_checks}\n{body}"


def build_upload_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    *,
    old: bool,
) -> str:
    values = {
        "upgrade_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": (
            f"{paths_value.run_id}:{new_package.sha256}:{old_package.sha256}"
        ),
    }
    target = paths_value.old_package if old else paths_value.new_package
    expected_hash = old_package.sha256 if old else new_package.sha256
    expected_size = old_package.size_bytes if old else new_package.size_bytes
    initialize = "" if old else r"""
[ ! -e "$upgrade_root" ]
mkdir -m 700 "$upgrade_root"
printf '%s\n' "$marker_value" > "$marker"
chmod 600 "$marker"
""".strip()
    body = rf"""
set -eu
umask 077
{initialize}
[ -d "$upgrade_root" ] && [ ! -L "$upgrade_root" ]
[ "$(stat -c '%a:%u:%g' "$upgrade_root")" = 700:0:0 ]
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
cat > {shlex.quote(target)}.part
[ "$(wc -c < {shlex.quote(target)}.part | tr -d ' ')" = {expected_size} ]
[ "$(sha256sum {shlex.quote(target)}.part | awk '{{print $1}}')" = {expected_hash} ]
chmod 600 {shlex.quote(target)}.part
mv {shlex.quote(target)}.part {shlex.quote(target)}
""".strip()
    return f"{_assignments(values)}\n{body}"


def build_boot_service(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM runtime upgrade early rollback",
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
            "Description=Hardened BliKVM runtime upgrade timed rollback",
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
            "Description=Hardened BliKVM runtime upgrade deadline",
            f"ConditionPathExists={paths_value.marker}",
            "",
            "[Timer]",
            f"OnActiveSec={UPGRADE_DELAY_SECONDS}s",
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
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
) -> str:
    values = _common_values(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    old_checks = _installed_checks(old_package)
    obsolete_checks = removed_on_rollback_checks(new_package, old_package)
    body = r"""
set -eu
[ "$#" = 1 ]
case "$1" in boot|timer|explicit) ;; *) exit 2 ;; esac
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ "$(sha256sum "$old_package" | awk '{print $1}')" = "$old_sha256" ]
[ "$(wc -c < "$old_package" | tr -d ' ')" = "$old_size" ]
systemctl disable --now hardened-blikvm-nginx.service >/dev/null 2>&1 || true
systemctl enable kvmd-nginx.service >/dev/null 2>&1
systemctl start kvmd-nginx.service >/dev/null 2>&1
systemctl disable --now hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service >/dev/null 2>&1 || true
dpkg -i "$old_package" > "$rollback_log" 2>&1
systemctl daemon-reload
systemctl disable hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service >/dev/null 2>&1 || true
[ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name")" = "ii :$old_version:arm64" ]
""".strip()
    tail = r"""
product_inactive_disabled
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
ports_exact
account_valid
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
control_boundary_absent
systemctl disable --now "$timer" >/dev/null 2>&1 || true
systemctl disable "$boot_service" >/dev/null 2>&1 || true
rm -f -- "$timer_link" "$boot_service_link"
rm -f -- "$timer_path" "$rollback_service_path" "$boot_service_path"
systemctl daemon-reload
rm -f -- "$new_package" "$old_package" "$install_log" "$rollback_log" "$marker" "$rollback_script"
rmdir "$upgrade_root"
""".strip()
    return f"#!/bin/sh\n{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n{old_checks}\n{obsolete_checks}\n{tail}\n"


def build_prepare_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
) -> str:
    rollback = build_rollback_script(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    boot = build_boot_service(paths_value)
    service = build_rollback_service(paths_value)
    timer = build_timer(paths_value)
    values = {
        **_common_values(
            paths_value, new_package, old_package, baseline, account_sha256
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
  if [ "$code" -ne 0 ] && [ "$rollback_ready" = yes ]; then
    /bin/sh "$rollback_script" explicit >/dev/null 2>&1 || true
  fi
  exit "$code"
}
trap on_exit EXIT HUP INT TERM
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ "$(sha256sum "$new_package" | awk '{print $1}')" = "$new_sha256" ]
[ "$(sha256sum "$old_package" | awk '{print $1}')" = "$old_sha256" ]
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
printf '%s' "$boot_b64" | base64 -d > "$boot_service_path.part"
printf '%s' "$service_b64" | base64 -d > "$rollback_service_path.part"
printf '%s' "$timer_b64" | base64 -d > "$timer_path.part"
[ "$(sha256sum "$rollback_script" | awk '{print $1}')" = "$rollback_sha256" ]
[ "$(sha256sum "$boot_service_path.part" | awk '{print $1}')" = "$boot_sha256" ]
[ "$(sha256sum "$rollback_service_path.part" | awk '{print $1}')" = "$service_sha256" ]
[ "$(sha256sum "$timer_path.part" | awk '{print $1}')" = "$timer_sha256" ]
chmod 700 "$rollback_script"
rollback_ready=yes
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
emit packages_staged yes
emit boot_rollback_armed yes
emit watchdog_active yes
emit rollback_synced yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_abandon_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
) -> str:
    values = {
        "upgrade_root": paths_value.root,
        "marker": paths_value.marker,
        "marker_value": (
            f"{paths_value.run_id}:{new_package.sha256}:{old_package.sha256}"
        ),
        "new_package": paths_value.new_package,
        "old_package": paths_value.old_package,
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$upgrade_root" ]; then
  [ -d "$upgrade_root" ] && [ ! -L "$upgrade_root" ]
  [ "$(stat -c '%a:%u:%g' "$upgrade_root")" = 700:0:0 ]
  [ -f "$marker" ] && [ ! -L "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  unexpected=$(find "$upgrade_root" -mindepth 1 -maxdepth 1 ! -name .owner ! -name new.deb ! -name new.deb.part ! -name old.deb ! -name old.deb.part -print -quit)
  [ -z "$unexpected" ]
  for path in "$new_package" "$new_package.part" "$old_package" "$old_package.part"; do
    [ -e "$path" ] || continue
    [ -f "$path" ] && [ ! -L "$path" ]
    [ "$(stat -c '%u:%g:%h' "$path")" = 0:0:1 ]
    [ "$(stat -c '%s' "$path")" -le 16777216 ]
    rm -f -- "$path"
  done
  rm -f -- "$marker"
  rmdir "$upgrade_root"
fi
[ -z "$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-runtime-upgrade-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-runtime-upgrade-*' -print -quit)" ]
emit stage_removed yes
""".strip()
    return f"{_assignments(values)}\n{body}"


def _healthy_checks(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
) -> str:
    values = _common_values(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    installed = _installed_checks(new_package)
    checks = r"""
[ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name")" = "ii :$new_version:arm64" ]
product_active_enabled
[ "$(systemctl is-active hardened-blikvm-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled hardened-blikvm-nginx.service)" = enabled ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled kvmd-nginx.service)" = disabled ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
control_boundary_absent
ports_exact
setup_closed
curl --noproxy '*' --insecure --silent --fail --max-time 5 --output /dev/null https://127.0.0.1/api/health
[ "$(served_certificate_hash)" = "$(source_certificate_hash)" ]
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
! grep -Fq 'ConditionPathExists=/run/kvmd/' /lib/systemd/system/hardened-blikvm-platform-broker.service
! grep -Fq 'ConditionPathExists=/run/kvmd/' /lib/systemd/system/hardened-blikvm-video-proxy.service
[ -z "$(dpkg --audit 2>/dev/null)" ]
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{installed}\n{checks}"


def build_apply_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
) -> str:
    values = _common_values(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    healthy = _healthy_checks(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=pre-upgrade
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$code" -ne 0 ]; then
    restored=no
    if /bin/sh "$rollback_script" explicit >/dev/null 2>&1; then restored=yes; fi
    emit failure_phase "$phase"
    emit cleanup_state "$restored"
  fi
  exit "$code"
}
trap on_exit EXIT HUP INT TERM
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
[ "$(systemctl is-active "$timer" 2>/dev/null || true)" = active ]
phase=package-upgrade
dpkg -i "$new_package" > "$install_log" 2>&1
systemctl daemon-reload
phase=runtime-start
systemctl enable hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service >/dev/null
systemctl start hardened-blikvm-platform-broker.service
systemctl start hardened-blikvm-video-proxy.service
systemctl restart hardened-blikvm-server.service
count=0
while [ "$count" -lt 100 ] && ! product_active_enabled; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 100 ]
phase=listener-cutover
systemctl disable --now kvmd-nginx.service >/dev/null
systemctl enable hardened-blikvm-nginx.service >/dev/null
systemctl start hardened-blikvm-nginx.service
count=0
while [ "$count" -lt 100 ] && ! ports_exact; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 100 ]
phase=health
""".strip()
    tail = r"""
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1/api/health
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1/api/health
sync
trap - EXIT HUP INT TERM
emit new_package_installed yes
emit boot_conditions_absent yes
emit hardened_services_active yes
emit hardened_nginx_active active
emit vendor_nginx_disabled yes
emit kvmd_active active
emit ports_exact yes
emit tls12 passed
emit tls13 passed
emit account_unchanged yes
emit config_unchanged yes
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n{healthy}\n{tail}"


def build_commit_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
    *,
    remove_guard: bool,
) -> str:
    values = _common_values(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    healthy = _healthy_checks(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    cleanup = ""
    if remove_guard:
        cleanup = r"""
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
systemctl disable --now "$timer" >/dev/null 2>&1
systemctl disable "$boot_service" >/dev/null 2>&1
rm -f -- "$timer_link" "$boot_service_link"
rm -f -- "$timer_path" "$rollback_service_path" "$boot_service_path"
systemctl daemon-reload
rm -f -- "$new_package" "$old_package" "$install_log" "$rollback_log" "$marker" "$rollback_script"
rmdir "$upgrade_root"
""".strip()
    body = rf"""
set -eu
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
{healthy}
{cleanup}
{healthy}
guards_absent
emit new_package_verified yes
emit boot_conditions_absent yes
emit hardened_services_persistent yes
emit hardened_nginx_persistent yes
emit vendor_nginx_disabled yes
emit kvmd_active active
emit ports_exact yes
emit account_unchanged yes
emit config_unchanged yes
emit control_boundary_absent yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_cleanup_script(
    paths_value: Paths,
    new_package: package_gate.PackageInfo,
    old_package: package_gate.PackageInfo,
    baseline: Mapping[str, str],
    account_sha256: str,
) -> str:
    values = _common_values(
        paths_value, new_package, old_package, baseline, account_sha256
    )
    old_checks = _installed_checks(old_package)
    obsolete_checks = removed_on_rollback_checks(new_package, old_package)
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -e "$upgrade_root" ]; then
  [ -f "$rollback_script" ] && [ ! -L "$rollback_script" ]
  /bin/sh "$rollback_script" explicit
fi
[ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name")" = "ii :$old_version:arm64" ]
""".strip()
    tail = r"""
product_inactive_disabled
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
ports_exact
account_valid
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{print $1}')" = "$nginx_unit_sha256" ]
control_boundary_absent
guards_absent
emit old_package_restored yes
emit product_services_disabled yes
emit vendor_nginx_restored yes
emit account_unchanged yes
emit config_unchanged yes
emit guards_removed yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n{old_checks}\n{obsolete_checks}\n{tail}"


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    new_package = package_gate.validate_package(
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
    )
    old_package = package_gate.validate_package(
        options.rollback_package,
        options.expected_rollback_sha256,
        options.expected_rollback_source_commit,
        options.expected_rollback_source_bundle_sha256,
        options.expected_rollback_builder_commit,
        expected_version=ROLLBACK_VERSION,
    )
    validate_unit_semantics(new_package, old_package)
    baseline = cutover.load_baseline(options.baseline)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    paths_value = paths(new_package.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    root_created = False
    guard_prepared = False
    account_sha256 = ""
    stage = "preflight"
    try:
        result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(paths_value, new_package, old_package, baseline),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("runtime upgrade preflight failed")
        preflight = parse_facts(result.stdout, PRE_KEYS)
        account_sha256 = preflight["account_sha256"]
        stage = "upload-new"
        with new_package.path.open("rb") as stream:
            result = prototype.run_ssh(
                target,
                secret,
                build_upload_script(
                    paths_value, new_package, old_package, old=False
                ),
                stdin=stream,
                timeout=90,
            )
        root_created = True
        if result.returncode != 0:
            raise ValueError("new runtime package upload failed")
        stage = "upload-rollback"
        with old_package.path.open("rb") as stream:
            result = prototype.run_ssh(
                target,
                secret,
                build_upload_script(paths_value, new_package, old_package, old=True),
                stdin=stream,
                timeout=90,
            )
        if result.returncode != 0:
            raise ValueError("rollback runtime package upload failed")
        stage = "prepare-guards"
        result = prototype.run_ssh(
            target,
            secret,
            build_prepare_script(
                paths_value,
                new_package,
                old_package,
                baseline,
                account_sha256,
            ),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("runtime upgrade guard prepare failed")
        prepared = parse_facts(result.stdout, PREPARE_KEYS)
        guard_prepared = True
        stage = "apply"
        result = prototype.run_ssh(
            target,
            secret,
            build_apply_script(
                paths_value,
                new_package,
                old_package,
                baseline,
                account_sha256,
            ),
            timeout=240,
        )
        if result.returncode != 0:
            raise ValueError("runtime upgrade apply failed")
        applied = parse_facts(result.stdout, APPLY_KEYS)
        stage = "commit"
        result = prototype.run_ssh(
            target,
            secret,
            build_commit_script(
                paths_value,
                new_package,
                old_package,
                baseline,
                account_sha256,
                remove_guard=True,
            ),
            timeout=120,
        )
        if result.returncode != 0:
            raise ValueError("runtime upgrade commit failed")
        _ = parse_facts(result.stdout, FINAL_KEYS)
        root_created = False
        guard_prepared = False
        stage = "independent-final"
        result = prototype.run_ssh(
            target,
            secret,
            build_commit_script(
                paths_value,
                new_package,
                old_package,
                baseline,
                account_sha256,
                remove_guard=False,
            ),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("runtime upgrade final verification failed")
        final_state = parse_facts(result.stdout, FINAL_KEYS)
        receipt: UpgradeReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-package-upgrade",
            "result": "passed",
            "sourceCommit": new_package.source_commit,
            "oldPackageSha256": old_package.sha256,
            "newPackageSha256": new_package.sha256,
            "preflight": preflight,
            "prepared": prepared,
            "applied": applied,
            "finalState": final_state,
            "policy": {
                "rollbackPackage": ROLLBACK_VERSION,
                "accountChanged": False,
                "tlsChanged": False,
                "configurationChanged": False,
                "vendorFallback": "retained-disabled",
                "deviceReboot": False,
                "targetPowerControl": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup_confirmed = not root_created
        if root_created and guard_prepared and account_sha256:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_cleanup_script(
                        paths_value,
                        new_package,
                        old_package,
                        baseline,
                        account_sha256,
                    ),
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
                    build_abandon_script(paths_value, new_package, old_package),
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
                "probe": "hardened-blikvm-runtime-package-upgrade",
                "result": "failed",
                "failure": {
                    "stage": stage,
                    "failureType": type(error).__name__,
                    "cleanupConfirmed": cleanup_confirmed,
                },
                "policy": {
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
