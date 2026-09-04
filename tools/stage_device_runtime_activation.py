#!/usr/bin/env python3
"""Reversibly activate and test the installed read-only runtime services."""

from __future__ import annotations

import argparse
import base64
from collections.abc import Mapping
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

from tools import runtime_provisioning_policy as identity_policy
from tools import stage_device_runtime_provision as provision
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


BROKER_UNIT = "hardened-blikvm-platform-broker.service"
VIDEO_UNIT = "hardened-blikvm-video-proxy.service"
SERVER_UNIT = "hardened-blikvm-server.service"
CONTROL_UNIT = "hardened-blikvm-control-broker.service"
CONTROL_SOCKET_UNIT = "hardened-blikvm-control-broker.socket"
TEST_ADMIN = "hardened-blikvm-test-admin"
STATE_ROOT = "/var/lib/hardened-blikvm"
SERVER_DROPIN_DIR = "/run/systemd/system/hardened-blikvm-server.service.d"
SERVER_DROPIN = f"{SERVER_DROPIN_DIR}/native-gate.conf"
MAX_PRIVATE_ARTIFACT_BYTES = 8 * 1024 * 1024
JPEG_COUNTER_AWK = (
    "{for (field = 1; field <= NF; field++) "
    "{if (previous == 255 && $field == 216) frames++; previous = $field}} "
    "END {print frames + 0}"
)
PREFLIGHT_KEYS = {
    "activation_gates_absent",
    "architecture",
    "control_boundary_absent",
    "entropy_ready",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "ports_free_before",
    "private_artifacts_absent",
    "rootfs_mode_before",
    "runtime_roots_absent",
    "tools_ready",
    "units_inactive_before",
}
PREPARE_KEYS = {
    "boot_rollback_armed",
    "rollback_state_synced",
    "test_password_generated",
    "watchdog_active",
}
APPLY_KEYS = {
    "abstract_sockets_absent",
    "broker_active",
    "broker_isolation_verified",
    "control_boundary_absent",
    "diagnostics_current_only",
    "diagnostics_sanitized",
    "diagnostics_status",
    "inventory_sanitized",
    "inventory_status",
    "mjpeg_frames",
    "mjpeg_reconnected",
    "persistent_credentials_private",
    "private_artifacts_bounded",
    "server_active",
    "server_isolation_verified",
    "session_revoked_stream",
    "setup_status",
    "tcp_listener_loopback_only",
    "udp_sockets_absent",
    "video_active",
    "video_isolation_verified",
}
COMMIT_KEYS = {
    "boot_rollback_removed",
    "cgroups_empty",
    "dropin_removed",
    "private_artifacts_removed",
    "runtime_roots_removed",
    "socket_tail_clean",
    "state_root_removed",
    "units_inactive",
    "watchdog_removed",
}
CLEANUP_KEYS = {"cleanup_restored"}
ALLOWED_FACT_VALUES = {
    "yes",
    "active",
    "aarch64",
    "rw",
    "200",
    "401",
}


@dataclass(frozen=True)
class RemotePaths:
    run_id: str
    root: str
    marker: str
    password: str
    request: str
    setup_response: str
    login_response: str
    cookie: str
    logout_config: str
    inventory: str
    diagnostics: str
    video_first: str
    video_second: str
    video_revoked: str
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


def _provision_options(options: Options) -> provision.Options:
    return provision.Options(
        options.endpoint,
        options.username,
        options.known_hosts,
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
        options.output,
    )


def validate_assets(options: Options) -> provision.Assets:
    return provision.validate_assets(_provision_options(options))


