#!/usr/bin/env python3
"""Provision fixed runtime identities and a dedicated KVMD credential with rollback."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
from pathlib import Path
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
from tools import runtime_provisioning_policy as policy
from tools import stage_device_debian_runtime_package as package_gate
from tools import stage_device_debian_runtime_retain as retain
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


PREFLIGHT_KEYS = {
    "account_databases_ready",
    "accounts_absent_before",
    "architecture",
    "config_absent_before",
    "dedicated_kvmd_account_absent",
    "entropy_ready",
    "gates_absent_before",
    "htpasswd_ready",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "locks_absent",
    "ports_free_before",
    "rootfs_mode_before",
    "sysusers_package_copy_verified",
    "tools_ready",
    "units_inactive_before",
}
PREPARE_KEYS = {
    "account_backups_verified",
    "boot_rollback_armed",
    "credential_generated",
    "htpasswd_backup_verified",
    "identities_predicted",
    "prediction_reproduced",
    "rollback_state_synced",
    "watchdog_active",
}
APPLY_KEYS = {
    "account_databases_predicted",
    "accounts_provisioned",
    "config_credentials_provisioned",
    "credential_sources_private",
    "dedicated_kvmd_authenticated",
    "htpasswd_transition_exact",
    "nginx_unchanged",
    "package_inventory_unchanged",
    "ports_free_after_apply",
    "unit_templates_inactive",
}
COMMIT_KEYS = {
    "boot_rollback_removed",
    "package_provisioned",
    "rollback_root_removed",
    "watchdog_removed",
}
FINAL_KEYS = {
    "accounts_provisioned_final",
    "config_credentials_private_final",
    "dedicated_kvmd_authenticated_final",
    "dpkg_audit_clean_final",
    "gates_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "package_installed_final",
    "ports_free_final",
    "rootfs_mode_final",
    "state_roots_absent_final",
    "unit_templates_inactive_final",
}
CLEANUP_KEYS = {"cleanup_restored"}
ALLOWED_FACT_VALUES = {"yes", "active", "aarch64", "rw"}
PASSWORD_HEX_BYTES = 64
MAX_ACCOUNT_DATABASE_BYTES = 1024 * 1024
MAX_HTPASSWD_BYTES = 1024 * 1024


@dataclass(frozen=True)
class RemotePaths:
    run_id: str
    root: str
    marker: str
    state: str
    phase: str
    account_backup: str
    synthetic_root: str
    sysusers: str
    pinned_sysusers: str
    password: str
    filtered_htpasswd: str
    rollback_script: str
    watchdog_script: str
    boot_unit: str
    boot_unit_path: str
    boot_unit_part: str
    boot_unit_link: str
    watchdog_unit: str
    config_stage: str


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


@dataclass(frozen=True)
class Assets:
    package: package_gate.PackageInfo
    sysusers: policy.SysusersInfo


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


def validate_assets(options: Options) -> Assets:
    package = package_gate.validate_package(
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
    )
    return Assets(package, policy.load_sysusers_source(package))


def remote_paths(sysusers_sha256: str, run_id: str) -> RemotePaths:
    if not re.fullmatch(r"[0-9a-f]{64}", sysusers_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("runtime provisioning identity is invalid")
    root = (
        "/var/lib/hardened-blikvm-provision-rollback-"
        f"{sysusers_sha256[:12]}-{run_id}"
    )
    boot_unit = f"hardened-blikvm-provision-rollback-{run_id}.service"
    return RemotePaths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        state=f"{root}/state",
        phase=f"{root}/phase",
        account_backup=f"{root}/account-backup",
        synthetic_root=f"{root}/synthetic-root",
        sysusers=f"{root}/runtime-sysusers.conf",
        pinned_sysusers=f"{root}/pinned-sysusers.conf",
        password=f"{root}/kvmd-password",
        filtered_htpasswd=f"{root}/htpasswd.filtered",
        rollback_script=f"{root}/rollback.sh",
        watchdog_script=f"{root}/watchdog.sh",
        boot_unit=boot_unit,
        boot_unit_path=f"/etc/systemd/system/{boot_unit}",
        boot_unit_part=f"/etc/systemd/system/{boot_unit}.part",
        boot_unit_link=f"/etc/systemd/system/sysinit.target.wants/{boot_unit}",
        watchdog_unit=f"hardened-blikvm-provision-watchdog-{run_id}.service",
        config_stage=f"/etc/.hardened-blikvm-{run_id}",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def _encoded(payload: str | bytes) -> str:
    encoded = payload.encode() if isinstance(payload, str) else payload
    return base64.b64encode(encoded).decode("ascii")


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("runtime provisioning output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in ALLOWED_FACT_VALUES
        ):
            raise ValueError("runtime provisioning returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("runtime provisioning result was incomplete")
    return dict(sorted(facts.items()))


def parse_failure_phase(raw: bytes, prefix: str) -> tuple[str, bool | None]:
    if len(raw) > 2048:
        raise ValueError("runtime provisioning failure output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key in facts or key not in {
            f"{prefix}_failure_phase",
            f"{prefix}_cleanup",
        }:
            raise ValueError("runtime provisioning failure output is invalid")
        facts[key] = value
    phase = facts.get(f"{prefix}_failure_phase", "")
    if not re.fullmatch(r"[a-z-]{1,40}", phase):
        raise ValueError("runtime provisioning failure phase is invalid")
    cleanup_value = facts.get(f"{prefix}_cleanup")
    if cleanup_value is None:
        cleanup = None
    elif cleanup_value in {"restored", "retained"}:
        cleanup = cleanup_value == "restored"
    else:
        raise ValueError("runtime provisioning cleanup fact is invalid")
    return phase, cleanup


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


PORT_FUNCTION = retain.PORT_FUNCTION

STATE_FUNCTIONS = r"""
package_installed_exact() {
  [ "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}:${Architecture}' "$package_name" 2>/dev/null || true)" = "ii :$package_version:$package_architecture" ]
}
units_loaded_disabled_inactive() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service \
    hardened-blikvm-control-broker.service hardened-blikvm-server.service; do
    [ "$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)" = loaded ] || return 1
    [ "$(systemctl is-enabled "$unit" 2>/dev/null || true)" = disabled ] || return 1
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
  done
}
runtime_accounts_absent() {
  for name in hardened-blikvm hardened-blikvm-broker hardened-blikvm-video \
    hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update; do
    if getent passwd "$name" >/dev/null 2>&1 || getent group "$name" >/dev/null 2>&1; then return 1; fi
  done
  return 0
}
runtime_state_roots_absent() {
  for path in /var/lib/hardened-blikvm /var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update; do
    [ ! -e "$path" ] || return 1
  done
}
other_package_hash() {
  (
    records=$(dpkg-query -W -f='${binary:Package}\t${Version}\t${Architecture}\t${db:Status-Abbrev}\n' 2>/dev/null | \
      awk -F '\t' -v package="$package_name" '$1 != package' | sort)
    [ -n "$records" ]
    printf '%s\n' "$records" | sha256sum | awk '{print $1}'
  )
}
nginx_tree_hash() {
  [ -d /etc/nginx ]
  find /etc/nginx -xdev -type f -print0 | sort -z | xargs -0 sha256sum | sha256sum | awk '{print $1}'
}
""".strip()


def build_preflight_script(paths: RemotePaths, assets: Assets) -> str:
    values = _assignments(
        {
            "rollback_root": paths.root,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "kvmd_account": policy.KVMD_ACCOUNT,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "installed_sysusers": policy.INSTALLED_SYSUSERS_PATH,
            "sysusers_sha256": assets.sysusers.sha256,
            "required_bytes": (
                MAX_ACCOUNT_DATABASE_BYTES * len(policy.ACCOUNT_DATABASE_NAMES)
                + MAX_HTPASSWD_BYTES
                + 16 * 1024 * 1024
            ),
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
  if [ "$code" -ne 0 ]; then emit preflight_failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
phase=tools
for tool in awk base64 cat chmod chown cmp cp curl cut df dpkg dpkg-query find findmnt getent grep install kvmd-htpasswd mkdir mv pgrep python3 readlink rm rmdir sed setpriv sha256sum sort stat sync systemctl systemd-analyze systemd-run systemd-sysusers tail tr wc xargs; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
phase=rootfs
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=capacity
available=$(df -B1 --output=avail /var | tail -n 1 | tr -d ' ')
[ "$available" -ge "$required_bytes" ]
available=$(df --output=iavail /var | tail -n 1 | tr -d ' ')
[ "$available" -ge 2048 ]
phase=package
package_installed_exact
[ -z "$(dpkg --audit 2>/dev/null)" ]
units_loaded_disabled_inactive
runtime_state_roots_absent
phase=sysusers-copy
[ -f "$installed_sysusers" ]
[ ! -L "$installed_sysusers" ]
[ "$(stat -c '%a:%u:%g:%h' "$installed_sysusers")" = 644:0:0:1 ]
[ "$(sha256sum "$installed_sysusers" | awk '{print $1}')" = "$sysusers_sha256" ]
phase=accounts
runtime_accounts_absent
[ ! -e /etc/ptmp ]
[ ! -e /etc/gtmp ]
for process in vipw vigr useradd groupadd usermod groupmod; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
pgrep -f '^systemd-sysusers( |$)' >/dev/null 2>&1 && exit 1
for name in passwd group shadow gshadow; do
  path="/etc/$name"
  [ -f "$path" ]
  [ ! -L "$path" ]
  [ "$(stat -c '%h' "$path")" = 1 ]
  [ "$(stat -c '%s' "$path")" -le 1048576 ]
done
phase=htpasswd
[ -f /etc/kvmd/htpasswd ]
[ ! -L /etc/kvmd/htpasswd ]
[ "$(stat -c '%a:%h' /etc/kvmd/htpasswd)" = 600:1 ]
[ "$(stat -c '%s' /etc/kvmd/htpasswd)" -le 1048576 ]
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$kvmd_account"; then exit 1; fi
phase=config
[ ! -e "$config_root" ]
[ ! -e "$config_stage" ]
[ -z "$(find /etc -maxdepth 1 -name '.hardened-blikvm-*' -print -quit)" ]
phase=gates
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-provision-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-provision-*' -print -quit)" ]
systemctl list-units --all --plain --no-legend 2>/dev/null | \
  awk '$1 ~ /^hardened-blikvm-provision-(rollback|watchdog)-[0-9a-f]+\.service$/ {found=1} END {exit found}'
phase=entropy
python3 -c 'import os; value = os.getrandom(1, os.GRND_NONBLOCK); assert len(value) == 1'
phase=kvmd
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
phase=ports
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
phase=complete
emit architecture aarch64
emit tools_ready yes
emit rootfs_mode_before "$rootfs_mode"
emit account_databases_ready yes
emit accounts_absent_before yes
emit config_absent_before yes
emit dedicated_kvmd_account_absent yes
emit entropy_ready yes
emit gates_absent_before yes
emit htpasswd_ready yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit locks_absent yes
emit ports_free_before yes
emit sysusers_package_copy_verified yes
emit units_inactive_before yes
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def build_boot_unit(paths: RemotePaths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM provisioning emergency rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=kvmd.service kvmd-nginx.service multi-user.target",
            f"ConditionPathExists={paths.marker}",
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
            f"ExecStart=/bin/sh {paths.rollback_script} boot",
            "TimeoutStartSec=5min",
            "",
            "[Install]",
            "WantedBy=sysinit.target",
            "",
        )
    )


REMOVE_ROLLBACK_TREE = r"""
remove_rollback_tree() {
  [ -d "$rollback_root" ]
  [ ! -L "$rollback_root" ]
  [ "$(stat -c '%a:%u:%g' "$rollback_root")" = 700:0:0 ]
  unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
    ! -name .owner ! -name state ! -name state.part ! -name phase ! -name phase.part \
    ! -name runtime-sysusers.conf ! -name pinned-sysusers.conf \
    ! -name kvmd-password ! -name htpasswd.filtered \
    ! -name rollback.sh ! -name watchdog.sh ! -name account-backup ! -name synthetic-root \
    -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$account_backup" ]; then
    [ -d "$account_backup" ]
    [ ! -L "$account_backup" ]
    [ "$(stat -c '%a:%u:%g' "$account_backup")" = 700:0:0 ]
    unexpected=$(find "$account_backup" -mindepth 1 -maxdepth 1 \
      ! -name passwd ! -name group ! -name shadow ! -name gshadow ! -name htpasswd \
      ! -name passwd- ! -name group- ! -name shadow- ! -name gshadow- ! -name .pwd.lock \
      -print -quit)
    [ -z "$unexpected" ]
    rm -f -- "$account_backup/passwd" "$account_backup/group" "$account_backup/shadow" \
      "$account_backup/gshadow" "$account_backup/htpasswd" \
      "$account_backup/passwd-" "$account_backup/group-" "$account_backup/shadow-" \
      "$account_backup/gshadow-" "$account_backup/.pwd.lock"
    rmdir "$account_backup"
  fi
  if [ -e "$synthetic_root" ]; then
    [ -d "$synthetic_root" ]
    [ ! -L "$synthetic_root" ]
    rm -f -- "$synthetic_root/etc/passwd" "$synthetic_root/etc/group" \
      "$synthetic_root/etc/shadow" "$synthetic_root/etc/gshadow" \
      "$synthetic_root/etc/passwd-" "$synthetic_root/etc/group-" \
      "$synthetic_root/etc/shadow-" "$synthetic_root/etc/gshadow-" \
      "$synthetic_root/etc/.pwd.lock" \
      "$synthetic_root/usr/lib/sysusers.d/50-runtime.conf"
    for directory in "$synthetic_root/usr/lib/sysusers.d" "$synthetic_root/usr/lib" \
      "$synthetic_root/usr" "$synthetic_root/etc" "$synthetic_root"; do
      if [ -d "$directory" ]; then rmdir "$directory"; fi
    done
  fi
  rm -f -- "$filtered_htpasswd" "$password_file" "$sysusers_file" "$pinned_sysusers" \
    "$state_file" "$rollback_root/state.part" "$phase_file" "$rollback_root/phase.part" \
    "$watchdog_script" "$rollback_script" "$marker"
  rmdir "$rollback_root"
}
""".strip()


def build_rollback_script(paths: RemotePaths, assets: Assets) -> str:
    boot_unit = build_boot_unit(paths)
    values = _assignments(
        {
            "rollback_root": paths.root,
            "run_id": paths.run_id,
            "marker": paths.marker,
            "marker_value": (
                f"{paths.run_id}:{assets.package.sha256}:{assets.sysusers.sha256}"
            ),
            "state_file": paths.state,
            "phase_file": paths.phase,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "pinned_sysusers": paths.pinned_sysusers,
            "password_file": paths.password,
            "filtered_htpasswd": paths.filtered_htpasswd,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_link": paths.boot_unit_link,
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "username_path": policy.USERNAME_PATH,
            "password_path": policy.PASSWORD_PATH,
            "kvmd_account": policy.KVMD_ACCOUNT,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "account_files": " ".join(
                f"/etc/{name}" for name in policy.ACCOUNT_DATABASE_NAMES
            ),
        }
    )
    body = r"""