def remote_paths(package_sha256: str, run_id: str) -> RemotePaths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("runtime activation identity is invalid")
    root = (
        "/var/lib/hardened-blikvm-activation-rollback-"
        f"{package_sha256[:12]}-{run_id}"
    )
    boot_unit = f"hardened-blikvm-activation-rollback-{run_id}.service"
    return RemotePaths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        password=f"{root}/web-password",
        request=f"{root}/account-request.json",
        setup_response=f"{root}/setup.json",
        login_response=f"{root}/login.json",
        cookie=f"{root}/session.cookies",
        logout_config=f"{root}/logout.curl",
        inventory=f"{root}/inventory.json",
        diagnostics=f"{root}/diagnostics.json",
        video_first=f"{root}/video-first.mjpeg",
        video_second=f"{root}/video-second.mjpeg",
        video_revoked=f"{root}/video-revoked.mjpeg",
        rollback_script=f"{root}/rollback.sh",
        watchdog_script=f"{root}/watchdog.sh",
        boot_unit=boot_unit,
        boot_unit_path=f"/etc/systemd/system/{boot_unit}",
        boot_unit_part=f"/etc/systemd/system/{boot_unit}.part",
        boot_unit_link=f"/etc/systemd/system/sysinit.target.wants/{boot_unit}",
        watchdog_unit=f"hardened-blikvm-activation-watchdog-{run_id}.service",
    )


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def _encoded(payload: str) -> str:
    return base64.b64encode(payload.encode()).decode("ascii")


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("runtime activation output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in ALLOWED_FACT_VALUES
        ):
            raise ValueError("runtime activation returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("runtime activation result was incomplete")
    return dict(sorted(facts.items()))


def parse_failure_phase(raw: bytes, prefix: str) -> tuple[str, bool | None]:
    if len(raw) > 2048:
        raise ValueError("runtime activation failure output exceeded the limit")
    facts: dict[str, str] = {}
    allowed = {f"{prefix}_failure_phase", f"{prefix}_cleanup"}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in allowed or key in facts:
            raise ValueError("runtime activation failure output is invalid")
        facts[key] = value
    phase = facts.get(f"{prefix}_failure_phase", "")
    if not re.fullmatch(r"[a-z-]{1,48}", phase):
        raise ValueError("runtime activation failure phase is invalid")
    cleanup_value = facts.get(f"{prefix}_cleanup")
    if cleanup_value is None:
        cleanup = None
    elif cleanup_value in {"restored", "retained"}:
        cleanup = cleanup_value == "restored"
    else:
        raise ValueError("runtime activation cleanup fact is invalid")
    return phase, cleanup


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


PORT_FUNCTION = provision.PORT_FUNCTION

STATE_FUNCTIONS = r"""
units_inactive() {
  for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
    [ "$(systemctl is-enabled "$unit" 2>/dev/null || true)" = disabled ] || return 1
  done
}
units_active() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = active ] || return 1
    [ "$(systemctl is-enabled "$unit" 2>/dev/null || true)" = disabled ] || return 1
  done
}
control_boundary_absent() {
  [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]
  [ "$(systemctl is-enabled hardened-blikvm-control-broker.service 2>/dev/null || true)" = disabled ]
  [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]
  [ "$(systemctl is-active hardened-blikvm-control-broker.socket 2>/dev/null || true)" != active ]
  [ "$(systemctl is-enabled hardened-blikvm-control-broker.socket 2>/dev/null || true)" != enabled ]
  [ ! -e /run/hardened-blikvm-control ]
}
private_artifacts_absent() {
  for root in /tmp /var/tmp /dev/shm; do
    [ -z "$(find "$root" -maxdepth 2 -name '*hardened-blikvm*' -print -quit 2>/dev/null)" ] || return 1
  done
}
cgroup_empty() {
  unit=$1
  group=$(systemctl show -p ControlGroup --value "$unit" 2>/dev/null || true)
  [ -z "$group" ] || [ ! -e "/sys/fs/cgroup$group/cgroup.procs" ] || [ ! -s "/sys/fs/cgroup$group/cgroup.procs" ]
}
socket_tail_clean() {
  for table in /proc/net/tcp /proc/net/tcp6; do
    awk '$2 ~ /:46A0$/ && ($4 == "05" || $4 == "08") {bad=1} END {exit bad}' "$table"
  done
}
""".strip()


def build_dropin() -> str:
    return "[Service]\nEnvironment=BLI_KVM_SECURE_COOKIES=0\n"


def build_preflight_script(paths: RemotePaths, assets: provision.Assets) -> str:
    values = _assignments(
        {
            "rollback_root": paths.root,
            "boot_unit_path": paths.boot_unit_path,
            "boot_unit_part": paths.boot_unit_part,
            "boot_unit_link": paths.boot_unit_link,
            "watchdog_unit": paths.watchdog_unit,
            "state_root": STATE_ROOT,
            "dropin_dir": SERVER_DROPIN_DIR,
            "dropin_path": SERVER_DROPIN,
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
for tool in awk base64 cat chmod chown cmp curl cut df find findmnt getent grep head id install kill mkdir mv od pgrep python3 readlink rm rmdir sed setpriv sha256sum sleep sort stat sync systemctl systemd-run tail tr wait wc xargs; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
phase=rootfs
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=services
units_inactive
control_boundary_absent
[ ! -e "$state_root" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
[ ! -e "$dropin_path" ]
[ ! -e "$dropin_dir" ]
phase=ports
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
socket_tail_clean
phase=artifacts
private_artifacts_absent
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-activation-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-activation-*' -print -quit)" ]
phase=kvmd
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ -S /run/kvmd/kvmd.sock ]
[ -S /run/kvmd/ustreamer.sock ]
phase=entropy
python3 -c 'import os; value = os.getrandom(1, os.GRND_NONBLOCK); assert len(value) == 1'
phase=complete
emit architecture aarch64
emit tools_ready yes
emit rootfs_mode_before "$rootfs_mode"
emit units_inactive_before yes
emit control_boundary_absent yes
emit runtime_roots_absent yes
emit ports_free_before yes
emit private_artifacts_absent yes
emit activation_gates_absent yes
emit entropy_ready yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n{body}"


def build_boot_unit(paths: RemotePaths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM activation emergency rollback",
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


REMOVE_PRIVATE_ROOT = r"""
remove_private_root() {
  [ -d "$rollback_root" ]
  [ ! -L "$rollback_root" ]
  [ "$(stat -c '%a:%u:%g' "$rollback_root")" = 700:0:0 ]
  unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
    ! -name .owner ! -name web-password ! -name account-request.json \
    ! -name setup.json ! -name login.json ! -name session.cookies \
    ! -name logout.curl ! -name inventory.json ! -name diagnostics.json \
    ! -name video-first.mjpeg ! -name video-second.mjpeg ! -name video-revoked.mjpeg \
    ! -name rollback.sh ! -name watchdog.sh -print -quit)
  [ -z "$unexpected" ]
  for path in "$password_file" "$request_file" "$setup_response" "$login_response" \
    "$cookie_file" "$logout_config" "$inventory_file" "$diagnostics_file" \
    "$video_first" "$video_second" "$video_revoked"; do
    if [ -e "$path" ]; then
      [ -f "$path" ]
      [ ! -L "$path" ]
      [ "$(stat -c '%u:%g:%h' "$path")" = 0:0:1 ]
      [ "$(stat -c '%s' "$path")" -le 8388608 ]
      rm -f -- "$path"
    fi
  done
  rm -f -- "$watchdog_script" "$rollback_script" "$marker"
  rmdir "$rollback_root"
}
""".strip()


def _path_assignments(paths: RemotePaths) -> dict[str, str]:
    return {
        "rollback_root": paths.root,
        "marker": paths.marker,
        "password_file": paths.password,
        "request_file": paths.request,
        "setup_response": paths.setup_response,
        "login_response": paths.login_response,
        "cookie_file": paths.cookie,
        "logout_config": paths.logout_config,
        "inventory_file": paths.inventory,
        "diagnostics_file": paths.diagnostics,
        "video_first": paths.video_first,
        "video_second": paths.video_second,
        "video_revoked": paths.video_revoked,
        "rollback_script": paths.rollback_script,
        "watchdog_script": paths.watchdog_script,
        "boot_unit": paths.boot_unit,
        "boot_unit_path": paths.boot_unit_path,
        "boot_unit_link": paths.boot_unit_link,
        "watchdog_unit": paths.watchdog_unit,
    }


def build_rollback_script(paths: RemotePaths, assets: provision.Assets) -> str:
    dropin = build_dropin()
    boot_unit = build_boot_unit(paths)
    values = {
        **_path_assignments(paths),
        "marker_value": f"{paths.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "dropin_dir": SERVER_DROPIN_DIR,
        "dropin_path": SERVER_DROPIN,
        "dropin_sha256": hashlib.sha256(dropin.encode()).hexdigest(),
        "state_root": STATE_ROOT,
        "server_user": "hardened-blikvm-server",
        "shared_group": identity_policy.SHARED_GROUP,
    }
    body = r"""
set -eu
export LC_ALL=C
[ "$#" = 1 ]
case "$1" in boot|watchdog|explicit) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do
  systemctl stop "$unit" >/dev/null 2>&1 || true
done
count=0
while [ "$count" -lt 100 ]; do
  if units_inactive && cgroup_empty hardened-blikvm-server.service && \
     cgroup_empty hardened-blikvm-video-proxy.service && \
     cgroup_empty hardened-blikvm-platform-broker.service; then break; fi
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
if [ -e "$state_root" ]; then
  server_uid=$(getent passwd "$server_user" | awk -F: '{print $3}')
  shared_gid=$(getent group "$shared_group" | awk -F: '{print $3}')
  [ -d "$state_root" ]
  [ ! -L "$state_root" ]
  [ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
  unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$state_root/account.json" ]; then
    [ -f "$state_root/account.json" ]
    [ ! -L "$state_root/account.json" ]
    [ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
    [ "$(stat -c '%s' "$state_root/account.json")" -le 4096 ]
    rm -f -- "$state_root/account.json"
  fi
  for path in "$state_root"/.account-*; do
    [ -e "$path" ] || continue
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%u:%g:%h' "$path")" = "$server_uid:$shared_gid:1" ]
    [ "$(stat -c '%s' "$path")" -le 4096 ]
    rm -f -- "$path"
  done
  rmdir "$state_root"
fi
if [ -e "$dropin_path" ]; then
  [ -f "$dropin_path" ]
  [ ! -L "$dropin_path" ]
  [ "$(stat -c '%a:%u:%g:%h' "$dropin_path")" = 644:0:0:1 ]
  [ "$(sha256sum "$dropin_path" | awk '{print $1}')" = "$dropin_sha256" ]
  rm -f -- "$dropin_path"
fi
if [ -d "$dropin_dir" ]; then rmdir "$dropin_dir"; fi
systemctl daemon-reload >/dev/null 2>&1
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
control_boundary_absent
private_artifacts_absent
socket_tail_clean
if [ -e "$boot_unit_link" ]; then
  [ -L "$boot_unit_link" ]
  [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
  rm -f -- "$boot_unit_link"
fi
if [ -e "$boot_unit_path" ]; then
  [ -f "$boot_unit_path" ]
  [ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
  rm -f -- "$boot_unit_path"
fi
systemctl daemon-reload >/dev/null 2>&1
remove_private_root
""".strip()
    return (
        f"#!/bin/sh\n{_assignments(values)}\n{PORT_FUNCTION}\n"
        f"{STATE_FUNCTIONS}\n{REMOVE_PRIVATE_ROOT}\n{body}\n"
    )


def _prepare_material(
    paths: RemotePaths, assets: provision.Assets
) -> tuple[str, str, str]:
    boot_unit = build_boot_unit(paths)
    rollback = build_rollback_script(paths, assets)
    watchdog = (
        "#!/bin/sh\nset -eu\nsleep 600\nexec /bin/sh "
        f"{shlex.quote(paths.rollback_script)} watchdog\n"
    )
    return boot_unit, rollback, watchdog


def build_prepare_script(paths: RemotePaths, assets: provision.Assets) -> str:
    boot_unit, rollback, watchdog = _prepare_material(paths, assets)
    values = {
        **_path_assignments(paths),
        "marker_value": f"{paths.run_id}:{assets.package.sha256}",
        "boot_unit_part": paths.boot_unit_part,
        "boot_unit_b64": _encoded(boot_unit),
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "rollback_b64": _encoded(rollback),
        "watchdog_b64": _encoded(watchdog),
    }
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=root
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
phase=password
python3 -c 'import os,sys; sys.stdout.write(os.getrandom(32, os.GRND_NONBLOCK).hex())' > "$password_file"
[ "$(wc -c < "$password_file" | tr -d ' ')" = 64 ]
grep -Eq '^[0-9a-f]{64}$' "$password_file"
chmod 400 "$password_file"
phase=scripts
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
emit test_password_generated yes
emit rollback_state_synced yes
emit boot_rollback_armed yes
emit watchdog_active yes
""".strip()
    return f"{_assignments(values)}\n{body}"


PROCESS_FUNCTIONS = r"""
process_no_udp_or_abstract() {
  pid=$1
  for fd in "/proc/$pid/fd/"*; do
    target=$(readlink "$fd" 2>/dev/null || true)
    case "$target" in socket:\[*\]) inode=${target#socket:[}; inode=${inode%]} ;; *) continue ;; esac
    for table in "/proc/$pid/net/udp" "/proc/$pid/net/udp6"; do
      awk -v inode="$inode" '$10 == inode {bad=1} END {exit bad}' "$table"
    done
    awk -v inode="$inode" '$7 == inode && $8 ~ /^@/ {bad=1} END {exit bad}' "/proc/$pid/net/unix"
  done
}
server_listener_exact() {
  pid=$1
  listeners=0
  for fd in "/proc/$pid/fd/"*; do
    target=$(readlink "$fd" 2>/dev/null || true)
    case "$target" in socket:\[*\]) inode=${target#socket:[}; inode=${inode%]} ;; *) continue ;; esac
    if awk -v inode="$inode" '$10 == inode && $4 == "0A" {found=1; if ($2 != "0100007F:46A0") bad=1} END {exit bad ? 2 : !found}' /proc/net/tcp; then
      listeners=$((listeners + 1))
    else
      result=$?
      [ "$result" != 2 ] || return 1
    fi
    if awk -v inode="$inode" '$10 == inode && $4 == "0A" {found=1} END {exit !found}' /proc/net/tcp6; then return 1; fi
  done
  [ "$listeners" = 1 ]
}
verify_zero_caps() {
  pid=$1
  [ "$(awk '/^CapEff:/ {print $2}' "/proc/$pid/status")" = 0000000000000000 ]
  [ "$(awk '/^CapBnd:/ {print $2}' "/proc/$pid/status")" = 0000000000000000 ]
  [ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$pid/status")" = 1 ]
}
""".strip()


def build_apply_script(paths: RemotePaths, assets: provision.Assets) -> str:
    dropin = build_dropin()
    values = {
        **_path_assignments(paths),
        "marker_value": f"{paths.run_id}:{assets.package.sha256}",
        "dropin_dir": SERVER_DROPIN_DIR,
        "dropin_path": SERVER_DROPIN,
        "dropin_b64": _encoded(dropin),
        "dropin_sha256": hashlib.sha256(dropin.encode()).hexdigest(),
        "state_root": STATE_ROOT,
        "test_admin": TEST_ADMIN,
        "jpeg_counter_awk": JPEG_COUNTER_AWK,
        "broker_unit": BROKER_UNIT,
        "video_unit": VIDEO_UNIT,
        "server_unit": SERVER_UNIT,
        "watchdog_unit": paths.watchdog_unit,
    }
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=validate
run_succeeded=no
cleanup_state=retained
stream_pid=
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ -n "$stream_pid" ]; then kill "$stream_pid" >/dev/null 2>&1 || true; wait "$stream_pid" 2>/dev/null || true; fi
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
[ -f "$rollback_script" ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
units_inactive
control_boundary_absent
[ ! -e "$state_root" ]
[ ! -e "$dropin_dir" ]
phase=dropin
mkdir -m 755 "$dropin_dir"
printf '%s' "$dropin_b64" | base64 -d > "$dropin_path"
[ "$(sha256sum "$dropin_path" | awk '{print $1}')" = "$dropin_sha256" ]
chmod 644 "$dropin_path"
sync "$dropin_path" "$dropin_dir"
systemctl daemon-reload
phase=start-broker
systemctl start "$broker_unit"
count=0
while [ "$count" -lt 100 ] && [ ! -S /run/hardened-blikvm/platform-broker.sock ]; do
  [ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" = active ]
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
phase=start-video
systemctl start "$video_unit"
count=0
while [ "$count" -lt 100 ] && [ ! -S /run/hardened-blikvm-video/mjpeg.sock ]; do
  [ "$(systemctl is-active "$video_unit" 2>/dev/null || true)" = active ]
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
phase=start-server
systemctl start "$server_unit"
count=0
ready=no
while [ "$count" -lt 100 ]; do
  if curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null http://127.0.0.1:18080/api/health 2>/dev/null; then ready=yes; break; fi
  [ "$(systemctl is-active "$server_unit" 2>/dev/null || true)" = active ]
  sleep 0.1
  count=$((count + 1))
done
[ "$ready" = yes ]
units_active
control_boundary_absent
broker_pid=$(systemctl show -p MainPID --value "$broker_unit")
video_pid=$(systemctl show -p MainPID --value "$video_unit")
server_pid=$(systemctl show -p MainPID --value "$server_unit")
for pid in "$broker_pid" "$video_pid" "$server_pid"; do case "$pid" in ''|0|*[!0-9]*) exit 1 ;; esac; done
phase=broker-isolation
[ "$(systemctl show -p User --value "$broker_unit")" = hardened-blikvm-broker ]
[ "$(systemctl show -p Group --value "$broker_unit")" = hardened-blikvm ]
[ "$(systemctl show -p SupplementaryGroups --value "$broker_unit")" = kvmd ]
[ "$(systemctl show -p PrivateNetwork --value "$broker_unit")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$broker_unit")" = closed ]
verify_zero_caps "$broker_pid"
[ "$(readlink "/proc/$broker_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
broker_uid=$(getent passwd hardened-blikvm-broker | awk -F: '{print $3}')
shared_gid=$(getent group hardened-blikvm | awk -F: '{print $3}')
[ "$(stat -c '%a:%u:%g' /run/hardened-blikvm)" = "750:$broker_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' /run/hardened-blikvm/platform-broker.sock)" = "660:$broker_uid:$shared_gid" ]
credentials_directory=$(tr '\0' '\n' < "/proc/$broker_pid/environ" | sed -n 's/^CREDENTIALS_DIRECTORY=//p')
[ -n "$credentials_directory" ]
for name in kvmd-user kvmd-password; do
  [ "$(stat -c '%a:%h' "$credentials_directory/$name")" = 400:1 ]
done
phase=video-isolation
[ "$(systemctl show -p User --value "$video_unit")" = hardened-blikvm-video ]
[ "$(systemctl show -p Group --value "$video_unit")" = hardened-blikvm ]
[ "$(systemctl show -p SupplementaryGroups --value "$video_unit")" = kvmd ]
[ "$(systemctl show -p PrivateNetwork --value "$video_unit")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$video_unit")" = closed ]
verify_zero_caps "$video_pid"
[ "$(readlink "/proc/$video_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
video_uid=$(getent passwd hardened-blikvm-video | awk -F: '{print $3}')
[ "$(stat -c '%a:%u:%g' /run/hardened-blikvm-video)" = "750:$video_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' /run/hardened-blikvm-video/mjpeg.sock)" = "660:$video_uid:$shared_gid" ]
phase=server-isolation
[ "$(systemctl show -p User --value "$server_unit")" = hardened-blikvm-server ]
[ "$(systemctl show -p Group --value "$server_unit")" = hardened-blikvm ]
[ -z "$(systemctl show -p SupplementaryGroups --value "$server_unit")" ]
[ "$(systemctl show -p PrivateNetwork --value "$server_unit")" = no ]
[ "$(systemctl show -p DevicePolicy --value "$server_unit")" = closed ]
verify_zero_caps "$server_pid"
[ "$(readlink "/proc/$server_pid/ns/net")" = "$(readlink /proc/1/ns/net)" ]
server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{print $3}')
[ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
listener_count=$(awk '$2 == "0100007F:46A0" && $4 == "0A" {count++} END {print count+0}' /proc/net/tcp)
[ "$listener_count" = 1 ]
awk '$2 ~ /:46A0$/ && !($2 == "0100007F:46A0" && $4 == "0A") {bad=1} END {exit bad}' /proc/net/tcp /proc/net/tcp6
server_listener_exact "$server_pid"
for pid in "$broker_pid" "$video_pid" "$server_pid"; do process_no_udp_or_abstract "$pid"; done
phase=persistent-credentials
for account in hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-server hardened-blikvm-control hardened-blikvm-update; do
  uid=$(getent passwd "$account" | awk -F: '{print $3}')
  gid=$(getent passwd "$account" | awk -F: '{print $4}')
  for path in /etc/hardened-blikvm/kvmd-user /etc/hardened-blikvm/kvmd-password; do
    if setpriv --reuid="$uid" --regid="$gid" --clear-groups --no-new-privs cat "$path" >/dev/null 2>&1; then exit 1; fi
  done
done
phase=setup-state
origin=http://127.0.0.1:18080
setup_status=$(curl --noproxy '*' --silent --max-time 3 --output "$setup_response" --write-out '%{http_code}' "$origin/api/auth/setup" 2>/dev/null)
[ "$setup_status" = 200 ]
grep -q '"required":true' "$setup_response"
printf '{"username":"%s","password":"' "$test_admin" > "$request_file"
cat "$password_file" >> "$request_file"
printf '"}\n' >> "$request_file"
chmod 600 "$request_file"
phase=setup-account
setup_status=$(curl --noproxy '*' --silent --max-time 8 --output "$setup_response" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request_file" "$origin/api/auth/setup" 2>/dev/null)
[ "$setup_status" = 200 ]
phase=login
login_status=$(curl --noproxy '*' --silent --max-time 8 --cookie-jar "$cookie_file" --output "$login_response" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request_file" "$origin/api/auth/login" 2>/dev/null)
[ "$login_status" = 200 ]
[ "$(stat -c '%a:%u:%g:%h' "$cookie_file")" = 600:0:0:1 ]
phase=inventory
inventory_status=$(curl --noproxy '*' --silent --max-time 15 --cookie "$cookie_file" --output "$inventory_file" --write-out '%{http_code}' "$origin/api/inventory" 2>/dev/null)
[ "$inventory_status" = 200 ]
grep -q '"schemaVersion":3' "$inventory_file"
grep -q '"backend":"kvmd_transition"' "$inventory_file"
if grep -Eqi '"(password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress)"' "$inventory_file"; then exit 1; fi
phase=diagnostics
diagnostics_status=$(curl --noproxy '*' --silent --max-time 15 --cookie "$cookie_file" --output "$diagnostics_file" --write-out '%{http_code}' "$origin/api/diagnostics" 2>/dev/null)
[ "$diagnostics_status" = 200 ]
grep -q '"schemaVersion":1' "$diagnostics_file"
grep -q '"availability":"current_only"' "$diagnostics_file"
grep -q '"history":{"state":"unavailable","reason":"persistence_not_provisioned"}' "$diagnostics_file"
if grep -Eqi '"(platformId|versions|bundle|binding|instanceId|windowId|password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress|address|rawLog)"' "$diagnostics_file"; then exit 1; fi
capture_video() {
  output=$1
  set +e
  curl --noproxy '*' --silent --max-time 4 --cookie "$cookie_file" --output "$output" "$origin/api/stream/mjpeg" 2>/dev/null
  result=$?
  set -e
  [ "$result" = 0 ] || [ "$result" = 28 ]
  [ "$(stat -c '%s' "$output")" -le 8388608 ]
  frames=$(od -An -v -tu1 "$output" | awk "$jpeg_counter_awk")
  [ "$frames" -ge 2 ]
}
phase=video-first
capture_video "$video_first"
phase=video-reconnect
capture_video "$video_second"
phase=video-revocation-start
curl --noproxy '*' --silent --max-time 15 --cookie "$cookie_file" --output "$video_revoked" "$origin/api/stream/mjpeg" 2>/dev/null &
stream_pid=$!
phase=video-revocation-warmup
count=0
while [ "$count" -lt 50 ] && { [ ! -f "$video_revoked" ] || [ "$(stat -c '%s' "$video_revoked")" -lt 1024 ]; }; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
phase=video-revocation-csrf
csrf=$(python3 -c 'import json,sys; value=json.load(open(sys.argv[1],encoding="utf-8"))["data"]["csrfToken"]; assert isinstance(value,str) and 16 <= len(value) <= 256; print(value,end="")' "$login_response")
printf 'header = "X-CSRF-Token: %s"\n' "$csrf" > "$logout_config"
chmod 600 "$logout_config"
phase=video-revocation-logout
logout_status=$(curl --noproxy '*' --silent --max-time 5 --config "$logout_config" --cookie "$cookie_file" \
  --request POST --output /dev/null --write-out '%{http_code}' "$origin/api/auth/logout" 2>/dev/null)
[ "$logout_status" = 200 ]
phase=video-revocation-stop
count=0
while [ "$count" -lt 50 ] && kill -0 "$stream_pid" 2>/dev/null; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
wait "$stream_pid" 2>/dev/null || true
stream_pid=
phase=video-revocation-session
revoked_status=$(curl --noproxy '*' --silent --max-time 3 --cookie "$cookie_file" --output /dev/null --write-out '%{http_code}' "$origin/api/diagnostics" 2>/dev/null)
[ "$revoked_status" = 401 ]
phase=state
[ -f "$state_root/account.json" ]
[ ! -L "$state_root/account.json" ]
[ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
[ "$(stat -c '%s' "$state_root/account.json")" -le 4096 ]
unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
[ -z "$unexpected" ]
control_boundary_absent
for path in "$rollback_root"/*; do
  [ -e "$path" ] || continue
  [ -f "$path" ]
  [ ! -L "$path" ]
  [ "$(stat -c '%u:%g:%h' "$path")" = 0:0:1 ]
  [ "$(stat -c '%s' "$path")" -le 8388608 ]
done
sync
run_succeeded=yes
trap - EXIT HUP INT TERM
emit broker_active active
emit video_active active
emit server_active active
emit broker_isolation_verified yes
emit video_isolation_verified yes
emit server_isolation_verified yes
emit tcp_listener_loopback_only yes
emit udp_sockets_absent yes
emit abstract_sockets_absent yes
emit persistent_credentials_private yes
emit control_boundary_absent yes
emit setup_status "$setup_status"
emit inventory_status "$inventory_status"
emit diagnostics_status "$diagnostics_status"
emit inventory_sanitized yes
emit diagnostics_current_only yes
emit diagnostics_sanitized yes
emit mjpeg_frames yes
emit mjpeg_reconnected yes
emit session_revoked_stream yes
emit private_artifacts_bounded yes
""".strip()
    return (
        f"{_assignments(values)}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{PROCESS_FUNCTIONS}\n{body}"
    )


def build_commit_script(paths: RemotePaths, assets: provision.Assets) -> str:
    dropin = build_dropin()
    boot_unit = build_boot_unit(paths)
    values = {
        **_path_assignments(paths),
        "boot_unit_part": paths.boot_unit_part,
        "marker_value": f"{paths.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "dropin_dir": SERVER_DROPIN_DIR,
        "dropin_path": SERVER_DROPIN,
        "dropin_sha256": hashlib.sha256(dropin.encode()).hexdigest(),
        "state_root": STATE_ROOT,
        "server_user": "hardened-blikvm-server",
        "shared_group": identity_policy.SHARED_GROUP,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" = active ]
units_active
control_boundary_absent
[ -f "$dropin_path" ]
[ "$(sha256sum "$dropin_path" | awk '{print $1}')" = "$dropin_sha256" ]
[ -d "$state_root" ]
port_in_use 46A0
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do
  systemctl stop "$unit"
done
count=0
while [ "$count" -lt 100 ]; do
  if units_inactive && cgroup_empty hardened-blikvm-server.service && \
     cgroup_empty hardened-blikvm-video-proxy.service && \
     cgroup_empty hardened-blikvm-platform-broker.service; then break; fi
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
server_uid=$(getent passwd "$server_user" | awk -F: '{print $3}')
shared_gid=$(getent group "$shared_group" | awk -F: '{print $3}')
[ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
[ -z "$unexpected" ]
if [ -e "$state_root/account.json" ]; then
  [ -f "$state_root/account.json" ]
  [ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
  [ "$(stat -c '%s' "$state_root/account.json")" -le 4096 ]
  rm -f -- "$state_root/account.json"
fi
for path in "$state_root"/.account-*; do
  [ -e "$path" ] || continue
  [ -f "$path" ]
  [ ! -L "$path" ]
  [ "$(stat -c '%u:%g:%h' "$path")" = "$server_uid:$shared_gid:1" ]
  [ "$(stat -c '%s' "$path")" -le 4096 ]
  rm -f -- "$path"
done
rmdir "$state_root"
rm -f -- "$dropin_path"
rmdir "$dropin_dir"
systemctl daemon-reload
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
control_boundary_absent
private_artifacts_absent
socket_tail_clean
systemctl stop "$watchdog_unit" >/dev/null 2>&1
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
[ -f "$boot_unit_path" ]
[ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
[ -L "$boot_unit_link" ]
[ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
rm -f -- "$boot_unit_link" "$boot_unit_path"
systemctl daemon-reload
remove_private_root
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
emit units_inactive yes
emit cgroups_empty yes
emit state_root_removed yes
emit runtime_roots_removed yes
emit dropin_removed yes
emit private_artifacts_removed yes
emit socket_tail_clean yes
emit watchdog_removed yes
emit boot_rollback_removed yes
""".strip()
    return (
        f"{_assignments(values)}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{REMOVE_PRIVATE_ROOT}\n{body}"
    )


def build_cleanup_script(paths: RemotePaths, assets: provision.Assets) -> str:
    boot_unit = build_boot_unit(paths)
    values = {
        **_path_assignments(paths),
        "boot_unit_part": paths.boot_unit_part,
        "marker_value": f"{paths.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "dropin_dir": SERVER_DROPIN_DIR,
        "dropin_path": SERVER_DROPIN,
        "state_root": STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
if [ -f "$rollback_script" ] && [ -f "$marker" ]; then
  /bin/sh "$rollback_script" explicit >/dev/null 2>&1
else
  units_inactive
  [ ! -e "$state_root" ]
  [ ! -e "$dropin_path" ]
  [ ! -e "$dropin_dir" ]
  if [ -e "$boot_unit_link" ]; then
    [ -L "$boot_unit_link" ]
    [ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
    rm -f -- "$boot_unit_link"
  fi
  if [ -e "$boot_unit_path" ]; then
    [ -f "$boot_unit_path" ]
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
      [ "$(cat "$marker")" = "$marker_value" ]
      remove_private_root
    else
      [ -z "$(find "$rollback_root" -mindepth 1 -print -quit)" ]
      rmdir "$rollback_root"
    fi
  fi
fi
units_inactive
control_boundary_absent
[ ! -e "$state_root" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
[ ! -e "$dropin_path" ]
[ ! -e "$dropin_dir" ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
private_artifacts_absent
socket_tail_clean
emit cleanup_restored yes
""".strip()
    return (
        f"{_assignments(values)}\n{PORT_FUNCTION}\n{STATE_FUNCTIONS}\n"
        f"{REMOVE_PRIVATE_ROOT}\n{body}"
    )


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    paths = remote_paths(assets.package.sha256, secrets.token_hex(8))
    provision_paths = provision.remote_paths(
        assets.sysusers.sha256, secrets.token_hex(8)
    )
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    try:
        provisioned_result = prototype.run_ssh(
            target,
            secret,
            provision.build_final_script(provision_paths, assets),
            timeout=60,
        )
        if provisioned_result.returncode != 0:
            raise ValueError("provisioned runtime precondition failed")
        provisioned_before = provision.parse_facts(
            provisioned_result.stdout, provision.FINAL_KEYS
        )
        preflight_result = prototype.run_ssh(
            target, secret, build_preflight_script(paths, assets), timeout=60
        )
        if preflight_result.returncode != 0:
            phase, _ = parse_failure_phase(preflight_result.stdout, "preflight")
            raise ValueError(f"runtime activation preflight failed at {phase}")
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        cleanup_required = True
        prepare_result = prototype.run_ssh(
            target, secret, build_prepare_script(paths, assets), timeout=120
        )
        if prepare_result.returncode != 0:
            phase, _ = parse_failure_phase(prepare_result.stdout, "prepare")
            raise ValueError(f"runtime activation prepare failed at {phase}")
        prepared = parse_facts(prepare_result.stdout, PREPARE_KEYS)
        apply_result = prototype.run_ssh(
            target, secret, build_apply_script(paths, assets), timeout=180
        )
        if apply_result.returncode != 0:
            phase, remote_cleanup = parse_failure_phase(apply_result.stdout, "apply")
            if remote_cleanup is True:
                cleanup_required = False
            raise ValueError(f"runtime activation apply failed at {phase}")
        applied = parse_facts(apply_result.stdout, APPLY_KEYS)
        commit_result = prototype.run_ssh(
            target, secret, build_commit_script(paths, assets), timeout=180
        )
        if commit_result.returncode != 0:
            raise ValueError("runtime activation commit cleanup failed")
        committed = parse_facts(commit_result.stdout, COMMIT_KEYS)
        cleanup_required = False
        final_result = prototype.run_ssh(
            target,
            secret,
            provision.build_final_script(provision_paths, assets),
            timeout=60,
        )
        if final_result.returncode != 0:
            raise ValueError("runtime activation final state failed")
        final_state = provision.parse_facts(final_result.stdout, provision.FINAL_KEYS)
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-activation-gate",
            "result": "passed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "policy": {
                "servicesEnabled": False,
                "controlServiceStarted": False,
                "controlSocketUnitPresent": False,
                "temporarySecureCookieOverride": "loopback-native-gate-only",
                "nginxMutation": False,
                "persistentApplicationState": False,
                "persistentDiagnosticHistory": False,
                "hardwareMutation": False,
                "physicalActionsInvoked": False,
                "secretEmitted": False,
            },
            "provisionedBefore": provisioned_before,
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
            "confirmed": not cleanup_required,
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
            "probe": "hardened-blikvm-runtime-activation-gate",
            "result": "failed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
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