set -eu
export LC_ALL=C
[ "$#" = 1 ]
case "$1" in boot|watchdog|explicit) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ ! -L "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$state_file" ]
[ ! -L "$state_file" ]
[ "$(stat -c '%a:%u:%g:%h' "$state_file")" = 600:0:0:1 ]
value() {
  awk -F= -v key="$1" '$1 == key {value=$2; count++} END {if (count == 1) print value; else exit 1}' "$state_file"
}
validate_digest() { case "$1" in *[!0-9a-f]*|'') return 1 ;; esac; [ "${#1}" = 64 ]; }
validate_config_dir() {
  root=$1
  [ -d "$root" ]
  [ ! -L "$root" ]
  [ "$(stat -c '%a:%u:%g' "$root")" = 700:0:0 ]
  unexpected=$(find "$root" -mindepth 1 -maxdepth 1 ! -name kvmd-user ! -name kvmd-password -print -quit)
  [ -z "$unexpected" ]
  user="$root/kvmd-user"
  password="$root/kvmd-password"
  for path in "$user" "$password"; do
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h' "$path")" = 400:0:0:1 ]
  done
  [ "$(wc -c < "$user" | tr -d ' ')" = 24 ]
  [ "$(cat "$user")" = "$kvmd_account" ]
  [ "$(wc -c < "$password" | tr -d ' ')" = 64 ]
  cmp -s "$password_file" "$password"
}
validate_config_stage_dir() {
  [ -d "$config_stage" ]
  [ ! -L "$config_stage" ]
  [ "$(stat -c '%a:%u:%g' "$config_stage")" = 700:0:0 ]
  unexpected=$(find "$config_stage" -mindepth 1 -maxdepth 1 ! -name kvmd-user ! -name kvmd-password -print -quit)
  [ -z "$unexpected" ]
  for path in "$config_stage/kvmd-user" "$config_stage/kvmd-password"; do
    if [ -e "$path" ]; then
      [ -f "$path" ]
      [ ! -L "$path" ]
      [ "$(stat -c '%u:%g:%h' "$path")" = 0:0:1 ]
      [ "$(stat -c '%s' "$path")" -le 64 ]
    fi
  done
}
phase=validate-backups
for name in passwd group shadow gshadow; do
  before=$(value "before_$name")
  after=$(value "after_$name")
  validate_digest "$before"
  validate_digest "$after"
  backup="$account_backup/$name"
  [ -f "$backup" ]
  [ ! -L "$backup" ]
  [ "$(stat -c '%h' "$backup")" = 1 ]
  [ "$(sha256sum "$backup" | awk '{print $1}')" = "$before" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$backup")" = "$(value "before_meta_$name")" ]
  current=$(sha256sum "/etc/$name" | awk '{print $1}')
  current_meta=$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")
  if [ "$current" = "$before" ]; then
    [ "$current_meta" = "$(value "before_meta_$name")" ]
  elif [ "$current" = "$after" ]; then
    [ "$current_meta" = "$(value "after_meta_$name")" ]
  else
    exit 1
  fi
done
precheck_aux() {
  key=$1
  name=$2
  before_present=$(value "before_${key}_present")
  after_hash=$(value "after_${key}_hash")
  after_meta=$(value "after_${key}_meta")
  validate_digest "$after_hash"
  case "$before_present" in yes|no) ;; *) return 1 ;; esac
  if [ "$before_present" = yes ]; then
    before_hash=$(value "before_${key}_hash")
    before_meta=$(value "before_${key}_meta")
    validate_digest "$before_hash"
    [ -f "$account_backup/$name" ]
    [ ! -L "$account_backup/$name" ]
    [ "$(sha256sum "$account_backup/$name" | awk '{print $1}')" = "$before_hash" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "$account_backup/$name")" = "$before_meta" ]
  fi
  if [ -e "/etc/$name" ]; then
    [ -f "/etc/$name" ]
    [ ! -L "/etc/$name" ]
    current_hash=$(sha256sum "/etc/$name" | awk '{print $1}')
    current_meta=$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")
    if [ "$current_hash:$current_meta" = "$after_hash:$after_meta" ]; then return 0; fi
    if [ "$before_present" = yes ] && \
       [ "$current_hash:$current_meta" = "$before_hash:$before_meta" ]; then return 0; fi
    return 1
  fi
  [ "$before_present" = no ]
}
precheck_aux passwd_backup passwd-
precheck_aux group_backup group-
precheck_aux shadow_backup shadow-
precheck_aux gshadow_backup gshadow-
precheck_aux pwd_lock .pwd.lock
htpasswd_before=$(value htpasswd_before)
validate_digest "$htpasswd_before"
[ -f "$account_backup/htpasswd" ]
[ ! -L "$account_backup/htpasswd" ]
[ "$(sha256sum "$account_backup/htpasswd" | awk '{print $1}')" = "$htpasswd_before" ]
[ -f "$password_file" ]
[ ! -L "$password_file" ]
[ "$(stat -c '%a:%u:%g:%h' "$password_file")" = 400:0:0:1 ]
[ "$(wc -c < "$password_file" | tr -d ' ')" = 64 ]
phase=validate-config
[ ! -e "$config_stage" ] || validate_config_stage_dir
[ ! -e "$config_root" ] || validate_config_dir "$config_root"
[ ! -e "$config_stage" ] || [ ! -e "$config_root" ]
phase=validate-htpasswd
current_htpasswd=$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')
htpasswd_state=before
if [ "$current_htpasswd" != "$htpasswd_before" ]; then
  count=$(awk -F: -v account="$kvmd_account" '$1 == account {count++} END {print count+0}' /etc/kvmd/htpasswd)
  [ "$count" = 1 ]
  if [ -e "$filtered_htpasswd" ]; then
    [ -f "$filtered_htpasswd" ]
    [ ! -L "$filtered_htpasswd" ]
    rm -f -- "$filtered_htpasswd"
  fi
  awk -F: -v account="$kvmd_account" '$1 != account {print $0}' /etc/kvmd/htpasswd > "$filtered_htpasswd"
  chmod 600 "$filtered_htpasswd"
  cmp -s "$account_backup/htpasswd" "$filtered_htpasswd"
  htpasswd_state=after
fi
phase=stop-product-units
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service \
  hardened-blikvm-platform-broker.service hardened-blikvm-control-broker.service; do
  if [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = active ]; then
    systemctl stop "$unit"
  fi
done
phase=remove-config
if [ -e "$config_stage" ]; then
  validate_config_stage_dir
  rm -f -- "$config_stage/kvmd-user" "$config_stage/kvmd-password"
  rmdir "$config_stage"
fi
if [ -e "$config_root" ]; then
  validate_config_dir "$config_root"
  rm -f -- "$username_path" "$password_path"
  rmdir "$config_root"
fi
phase=restore-htpasswd
if [ "$htpasswd_state" = after ]; then
  restore="/etc/kvmd/.htpasswd.hardened-blikvm-$run_id"
  [ ! -e "$restore" ]
  cp -a -- "$account_backup/htpasswd" "$restore"
  sync "$restore"
  mv -f -- "$restore" /etc/kvmd/htpasswd
  sync /etc/kvmd/htpasswd
fi
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$htpasswd_before" ]
phase=restore-account-databases
for database in $account_files; do
  name=${database##*/}
  before=$(value "before_$name")
  after=$(value "after_$name")
  current=$(sha256sum "$database" | awk '{print $1}')
  if [ "$current" = "$after" ]; then
    restore="/etc/.$name.hardened-blikvm-$run_id"
    [ ! -e "$restore" ]
    cp -a -- "$account_backup/$name" "$restore"
    sync "$restore"
    mv -f -- "$restore" "$database"
    sync "$database"
  elif [ "$current" != "$before" ]; then
    exit 3
  fi
  [ "$(sha256sum "$database" | awk '{print $1}')" = "$before" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$database")" = "$(value "before_meta_$name")" ]
  cmp -s "$account_backup/$name" "$database"
done
restore_aux() {
  key=$1
  name=$2
  before_present=$(value "before_${key}_present")
  after_hash=$(value "after_${key}_hash")
  after_meta=$(value "after_${key}_meta")
  if [ -e "/etc/$name" ] && \
     [ "$(sha256sum "/etc/$name" | awk '{print $1}'):$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")" = "$after_hash:$after_meta" ]; then
    if [ "$before_present" = yes ]; then
      restore="/etc/.$name.hardened-blikvm-$run_id"
      [ ! -e "$restore" ]
      cp -a -- "$account_backup/$name" "$restore"
      sync "$restore"
      mv -f -- "$restore" "/etc/$name"
      sync "/etc/$name"
    else
      rm -f -- "/etc/$name"
      sync /etc
    fi
  fi
  if [ "$before_present" = yes ]; then
    [ "$(sha256sum "/etc/$name" | awk '{print $1}')" = "$(value "before_${key}_hash")" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")" = "$(value "before_${key}_meta")" ]
  else
    [ ! -e "/etc/$name" ]
  fi
}
restore_aux passwd_backup passwd-
restore_aux group_backup group-
restore_aux shadow_backup shadow-
restore_aux gshadow_backup gshadow-
restore_aux pwd_lock .pwd.lock
runtime_accounts_absent
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$kvmd_account"; then exit 1; fi
package_installed_exact
units_loaded_disabled_inactive
runtime_state_roots_absent
phase=remove-boot-rollback
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
systemctl daemon-reload >/dev/null 2>&1
phase=remove-rollback-root
remove_rollback_tree
""".strip()
    return (
        f"#!/bin/sh\n{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{REMOVE_ROLLBACK_TREE}\n{body}\n"
    )


def _prepare_material(paths: RemotePaths, assets: Assets) -> tuple[str, str, str]:
    boot_unit = build_boot_unit(paths)
    rollback = build_rollback_script(paths, assets)
    watchdog = (
        "#!/bin/sh\nset -eu\nsleep 600\nexec /bin/sh "
        f"{shlex.quote(paths.rollback_script)} watchdog\n"
    )
    return boot_unit, rollback, watchdog


def build_prepare_script(paths: RemotePaths, assets: Assets) -> str:
    boot_unit, rollback, watchdog = _prepare_material(paths, assets)
    values = _assignments(
        {
            "rollback_root": paths.root,
            "marker": paths.marker,
            "marker_value": (
                f"{paths.run_id}:{assets.package.sha256}:{assets.sysusers.sha256}"
            ),
            "state_file": paths.state,
            "phase_file": paths.phase,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "pinned_sysusers": paths.pinned_sysusers,
            "password_file": paths.password,
            "filtered_htpasswd": paths.filtered_htpasswd,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "package_sha256": assets.package.sha256,
            "sysusers_sha256": assets.sysusers.sha256,
            "sysusers_b64": _encoded(assets.sysusers.payload),
            "rollback_b64": _encoded(rollback),
            "watchdog_b64": _encoded(watchdog),
            "boot_unit_b64": _encoded(boot_unit),
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        }
    )
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=create-root
on_exit() {
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit prepare_failure_phase "$phase"; fi
  exit "$code"
}
trap on_exit EXIT
[ ! -e "$rollback_root" ]
mkdir -m 700 "$rollback_root"
printf '%s\n' "$marker_value" > "$marker"
chmod 600 "$marker"
phase=backup
mkdir -m 700 "$account_backup"
mkdir -m 700 "$synthetic_root"
mkdir -m 700 "$synthetic_root/etc"
mkdir -m 755 "$synthetic_root/usr"
mkdir -m 755 "$synthetic_root/usr/lib"
mkdir -m 755 "$synthetic_root/usr/lib/sysusers.d"
for name in passwd group shadow gshadow; do
  cp -a -- "/etc/$name" "$account_backup/$name"
  cp -a -- "/etc/$name" "$synthetic_root/etc/$name"
  [ "$(stat -c '%h' "$account_backup/$name")" = 1 ]
  [ "$(stat -c '%s' "$account_backup/$name")" -le 1048576 ]
done
for name in passwd- group- shadow- gshadow- .pwd.lock; do
  if [ -e "/etc/$name" ]; then
    [ -f "/etc/$name" ]
    [ ! -L "/etc/$name" ]
    [ "$(stat -c '%h' "/etc/$name")" = 1 ]
    [ "$(stat -c '%s' "/etc/$name")" -le 1048576 ]
    cp -a -- "/etc/$name" "$account_backup/$name"
  fi
done
cp -a -- /etc/kvmd/htpasswd "$account_backup/htpasswd"
[ "$(stat -c '%h' "$account_backup/htpasswd")" = 1 ]
[ "$(stat -c '%s' "$account_backup/htpasswd")" -le 1048576 ]
printf '%s' "$sysusers_b64" | base64 -d > "$sysusers_file"
[ "$(sha256sum "$sysusers_file" | awk '{print $1}')" = "$sysusers_sha256" ]
chmod 600 "$sysusers_file"
install -m 644 "$sysusers_file" "$synthetic_root/usr/lib/sysusers.d/50-runtime.conf"
phase=first-prediction
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
synthetic_passwd="$synthetic_root/etc/passwd"
synthetic_group="$synthetic_root/etc/group"
validate_account() {
  name=$1
  entry=$(awk -F: -v name="$name" '$1 == name {print $0}' "$synthetic_passwd")
  [ -n "$entry" ]
  [ "$(printf '%s\n' "$entry" | wc -l | tr -d ' ')" = 1 ]
  uid=$(printf '%s\n' "$entry" | awk -F: '{print $3}')
  gid=$(printf '%s\n' "$entry" | awk -F: '{print $4}')
  home=$(printf '%s\n' "$entry" | awk -F: '{print $6}')
  shell=$(printf '%s\n' "$entry" | awk -F: '{print $7}')
  case "$uid:$gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
  [ "$home" = /nonexistent ]
  [ "$shell" = /usr/sbin/nologin ]
  [ "$(awk -F: -v name="$name" '$1 == name {print $3}' "$synthetic_group")" = "$gid" ]
}
for name in hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update; do
  validate_account "$name"
done
broker_uid=$(awk -F: '$1 == "hardened-blikvm-broker" {print $3}' "$synthetic_passwd")
video_uid=$(awk -F: '$1 == "hardened-blikvm-video" {print $3}' "$synthetic_passwd")
control_uid=$(awk -F: '$1 == "hardened-blikvm-control" {print $3}' "$synthetic_passwd")
server_uid=$(awk -F: '$1 == "hardened-blikvm-server" {print $3}' "$synthetic_passwd")
update_uid=$(awk -F: '$1 == "hardened-blikvm-update" {print $3}' "$synthetic_passwd")
broker_gid=$(awk -F: '$1 == "hardened-blikvm-broker" {print $3}' "$synthetic_group")
video_gid=$(awk -F: '$1 == "hardened-blikvm-video" {print $3}' "$synthetic_group")
control_gid=$(awk -F: '$1 == "hardened-blikvm-control" {print $3}' "$synthetic_group")
server_gid=$(awk -F: '$1 == "hardened-blikvm-server" {print $3}' "$synthetic_group")
update_gid=$(awk -F: '$1 == "hardened-blikvm-update" {print $3}' "$synthetic_group")
shared_gid=$(awk -F: '$1 == "hardened-blikvm" {print $3}' "$synthetic_group")
[ "$(printf '%s\n' "$broker_uid" "$video_uid" "$control_uid" "$server_uid" "$update_uid" | sort -u | wc -l | tr -d ' ')" = 5 ]
[ "$(printf '%s\n' "$shared_gid" "$broker_gid" "$video_gid" "$control_gid" "$server_gid" "$update_gid" | sort -u | wc -l | tr -d ' ')" = 6 ]
for value in "$broker_uid" "$video_uid" "$control_uid" "$server_uid" "$update_uid"; do
  if awk -F: -v id="$value" '$3 == id {found=1} END {exit !found}' "$account_backup/passwd"; then exit 1; fi
done
for value in "$shared_gid" "$broker_gid" "$video_gid" "$control_gid" "$server_gid" "$update_gid"; do
  if awk -F: -v id="$value" '$3 == id {found=1} END {exit !found}' "$account_backup/group"; then exit 1; fi
done
members=$(awk -F: '$1 == "hardened-blikvm" {gsub(/,/,"\n",$4); print $4}' "$synthetic_group" | sort)
expected_members=$(printf '%s\n' hardened-blikvm-broker hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update hardened-blikvm-video | sort)
[ "$members" = "$expected_members" ]
managed_accounts='hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update'
managed_groups="hardened-blikvm $managed_accounts"
for name in passwd shadow; do
  awk -F: -v names="$managed_accounts" 'BEGIN {n=split(names,a," "); for(i=1;i<=n;i++) managed[a[i]]=1} !($1 in managed) {print $0}' \
    "$synthetic_root/etc/$name" | cmp -s "$account_backup/$name" -
done
for name in group gshadow; do
  awk -F: -v names="$managed_groups" 'BEGIN {n=split(names,a," "); for(i=1;i<=n;i++) managed[a[i]]=1} !($1 in managed) {print $0}' \
    "$synthetic_root/etc/$name" | cmp -s "$account_backup/$name" -
done
first_passwd=$(sha256sum "$synthetic_root/etc/passwd" | awk '{print $1}')
first_group=$(sha256sum "$synthetic_root/etc/group" | awk '{print $1}')
first_shadow=$(sha256sum "$synthetic_root/etc/shadow" | awk '{print $1}')
first_gshadow=$(sha256sum "$synthetic_root/etc/gshadow" | awk '{print $1}')
phase=pin-prediction
{
  printf 'g hardened-blikvm %s -\n' "$shared_gid"
  printf 'u hardened-blikvm-broker %s "Hardened BliKVM platform broker" /nonexistent /usr/sbin/nologin\n' "$broker_uid"
  printf 'm hardened-blikvm-broker hardened-blikvm\n'
  printf 'u hardened-blikvm-video %s "Hardened BliKVM MJPEG proxy" /nonexistent /usr/sbin/nologin\n' "$video_uid"
  printf 'm hardened-blikvm-video hardened-blikvm\n'
  printf 'u hardened-blikvm-control %s "Hardened BliKVM control policy broker" /nonexistent /usr/sbin/nologin\n' "$control_uid"
  printf 'm hardened-blikvm-control hardened-blikvm\n'
  printf 'u hardened-blikvm-server %s "Hardened BliKVM network server" /nonexistent /usr/sbin/nologin\n' "$server_uid"
  printf 'm hardened-blikvm-server hardened-blikvm\n'
  printf 'u hardened-blikvm-update %s "Hardened BliKVM application update supervisor" /nonexistent /usr/sbin/nologin\n' "$update_uid"
  printf 'm hardened-blikvm-update hardened-blikvm\n'
} > "$pinned_sysusers"
chmod 600 "$pinned_sysusers"
rm -f -- "$synthetic_root/etc/passwd-" "$synthetic_root/etc/group-" \
  "$synthetic_root/etc/shadow-" "$synthetic_root/etc/gshadow-" "$synthetic_root/etc/.pwd.lock"
for name in passwd group shadow gshadow; do cp -a -- "$account_backup/$name" "$synthetic_root/etc/$name"; done
install -m 644 "$pinned_sysusers" "$synthetic_root/usr/lib/sysusers.d/50-runtime.conf"
phase=second-prediction
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
[ "$(sha256sum "$synthetic_root/etc/passwd" | awk '{print $1}')" = "$first_passwd" ]
[ "$(sha256sum "$synthetic_root/etc/group" | awk '{print $1}')" = "$first_group" ]
[ "$(sha256sum "$synthetic_root/etc/shadow" | awk '{print $1}')" = "$first_shadow" ]
[ "$(sha256sum "$synthetic_root/etc/gshadow" | awk '{print $1}')" = "$first_gshadow" ]
phase=state
record_aux() {
  key=$1
  name=$2
  if [ -e "$account_backup/$name" ]; then
    printf 'before_%s_present=yes\n' "$key"
    printf 'before_%s_hash=%s\n' "$key" "$(sha256sum "$account_backup/$name" | awk '{print $1}')"
    printf 'before_%s_meta=%s\n' "$key" "$(stat -c '%a:%u:%g:%h:%s' "$account_backup/$name")"
  else
    printf 'before_%s_present=no\n' "$key"
  fi
  [ -f "$synthetic_root/etc/$name" ]
  [ ! -L "$synthetic_root/etc/$name" ]
  printf 'after_%s_hash=%s\n' "$key" "$(sha256sum "$synthetic_root/etc/$name" | awk '{print $1}')"
  printf 'after_%s_meta=%s\n' "$key" "$(stat -c '%a:%u:%g:%h:%s' "$synthetic_root/etc/$name")"
}
{
  printf 'package_sha=%s\n' "$package_sha256"
  printf 'sysusers_sha=%s\n' "$sysusers_sha256"
  printf 'htpasswd_before=%s\n' "$(sha256sum "$account_backup/htpasswd" | awk '{print $1}')"
  printf 'nginx_before=%s\n' "$(nginx_tree_hash)"
  printf 'package_inventory_before=%s\n' "$(other_package_hash)"
  for name in passwd group shadow gshadow; do
    printf 'before_%s=%s\n' "$name" "$(sha256sum "$account_backup/$name" | awk '{print $1}')"
    printf 'after_%s=%s\n' "$name" "$(sha256sum "$synthetic_root/etc/$name" | awk '{print $1}')"
    printf 'before_meta_%s=%s\n' "$name" "$(stat -c '%a:%u:%g:%h:%s' "$account_backup/$name")"
    printf 'after_meta_%s=%s\n' "$name" "$(stat -c '%a:%u:%g:%h:%s' "$synthetic_root/etc/$name")"
  done
  printf 'uid_broker=%s\nuid_video=%s\nuid_control=%s\nuid_server=%s\nuid_update=%s\n' \
    "$broker_uid" "$video_uid" "$control_uid" "$server_uid" "$update_uid"
  printf 'gid_broker=%s\ngid_video=%s\ngid_control=%s\ngid_server=%s\ngid_update=%s\nshared_gid=%s\n' \
    "$broker_gid" "$video_gid" "$control_gid" "$server_gid" "$update_gid" "$shared_gid"
  record_aux passwd_backup passwd-
  record_aux group_backup group-
  record_aux shadow_backup shadow-
  record_aux gshadow_backup gshadow-
  record_aux pwd_lock .pwd.lock
} > "$state_file.part"
chmod 600 "$state_file.part"
sync "$state_file.part"
mv "$state_file.part" "$state_file"
printf '%s\n' prepared > "$phase_file"
chmod 600 "$phase_file"
phase=credential
python3 -c 'import os,sys; sys.stdout.write(os.getrandom(32, os.GRND_NONBLOCK).hex())' > "$password_file"
[ "$(wc -c < "$password_file" | tr -d ' ')" = 64 ]
grep -Eq '^[0-9a-f]{64}$' "$password_file"
chmod 400 "$password_file"
phase=rollback-scripts
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
printf '%s' "$watchdog_b64" | base64 -d > "$watchdog_script"
chmod 700 "$rollback_script" "$watchdog_script"
printf '%s' "$boot_unit_b64" | base64 -d > "$boot_unit_part"
[ "$(sha256sum "$boot_unit_part" | awk '{print $1}')" = "$boot_unit_sha256" ]
chmod 644 "$boot_unit_part"
mv "$boot_unit_part" "$boot_unit_path"
[ -d /etc/systemd/system/sysinit.target.wants ]
ln -s "../$boot_unit" "$boot_unit_link"
sync
systemctl daemon-reload
[ "$(systemctl is-enabled "$boot_unit" 2>/dev/null || true)" = enabled ]
phase=watchdog
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
sync
phase=complete
emit account_backups_verified yes
emit htpasswd_backup_verified yes
emit identities_predicted yes
emit prediction_reproduced yes
emit credential_generated yes
emit rollback_state_synced yes
emit boot_rollback_armed yes
emit watchdog_active yes
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def _state_helpers() -> str:
    return r"""
value() {
  awk -F= -v key="$1" '$1 == key {value=$2; count++} END {if (count == 1) print value; else exit 1}' "$state_file"
}
validate_digest() { case "$1" in *[!0-9a-f]*|'') return 1 ;; esac; [ "${#1}" = 64 ]; }
write_phase() {
  value=$1
  printf '%s\n' "$value" > "$phase_file.part"
  chmod 600 "$phase_file.part"
  sync "$phase_file.part"
  mv "$phase_file.part" "$phase_file"
  sync "$phase_file"
}
verify_identities() {
  for spec in \
    broker:hardened-blikvm-broker video:hardened-blikvm-video \
    control:hardened-blikvm-control server:hardened-blikvm-server \
    update:hardened-blikvm-update; do
    key=${spec%%:*}
    name=${spec#*:}
    expected_uid=$(value "uid_$key")
    expected_gid=$(value "gid_$key")
    entry=$(getent passwd "$name")
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $3}')" = "$expected_uid" ]
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $4}')" = "$expected_gid" ]
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $6}')" = /nonexistent ]
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $7}')" = /usr/sbin/nologin ]
    [ "$(getent group "$name" | awk -F: 'NF == 4 {print $3}')" = "$expected_gid" ]
  done
  expected_shared_gid=$(value shared_gid)
  [ "$(getent group hardened-blikvm | awk -F: 'NF == 4 {print $3}')" = "$expected_shared_gid" ]
  members=$(getent group hardened-blikvm | awk -F: '{gsub(/,/,"\n",$4); print $4}' | sort)
  expected_members=$(printf '%s\n' hardened-blikvm-broker hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update hardened-blikvm-video | sort)
  [ "$members" = "$expected_members" ]
}
verify_aux_after() {
  for spec in passwd_backup:passwd- group_backup:group- shadow_backup:shadow- \
    gshadow_backup:gshadow- pwd_lock:.pwd.lock; do
    key=${spec%%:*}
    name=${spec#*:}
    [ -f "/etc/$name" ]
    [ ! -L "/etc/$name" ]
    [ "$(sha256sum "/etc/$name" | awk '{print $1}')" = "$(value "after_${key}_hash")" ]
    [ "$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")" = "$(value "after_${key}_meta")" ]
  done
}
verify_htpasswd_after() {
  htpasswd_before=$(value htpasswd_before)
  validate_digest "$htpasswd_before"
  current=$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')
  [ "$current" != "$htpasswd_before" ]
  count=$(awk -F: -v account="$kvmd_account" '$1 == account {count++} END {print count+0}' /etc/kvmd/htpasswd)
  [ "$count" = 1 ]
  if [ -e "$filtered_htpasswd" ]; then
    [ -f "$filtered_htpasswd" ]
    [ ! -L "$filtered_htpasswd" ]
    rm -f -- "$filtered_htpasswd"
  fi
  awk -F: -v account="$kvmd_account" '$1 != account {print $0}' /etc/kvmd/htpasswd > "$filtered_htpasswd"
  chmod 600 "$filtered_htpasswd"
  cmp -s "$account_backup/htpasswd" "$filtered_htpasswd"
}
verify_config() {
  [ -d "$config_root" ]
  [ ! -L "$config_root" ]
  [ "$(stat -c '%a:%u:%g' "$config_root")" = 700:0:0 ]
  unexpected=$(find "$config_root" -mindepth 1 -maxdepth 1 ! -name kvmd-user ! -name kvmd-password -print -quit)
  [ -z "$unexpected" ]
  for path in "$username_path" "$password_path"; do
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h' "$path")" = 400:0:0:1 ]
  done
  [ "$(wc -c < "$username_path" | tr -d ' ')" = 24 ]
  [ "$(cat "$username_path")" = "$kvmd_account" ]
  [ "$(wc -c < "$password_path" | tr -d ' ')" = 64 ]
  grep -Eq '^[0-9a-f]{64}$' "$password_path"
  cmp -s "$password_file" "$password_path"
  broker_uid=$(value uid_broker)
  broker_gid=$(value gid_broker)
  if setpriv --reuid="$broker_uid" --regid="$broker_gid" --clear-groups --no-new-privs \
    cat "$password_path" >/dev/null 2>&1; then return 1; fi
}
authenticate_kvmd() {
  status=$(
    {
      printf 'header = "X-KVMD-User: %s"\n' "$kvmd_account"
      printf 'header = "X-KVMD-Passwd: '
      cat "$password_file"
      printf '"\n'
    } | curl --silent --max-time 8 --config - --unix-socket /run/kvmd/kvmd.sock \
      --output /dev/null --write-out '%{http_code}' http://localhost/info 2>/dev/null
  )
  [ "$status" = 200 ]
}
""".strip()


def build_apply_script(paths: RemotePaths, assets: Assets) -> str:
    values = _assignments(
        {
            "rollback_root": paths.root,
            "marker": paths.marker,
            "marker_value": (
                f"{paths.run_id}:{assets.package.sha256}:{assets.sysusers.sha256}"
            ),
            "state_file": paths.state,
            "phase_file": paths.phase,
            "account_backup": paths.account_backup,
            "pinned_sysusers": paths.pinned_sysusers,
            "password_file": paths.password,
            "filtered_htpasswd": paths.filtered_htpasswd,
            "rollback_script": paths.rollback_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "username_path": policy.USERNAME_PATH,
            "password_path": policy.PASSWORD_PATH,
            "kvmd_account": policy.KVMD_ACCOUNT,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=validate
run_succeeded=no
cleanup_state=retained
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    if /bin/sh "$rollback_script" explicit >/dev/null 2>&1; then cleanup_state=restored; fi
    emit apply_failure_phase "$phase"
    emit apply_cleanup "$cleanup_state"
  fi
  exit "$code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(cat "$phase_file")" = prepared ]
[ -f "$rollback_script" ]
[ -x "$rollback_script" ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
[ -f "$boot_unit_path" ]
[ -L "$boot_unit_link" ]
package_installed_exact
units_loaded_disabled_inactive
runtime_accounts_absent
[ ! -e "$config_root" ]
[ ! -e "$config_stage" ]
for name in passwd group shadow gshadow; do
  [ "$(sha256sum "/etc/$name" | awk '{print $1}')" = "$(value "before_$name")" ]
done
[ "$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')" = "$(value htpasswd_before)" ]
for process in vipw vigr useradd groupadd usermod groupmod; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
pgrep -f '^systemd-sysusers( |$)' >/dev/null 2>&1 && exit 1
phase=sysusers
systemd-sysusers "$pinned_sysusers" >/dev/null 2>&1
for name in passwd group shadow gshadow; do
  [ "$(sha256sum "/etc/$name" | awk '{print $1}')" = "$(value "after_$name")" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")" = "$(value "after_meta_$name")" ]
done
verify_aux_after
verify_identities
write_phase sysusers
phase=kvmd-account
pgrep -x kvmd-htpasswd >/dev/null 2>&1 && exit 1
kvmd-htpasswd set -i -q "$kvmd_account" < "$password_file" >/dev/null 2>&1
verify_htpasswd_after
authenticate_kvmd
write_phase htpasswd
phase=config
[ ! -e "$config_stage" ]
mkdir -m 700 "$config_stage"
printf '%s\n' "$kvmd_account" > "$config_stage/kvmd-user"
install -m 0400 "$password_file" "$config_stage/kvmd-password"
chown 0:0 "$config_stage/kvmd-user" "$config_stage/kvmd-password"
chmod 0400 "$config_stage/kvmd-user" "$config_stage/kvmd-password"
sync "$config_stage/kvmd-user" "$config_stage/kvmd-password" "$config_stage"
mv "$config_stage" "$config_root"
sync /etc
verify_config
write_phase config
phase=final-checks
verify_identities
verify_aux_after
verify_htpasswd_after
authenticate_kvmd
verify_config
package_installed_exact
units_loaded_disabled_inactive
runtime_state_roots_absent
[ -z "$(dpkg --audit 2>/dev/null)" ]
[ "$(other_package_hash)" = "$(value package_inventory_before)" ]
[ "$(nginx_tree_hash)" = "$(value nginx_before)" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
sync
run_succeeded=yes
trap - EXIT HUP INT TERM
emit account_databases_predicted yes
emit accounts_provisioned yes
emit dedicated_kvmd_authenticated yes
emit htpasswd_transition_exact yes
emit config_credentials_provisioned yes
emit credential_sources_private yes
emit package_inventory_unchanged yes
emit nginx_unchanged yes
emit unit_templates_inactive yes
emit ports_free_after_apply yes
""".strip()
    return (
        f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{_state_helpers()}\n{body}"
    )


def build_commit_script(paths: RemotePaths, assets: Assets) -> str:
    boot_unit = build_boot_unit(paths)
    values = _assignments(
        {
            "rollback_root": paths.root,
            "marker": paths.marker,
            "marker_value": (
                f"{paths.run_id}:{assets.package.sha256}:{assets.sysusers.sha256}"
            ),
            "state_file": paths.state,
            "phase_file": paths.phase,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "pinned_sysusers": paths.pinned_sysusers,
            "password_file": paths.password,
            "filtered_htpasswd": paths.filtered_htpasswd,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_link": paths.boot_unit_link,
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
            "watchdog_unit": paths.watchdog_unit,
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "username_path": policy.USERNAME_PATH,
            "password_path": policy.PASSWORD_PATH,
            "kvmd_account": policy.KVMD_ACCOUNT,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(cat "$phase_file")" = config ]
[ -f "$boot_unit_path" ]
[ ! -L "$boot_unit_path" ]
[ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
[ -L "$boot_unit_link" ]
[ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
for name in passwd group shadow gshadow; do
  [ "$(sha256sum "/etc/$name" | awk '{print $1}')" = "$(value "after_$name")" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "/etc/$name")" = "$(value "after_meta_$name")" ]
done
verify_aux_after
verify_identities
verify_htpasswd_after
authenticate_kvmd
verify_config
package_installed_exact
units_loaded_disabled_inactive
runtime_state_roots_absent
[ -z "$(dpkg --audit 2>/dev/null)" ]
[ "$(other_package_hash)" = "$(value package_inventory_before)" ]
[ "$(nginx_tree_hash)" = "$(value nginx_before)" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
sync
systemctl stop "$watchdog_unit" >/dev/null 2>&1
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
rm -f -- "$boot_unit_link" "$boot_unit_path"
systemctl daemon-reload
remove_rollback_tree
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_link" ]
package_installed_exact
units_loaded_disabled_inactive
[ -d "$config_root" ]
emit package_provisioned yes
emit watchdog_removed yes
emit boot_rollback_removed yes
emit rollback_root_removed yes
""".strip()
    return (
        f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{REMOVE_ROLLBACK_TREE}\n{_state_helpers()}\n{body}"
    )


FINAL_STATE_FUNCTIONS = r"""
verify_identities_final() {
  ids=
  gids=
  for name in hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update; do
    entry=$(getent passwd "$name")
    uid=$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $3}')
    gid=$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $4}')
    case "$uid:$gid" in *[!0-9:]*|0:*|*:0|:) return 1 ;; esac
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $6}')" = /nonexistent ]
    [ "$(printf '%s\n' "$entry" | awk -F: 'NF == 7 {print $7}')" = /usr/sbin/nologin ]
    [ "$(getent group "$name" | awk -F: 'NF == 4 {print $3}')" = "$gid" ]
    awk -F: -v id="$uid" -v name="$name" '$3 == id && $1 != name {bad=1} END {exit bad}' /etc/passwd
    awk -F: -v id="$gid" -v name="$name" '$3 == id && $1 != name {bad=1} END {exit bad}' /etc/group
    ids="$ids $uid"
    gids="$gids $gid"
  done
  shared_gid=$(getent group hardened-blikvm | awk -F: 'NF == 4 {print $3}')
  case "$shared_gid" in ''|0|*[!0-9]*) return 1 ;; esac
  awk -F: -v id="$shared_gid" '$3 == id && $1 != "hardened-blikvm" {bad=1} END {exit bad}' /etc/group
  [ "$(printf '%s\n' $ids | sort -u | wc -l | tr -d ' ')" = 5 ]
  [ "$(printf '%s\n' $gids "$shared_gid" | sort -u | wc -l | tr -d ' ')" = 6 ]
  members=$(getent group hardened-blikvm | awk -F: '{gsub(/,/,"\n",$4); print $4}' | sort)
  expected_members=$(printf '%s\n' hardened-blikvm-broker hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update hardened-blikvm-video | sort)
  [ "$members" = "$expected_members" ]
}
verify_config_final() {
  [ -d "$config_root" ]
  [ ! -L "$config_root" ]
  [ "$(stat -c '%a:%u:%g' "$config_root")" = 700:0:0 ]
  unexpected=$(find "$config_root" -mindepth 1 -maxdepth 1 ! -name kvmd-user ! -name kvmd-password -print -quit)
  [ -z "$unexpected" ]
  for path in "$username_path" "$password_path"; do
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%a:%u:%g:%h' "$path")" = 400:0:0:1 ]
  done
  [ "$(wc -c < "$username_path" | tr -d ' ')" = 24 ]
  [ "$(cat "$username_path")" = "$kvmd_account" ]
  [ "$(wc -c < "$password_path" | tr -d ' ')" = 64 ]
  grep -Eq '^[0-9a-f]{64}$' "$password_path"
  broker_uid=$(getent passwd hardened-blikvm-broker | awk -F: '{print $3}')
  broker_gid=$(getent passwd hardened-blikvm-broker | awk -F: '{print $4}')
  if setpriv --reuid="$broker_uid" --regid="$broker_gid" --clear-groups --no-new-privs \
    cat "$password_path" >/dev/null 2>&1; then return 1; fi
}
authenticate_kvmd_final() {
  status=$(
    {
      printf 'header = "X-KVMD-User: %s"\n' "$kvmd_account"
      printf 'header = "X-KVMD-Passwd: '
      cat "$password_path"
      printf '"\n'
    } | curl --silent --max-time 8 --config - --unix-socket /run/kvmd/kvmd.sock \
      --output /dev/null --write-out '%{http_code}' http://localhost/info 2>/dev/null
  )
  [ "$status" = 200 ]
}
""".strip()


def build_final_script(paths: RemotePaths, assets: Assets) -> str:
    tree_sha256, _ = retain.dedicated_tree(assets.package)
    installed_checks = retain.installed_checks(assets.package)
    cap_checks = retain.capability_checks()
    values = _assignments(
        {
            "rollback_root": paths.root,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "username_path": policy.USERNAME_PATH,
            "password_path": policy.PASSWORD_PATH,
            "kvmd_account": policy.KVMD_ACCOUNT,
            "package_name": debian.PACKAGE_NAME,
            "package_version": debian.PACKAGE_VERSION,
            "package_architecture": debian.PACKAGE_ARCHITECTURE,
            "expected_tree_sha256": tree_sha256,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
package_installed_exact
INSTALLED_CHECKS
actual_tree_sha256=$(find /usr/libexec/hardened-blikvm /usr/share/hardened-blikvm \
  /usr/share/doc/hardened-blikvm-runtime -xdev -printf '%y %p\n' | sort | sha256sum | awk '{print $1}')
[ "$actual_tree_sha256" = "$expected_tree_sha256" ]
CAPABILITY_CHECKS
verify_identities_final
verify_config_final
if ! kvmd-htpasswd list 2>/dev/null | grep -Fxq "$kvmd_account"; then exit 1; fi
authenticate_kvmd_final
units_loaded_disabled_inactive
runtime_state_roots_absent
[ ! -e "$config_stage" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-provision-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-provision-*' -print -quit)" ]
[ -z "$(dpkg --audit 2>/dev/null)" ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit package_installed_final yes
emit accounts_provisioned_final yes
emit config_credentials_private_final yes
emit dedicated_kvmd_authenticated_final yes
emit unit_templates_inactive_final yes
emit state_roots_absent_final yes
emit gates_absent_final yes
emit dpkg_audit_clean_final yes
emit ports_free_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit rootfs_mode_final "$rootfs_mode"
""".strip().replace("INSTALLED_CHECKS", installed_checks).replace(
        "CAPABILITY_CHECKS", cap_checks
    )
    return (
        f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{FINAL_STATE_FUNCTIONS}\n{body}"
    )


def build_cleanup_script(paths: RemotePaths, assets: Assets) -> str:
    boot_unit = build_boot_unit(paths)
    values = _assignments(
        {
            "rollback_root": paths.root,
            "marker": paths.marker,
            "marker_value": (
                f"{paths.run_id}:{assets.package.sha256}:{assets.sysusers.sha256}"
            ),
            "state_file": paths.state,
            "phase_file": paths.phase,
            "account_backup": paths.account_backup,
            "synthetic_root": paths.synthetic_root,
            "sysusers_file": paths.sysusers,
            "pinned_sysusers": paths.pinned_sysusers,
            "password_file": paths.password,
            "filtered_htpasswd": paths.filtered_htpasswd,
            "rollback_script": paths.rollback_script,
            "watchdog_script": paths.watchdog_script,
            "boot_unit": paths.boot_unit,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
            "watchdog_unit": paths.watchdog_unit,
            "config_root": policy.CONFIG_ROOT,
            "config_stage": paths.config_stage,
            "kvmd_account": policy.KVMD_ACCOUNT,
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
if [ -f "$rollback_script" ] && [ -f "$state_file" ] && [ -f "$marker" ]; then
  /bin/sh "$rollback_script" explicit >/dev/null 2>&1
else
  package_installed_exact
  units_loaded_disabled_inactive
  runtime_accounts_absent
  [ ! -e "$config_root" ]
  [ ! -e "$config_stage" ]
  if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$kvmd_account"; then exit 1; fi
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
    [ "$(stat -c '%u:%g:%h' "$boot_unit_part")" = 0:0:1 ]
    [ "$(stat -c '%s' "$boot_unit_part")" -le 4096 ]
    rm -f -- "$boot_unit_part"
  fi
  systemctl daemon-reload >/dev/null 2>&1
  if [ -e "$rollback_root" ]; then
    [ -d "$rollback_root" ]
    [ ! -L "$rollback_root" ]
    [ "$(stat -c '%a:%u:%g' "$rollback_root")" = 700:0:0 ]
    if [ -e "$marker" ]; then
      [ -f "$marker" ]
      [ "$(cat "$marker")" = "$marker_value" ]
      remove_rollback_tree
    else
      [ -z "$(find "$rollback_root" -mindepth 1 -print -quit)" ]
      rmdir "$rollback_root"
    fi
  fi
fi
package_installed_exact
units_loaded_disabled_inactive
runtime_accounts_absent
[ ! -e "$config_root" ]
[ ! -e "$config_stage" ]
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$kvmd_account"; then exit 1; fi
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
emit cleanup_restored yes
""".strip()
    return (
        f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{REMOVE_ROLLBACK_TREE}\n{body}"
    )


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    run_id = secrets.token_hex(8)
    paths = remote_paths(assets.sysusers.sha256, run_id)
    retained_paths = retain.remote_paths(assets.package.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    committed_state = False
    try:
        retained_result = prototype.run_ssh(
            target,
            secret,
            retain.build_final_script(retained_paths, assets.package),
            timeout=45,
        )
        if retained_result.returncode != 0:
            raise ValueError("retained package precondition failed")
        retained_before = retain.parse_facts(retained_result.stdout, retain.FINAL_KEYS)
        preflight_result = prototype.run_ssh(
            target, secret, build_preflight_script(paths, assets), timeout=60
        )
        if preflight_result.returncode != 0:
            phase, _ = parse_failure_phase(preflight_result.stdout, "preflight")
            raise ValueError(f"runtime provisioning preflight failed at {phase}")
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        cleanup_required = True
        prepare_result = prototype.run_ssh(
            target, secret, build_prepare_script(paths, assets), timeout=180
        )
        if prepare_result.returncode != 0:
            phase, _ = parse_failure_phase(prepare_result.stdout, "prepare")
            raise ValueError(f"runtime provisioning prepare failed at {phase}")
        prepared = parse_facts(prepare_result.stdout, PREPARE_KEYS)
        apply_result = prototype.run_ssh(
            target, secret, build_apply_script(paths, assets), timeout=180
        )
        if apply_result.returncode != 0:
            phase, remote_cleanup = parse_failure_phase(apply_result.stdout, "apply")
            if remote_cleanup is True:
                cleanup_required = False
            raise ValueError(f"runtime provisioning apply failed at {phase}")
        applied = parse_facts(apply_result.stdout, APPLY_KEYS)
        commit_result = prototype.run_ssh(
            target, secret, build_commit_script(paths, assets), timeout=120
        )
        if commit_result.returncode == 0:
            committed = parse_facts(commit_result.stdout, COMMIT_KEYS)
            cleanup_required = False
            committed_state = True
        else:
            recovery_result = prototype.run_ssh(
                target, secret, build_final_script(paths, assets), timeout=60
            )
            if recovery_result.returncode != 0:
                raise ValueError("runtime provisioning commit failed")
            _ = parse_facts(recovery_result.stdout, FINAL_KEYS)
            committed = {key: "yes" for key in sorted(COMMIT_KEYS)}
            cleanup_required = False
            committed_state = True
        final_result = prototype.run_ssh(
            target, secret, build_final_script(paths, assets), timeout=60
        )
        if final_result.returncode != 0:
            raise ValueError("runtime provisioning final state failed")
        final_state = parse_facts(final_result.stdout, FINAL_KEYS)
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-gate",
            "result": "passed",
            "package": {
                "name": debian.PACKAGE_NAME,
                "version": debian.PACKAGE_VERSION,
                "architecture": debian.PACKAGE_ARCHITECTURE,
                "sha256": assets.package.sha256,
                "sourceCommit": assets.package.source_commit,
                "sourceBundleSha256": assets.package.source_bundle_sha256,
                "builderCommit": assets.package.builder_commit,
            },
            "identityPolicy": {
                "sysusersSha256": assets.sysusers.sha256,
                "accountCount": len(policy.ACCOUNT_NAMES),
                "groupCount": len(policy.GROUP_NAMES),
                "dedicatedKvmdAccount": policy.KVMD_ACCOUNT,
                "callerSelectedIdentity": False,
            },
            "policy": {
                "packageRetained": True,
                "accountsProvisioned": True,
                "credentialsProvisioned": True,
                "servicesActive": False,
                "servicesEnabled": False,
                "nginxMutation": False,
                "updateRuntime": "absent",
                "diagnosticPersistence": "absent",
                "hardwareMutation": "absent",
                "physicalActionsInvoked": False,
                "secretEmitted": False,
            },
            "retainedBefore": retained_before,
            "preflight": preflight,
            "prepared": prepared,
            "applied": applied,
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
        cleanup: CleanupReceipt = {
            "attempted": False,
            "confirmed": not cleanup_required and not committed_state,
        }
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target, secret, build_cleanup_script(paths, assets), timeout=180
                )
                facts = (
                    parse_facts(result.stdout, CLEANUP_KEYS)
                    if result.returncode == 0
                    else {}
                )
                cleanup = {
                    "attempted": True,
                    "confirmed": facts == {"cleanup_restored": "yes"},
                }
            except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
                cleanup = {"attempted": True, "confirmed": False}
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-gate",
            "result": "failed",
            "package": {
                "sha256": assets.package.sha256,
                "sourceCommit": assets.package.source_commit,
                "sourceBundleSha256": assets.package.source_bundle_sha256,
                "builderCommit": assets.package.builder_commit,
            },
            "failure": {
                "stage": str(error)[:96],
                "failureType": type(error).__name__,
                "committedStatePossible": committed_state,
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
