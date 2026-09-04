#!/usr/bin/env python3
"""Run the installed runtime behind an ephemeral shadow TLS proxy and Chromium."""

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

from tools import stage_device_runtime_activation as activation
from tools import stage_device_runtime_provision as provision
from tools import stage_device_tls_browser as tls
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


TLS_PORT = 18443
PREFLIGHT_KEYS = {
    "architecture",
    "browser_runtime_ready",
    "gates_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "nginx_ready",
    "ports_free_before",
    "rootfs_mode_before",
    "runtime_roots_absent",
    "tools_ready",
    "units_inactive_before",
}
START_KEYS = {
    "control_boundary_absent",
    "nginx_isolation_verified",
    "nginx_tree_unchanged",
    "ready",
    "runtime_active",
    "tls12",
    "tls13",
    "tls_loopback",
    "watchdog_active",
}
COMMIT_KEYS = {
    "boot_rollback_removed",
    "cgroups_empty",
    "nginx_shadow_removed",
    "nginx_tree_unchanged",
    "private_root_removed",
    "runtime_roots_removed",
    "socket_tail_clean",
    "state_root_removed",
    "units_inactive",
    "watchdog_removed",
}
CLEANUP_KEYS = {"cleanup_restored"}
ALLOWED_VALUES = {
    "yes",
    "active",
    "aarch64",
    "rw",
    "passed",
}


@dataclass(frozen=True)
class Paths:
    run_id: str
    root: str
    marker: str
    certificate: str
    key: str
    nginx_config: str
    nginx_tree_hash: str
    nginx_state: str
    rollback_script: str
    watchdog_script: str
    boot_unit: str
    boot_unit_path: str
    boot_unit_part: str
    boot_unit_link: str
    watchdog_unit: str
    nginx_unit: str


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
    parser.add_argument("--known-hosts", required=True)
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


def paths(package_sha256: str, run_id: str) -> Paths:
    if not re.fullmatch(r"[0-9a-f]{64}", package_sha256) or not re.fullmatch(
        r"[0-9a-f]{16}", run_id
    ):
        raise ValueError("installed TLS gate identity is invalid")
    root = (
        "/var/lib/hardened-blikvm-installed-tls-rollback-"
        f"{package_sha256[:12]}-{run_id}"
    )
    boot_unit = f"hardened-blikvm-installed-tls-rollback-{run_id}.service"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        certificate=f"{root}/tls.crt",
        key=f"{root}/tls.key",
        nginx_config=f"{root}/nginx.conf",
        nginx_tree_hash=f"{root}/nginx-tree.sha256",
        nginx_state=f"{root}/nginx-state",
        rollback_script=f"{root}/rollback.sh",
        watchdog_script=f"{root}/watchdog.sh",
        boot_unit=boot_unit,
        boot_unit_path=f"/etc/systemd/system/{boot_unit}",
        boot_unit_part=f"/etc/systemd/system/{boot_unit}.part",
        boot_unit_link=f"/etc/systemd/system/sysinit.target.wants/{boot_unit}",
        watchdog_unit=f"hardened-blikvm-installed-tls-watchdog-{run_id}.service",
        nginx_unit=f"hardened-blikvm-installed-tls-{run_id}.service",
    )


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def render_nginx(paths_value: Paths) -> str:
    return f"""pid {paths_value.nginx_state}/nginx.pid;
error_log {paths_value.nginx_state}/error.log notice;
worker_processes 1;

events {{
    worker_connections 64;
}}

http {{
    access_log off;
    server_tokens off;
    client_body_temp_path {paths_value.nginx_state}/client-temp;
    proxy_temp_path {paths_value.nginx_state}/proxy-temp;
    map $http_upgrade $hardened_blikvm_connection_upgrade {{
        default upgrade;
        '' close;
    }}
    map $http_host $hardened_blikvm_forwarded_host {{
        default $http_host;
        '' $host;
    }}
    server {{
        listen 127.0.0.1:{TLS_PORT} ssl;
        server_name _;
        ssl_certificate {paths_value.certificate};
        ssl_certificate_key {paths_value.key};
        ssl_protocols TLSv1.2 TLSv1.3;
        ssl_session_cache off;
        ssl_session_tickets off;
        client_max_body_size 16m;
        location / {{
            proxy_pass http://127.0.0.1:18080;
            proxy_http_version 1.1;
            proxy_set_header Host $hardened_blikvm_forwarded_host;
            proxy_set_header X-Forwarded-Proto https;
            proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $hardened_blikvm_connection_upgrade;
            proxy_buffering off;
            proxy_read_timeout 3600s;
        }}
    }}
}}
"""


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("installed TLS gate output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected_keys
            or key in facts
            or value not in ALLOWED_VALUES
        ):
            raise ValueError("installed TLS gate returned an invalid fact")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("installed TLS gate result was incomplete")
    return dict(sorted(facts.items()))


def parse_failure(raw: bytes) -> tuple[str, bool]:
    if len(raw) > 2048:
        raise ValueError("installed TLS failure output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key in facts or key not in {
            "failure_phase",
            "failure_cleanup",
        }:
            raise ValueError("installed TLS failure output is invalid")
        facts[key] = value
    phase = facts.get("failure_phase", "")
    cleanup = facts.get("failure_cleanup", "")
    if not re.fullmatch(r"[a-z-]{1,48}", phase) or cleanup not in {
        "restored",
        "retained",
    }:
        raise ValueError("installed TLS failure facts are invalid")
    return phase, cleanup == "restored"


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def build_preflight_script(paths_value: Paths) -> str:
    values = _assignments(
        {
            "rollback_root": paths_value.root,
            "boot_unit_path": paths_value.boot_unit_path,
            "boot_unit_part": paths_value.boot_unit_part,
            "boot_unit_link": paths_value.boot_unit_link,
            "watchdog_unit": paths_value.watchdog_unit,
            "nginx_unit": paths_value.nginx_unit,
            "state_root": activation.STATE_ROOT,
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
  if [ "$code" -ne 0 ]; then emit failure_phase "$phase"; emit failure_cleanup restored; fi
  exit "$code"
}
trap on_exit EXIT
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
phase=tools
for tool in awk base64 cat chmod chown cmp curl df find findmnt getent grep install mkdir mv nginx openssl pgrep readlink rm rmdir setpriv sha256sum sleep sort stat sync systemctl systemd-run tail tr wc xargs; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
phase=rootfs
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=runtime
units_inactive
control_boundary_absent
[ ! -e "$state_root" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
phase=ports
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
socket_tail_clean
phase=gates
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
[ "$(systemctl show -p LoadState --value "$nginx_unit" 2>/dev/null || true)" = not-found ]
[ -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-installed-tls-rollback-*' -print -quit)" ]
[ -z "$(find /etc/systemd/system -maxdepth 2 -name 'hardened-blikvm-installed-tls-*' -print -quit)" ]
phase=nginx
[ -x /usr/sbin/nginx ]
[ -d /etc/nginx ]
phase=kvmd
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ -S /run/kvmd/kvmd.sock ]
[ -S /run/kvmd/ustreamer.sock ]
phase=complete
emit architecture aarch64
emit browser_runtime_ready yes
emit tools_ready yes
emit nginx_ready yes
emit rootfs_mode_before "$rootfs_mode"
emit units_inactive_before yes
emit runtime_roots_absent yes
emit ports_free_before yes
emit gates_absent_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
""".strip()
    return (
        f"{values}\n{activation.PORT_FUNCTION}\n"
        f"{activation.STATE_FUNCTIONS}\n{body}"
    )


def build_boot_unit(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM installed TLS gate rollback",
            "DefaultDependencies=no",
            "After=local-fs.target",
            "Before=kvmd.service kvmd-nginx.service multi-user.target",
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


def _path_values(paths_value: Paths) -> dict[str, str]:
    return {
        "rollback_root": paths_value.root,
        "marker": paths_value.marker,
        "certificate": paths_value.certificate,
        "tls_key": paths_value.key,
        "nginx_config": paths_value.nginx_config,
        "nginx_tree_hash": paths_value.nginx_tree_hash,
        "nginx_state": paths_value.nginx_state,
        "rollback_script": paths_value.rollback_script,
        "watchdog_script": paths_value.watchdog_script,
        "boot_unit": paths_value.boot_unit,
        "boot_unit_path": paths_value.boot_unit_path,
        "boot_unit_link": paths_value.boot_unit_link,
        "watchdog_unit": paths_value.watchdog_unit,
        "nginx_unit": paths_value.nginx_unit,
    }


REMOVE_ROOT = r"""
remove_gate_root() {
  www_uid=$(getent passwd www-data | awk -F: '{print $3}')
  www_gid=$(getent group www-data | awk -F: '{print $3}')
  [ -d "$rollback_root" ]
  [ ! -L "$rollback_root" ]
  [ "$(stat -c '%a:%u:%g' "$rollback_root")" = "750:0:$www_gid" ]
  unexpected=$(find "$rollback_root" -mindepth 1 -maxdepth 1 \
    ! -name .owner ! -name tls.crt ! -name tls.key ! -name nginx.conf \
    ! -name nginx-tree.sha256 ! -name nginx-state ! -name rollback.sh ! -name watchdog.sh \
    -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$nginx_state" ]; then
    [ -d "$nginx_state" ]
    [ ! -L "$nginx_state" ]
    [ "$(stat -c '%a:%u:%g' "$nginx_state")" = "700:$www_uid:$www_gid" ]
    unexpected=$(find "$nginx_state" -mindepth 1 -maxdepth 1 \
      ! -name nginx.pid ! -name error.log ! -name client-temp ! -name proxy-temp -print -quit)
    [ -z "$unexpected" ]
    for path in "$nginx_state/nginx.pid" "$nginx_state/error.log"; do
      if [ -e "$path" ]; then
        [ -f "$path" ]
        [ ! -L "$path" ]
        [ "$(stat -c '%u:%g:%h' "$path")" = "$www_uid:$www_gid:1" ]
        [ "$(stat -c '%s' "$path")" -le 1048576 ]
        rm -f -- "$path"
      fi
    done
    for directory in "$nginx_state/client-temp" "$nginx_state/proxy-temp"; do
      if [ -d "$directory" ]; then
        [ -z "$(find "$directory" -mindepth 1 -print -quit)" ]
        rmdir "$directory"
      fi
    done
    rmdir "$nginx_state"
  fi
  for path in "$certificate" "$tls_key" "$nginx_config" "$nginx_tree_hash"; do
    if [ -e "$path" ]; then
      [ -f "$path" ]
      [ ! -L "$path" ]
      [ "$(stat -c '%h' "$path")" = 1 ]
      [ "$(stat -c '%s' "$path")" -le 1048576 ]
      rm -f -- "$path"
    fi
  done
  rm -f -- "$watchdog_script" "$rollback_script" "$marker"
  rmdir "$rollback_root"
}
""".strip()


def build_rollback_script(paths_value: Paths, assets: provision.Assets) -> str:
    boot_unit = build_boot_unit(paths_value)
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "state_root": activation.STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
[ "$#" = 1 ]
case "$1" in boot|watchdog|explicit) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
systemctl stop "$nginx_unit" >/dev/null 2>&1 || true
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do
  systemctl stop "$unit" >/dev/null 2>&1 || true
done
count=0
while [ "$count" -lt 100 ]; do
  if units_inactive && cgroup_empty hardened-blikvm-server.service && \
     cgroup_empty hardened-blikvm-video-proxy.service && \
     cgroup_empty hardened-blikvm-platform-broker.service && \
     cgroup_empty "$nginx_unit"; then break; fi
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
systemctl reset-failed "$nginx_unit" >/dev/null 2>&1 || true
if [ -e "$state_root" ]; then
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{print $3}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{print $3}')
  [ -d "$state_root" ]
  [ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
  unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
  [ -z "$unexpected" ]
  for path in "$state_root/account.json" "$state_root"/.account-*; do
    [ -e "$path" ] || continue
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%u:%g:%h' "$path")" = "$server_uid:$shared_gid:1" ]
    [ "$(stat -c '%s' "$path")" -le 4096 ]
    rm -f -- "$path"
  done
  rmdir "$state_root"
fi
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
control_boundary_absent
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
remove_gate_root
""".strip()
    return (
        f"#!/bin/sh\n{_assignments(values)}\n{activation.PORT_FUNCTION}\n"
        f"{activation.STATE_FUNCTIONS}\n{REMOVE_ROOT}\n{body}\n"
    )


def build_start_script(paths_value: Paths, assets: provision.Assets) -> str:
    nginx = render_nginx(paths_value)
    boot_unit = build_boot_unit(paths_value)
    rollback = build_rollback_script(paths_value, assets)
    watchdog = (
        "#!/bin/sh\nset -eu\nsleep 600\nexec /bin/sh "
        f"{shlex.quote(paths_value.rollback_script)} watchdog\n"
    )
    values = {
        **_path_values(paths_value),
        "boot_unit_part": paths_value.boot_unit_part,
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "nginx_b64": base64.b64encode(nginx.encode()).decode("ascii"),
        "nginx_sha256": hashlib.sha256(nginx.encode()).hexdigest(),
        "boot_unit_b64": base64.b64encode(boot_unit.encode()).decode("ascii"),
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "rollback_b64": base64.b64encode(rollback.encode()).decode("ascii"),
        "watchdog_b64": base64.b64encode(watchdog.encode()).decode("ascii"),
        "state_root": activation.STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
umask 077
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=root
run_succeeded=no
cleanup_state=retained
on_exit() {
  code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ] && [ -f "$rollback_script" ]; then
    if /bin/sh "$rollback_script" explicit >/dev/null 2>&1; then cleanup_state=restored; fi
    emit failure_phase "$phase"
    emit failure_cleanup "$cleanup_state"
  fi
  exit "$code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
www_uid=$(getent passwd www-data | awk -F: '{print $3}')
www_gid=$(getent group www-data | awk -F: '{print $3}')
case "$www_uid:$www_gid" in *[!0-9:]*|0:*|*:0|:) exit 1 ;; esac
[ ! -e "$rollback_root" ]
mkdir -m 750 "$rollback_root"
chown 0:"$www_gid" "$rollback_root"
printf '%s\n' "$marker_value" > "$marker"
chmod 600 "$marker"
phase=tls
openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 1 \
  -subj '/CN=Hardened-BliKVM-Test' -keyout "$tls_key" -out "$certificate" >/dev/null 2>&1
chown "$www_uid:$www_gid" "$tls_key"
chown 0:"$www_gid" "$certificate"
chmod 400 "$tls_key"
chmod 440 "$certificate"
printf '%s' "$nginx_b64" | base64 -d > "$nginx_config"
[ "$(sha256sum "$nginx_config" | awk '{print $1}')" = "$nginx_sha256" ]
chown 0:"$www_gid" "$nginx_config"
chmod 440 "$nginx_config"
mkdir -m 700 "$nginx_state"
chown "$www_uid:$www_gid" "$nginx_state"
mkdir -m 700 "$nginx_state/client-temp" "$nginx_state/proxy-temp"
chown "$www_uid:$www_gid" "$nginx_state/client-temp" "$nginx_state/proxy-temp"
setpriv --reuid="$www_uid" --regid="$www_gid" --clear-groups --no-new-privs \
  /usr/sbin/nginx -t -c "$nginx_config" -p "$nginx_state" >/dev/null 2>&1
nginx_tree_hash > "$nginx_tree_hash"
chmod 600 "$nginx_tree_hash"
phase=rollback
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
printf '%s' "$watchdog_b64" | base64 -d > "$watchdog_script"
chmod 700 "$rollback_script" "$watchdog_script"
printf '%s' "$boot_unit_b64" | base64 -d > "$boot_unit_part"
[ "$(sha256sum "$boot_unit_part" | awk '{print $1}')" = "$boot_unit_sha256" ]
chmod 644 "$boot_unit_part"
mv "$boot_unit_part" "$boot_unit_path"
ln -s "../$boot_unit" "$boot_unit_link"
sync
systemctl daemon-reload
systemd-run --quiet --collect --unit="$watchdog_unit" \
  --property=NoNewPrivileges=yes --property=PrivateDevices=yes \
  --property=PrivateNetwork=yes --property=ProtectHome=yes \
  --property=ProtectKernelLogs=yes --property=ProtectKernelModules=yes \
  --property=ProtectKernelTunables=yes --property=RestrictAddressFamilies=AF_UNIX \
  /bin/sh "$watchdog_script"
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl is-active "$watchdog_unit" 2>/dev/null || true)" != active ]; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
phase=runtime
systemctl start hardened-blikvm-platform-broker.service
systemctl start hardened-blikvm-video-proxy.service
systemctl start hardened-blikvm-server.service
count=0
while [ "$count" -lt 100 ] && ! curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null http://127.0.0.1:18080/api/health 2>/dev/null; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 100 ]
units_active
control_boundary_absent
phase=nginx-start
systemd-run --quiet --collect --unit="$nginx_unit" \
  --property=User=www-data --property=Group=www-data \
  --property=NoNewPrivileges=yes --property=PrivateDevices=yes --property=PrivateTmp=yes \
  --property=ProtectHome=yes --property=ProtectSystem=strict \
  --property=ProtectKernelLogs=yes --property=ProtectKernelModules=yes \
  --property=ProtectKernelTunables=yes --property=RestrictNamespaces=yes \
  --property=RestrictAddressFamilies='AF_UNIX AF_INET' \
  --property=IPAddressDeny=any --property=IPAddressAllow=localhost \
  --property=SocketBindDeny=any --property=SocketBindAllow=tcp:18443 \
  --property=CapabilityBoundingSet= --property=AmbientCapabilities= \
  --property=ReadWritePaths="$nginx_state" \
  /usr/sbin/nginx -c "$nginx_config" -p "$nginx_state" -g 'daemon off;'
count=0
while [ "$count" -lt 100 ] && ! port_in_use 480B; do
  [ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ]
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
listener_count=$(awk '$2 == "0100007F:480B" && $4 == "0A" {count++} END {print count+0}' /proc/net/tcp)
[ "$listener_count" = 1 ]
awk '$2 ~ /:480B$/ && !($2 == "0100007F:480B" && $4 == "0A") {bad=1} END {exit bad}' /proc/net/tcp /proc/net/tcp6
nginx_pid=$(systemctl show -p MainPID --value "$nginx_unit")
case "$nginx_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
[ "$(systemctl show -p User --value "$nginx_unit")" = www-data ]
[ "$(systemctl show -p Group --value "$nginx_unit")" = www-data ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$nginx_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$nginx_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$nginx_pid/status")" = 1 ]
phase=tls12
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1:18443/api/health
phase=tls13
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1:18443/api/health
[ "$(nginx_tree_hash)" = "$(cat "$nginx_tree_hash")" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
sync
run_succeeded=yes
trap - EXIT HUP INT TERM
emit ready yes
emit runtime_active yes
emit watchdog_active yes
emit tls_loopback yes
emit tls12 passed
emit tls13 passed
emit nginx_isolation_verified yes
emit nginx_tree_unchanged yes
emit control_boundary_absent yes
""".strip()
    return (
        f"{_assignments(values)}\n{activation.PORT_FUNCTION}\n"
        f"{activation.STATE_FUNCTIONS}\n{provision.STATE_FUNCTIONS}\n{body}"
    )


STATE_CLEANUP = r"""
remove_account_state() {
  [ -e "$state_root" ] || return 0
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{print $3}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{print $3}')
  [ -d "$state_root" ]
  [ ! -L "$state_root" ]
  [ "$(stat -c '%a:%u:%g' "$state_root")" = "700:$server_uid:$shared_gid" ]
  unexpected=$(find "$state_root" -mindepth 1 -maxdepth 1 ! -name account.json ! -name '.account-*' -print -quit)
  [ -z "$unexpected" ]
  for path in "$state_root/account.json" "$state_root"/.account-*; do
    [ -e "$path" ] || continue
    [ -f "$path" ]
    [ ! -L "$path" ]
    [ "$(stat -c '%u:%g:%h' "$path")" = "$server_uid:$shared_gid:1" ]
    [ "$(stat -c '%s' "$path")" -le 4096 ]
    rm -f -- "$path"
  done
  rmdir "$state_root"
}
""".strip()


def build_commit_script(paths_value: Paths, assets: provision.Assets) -> str:
    boot_unit = build_boot_unit(paths_value)
    values = {
        **_path_values(paths_value),
        "boot_unit_part": paths_value.boot_unit_part,
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "state_root": activation.STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
units_active
control_boundary_absent
[ "$(systemctl is-active "$nginx_unit" 2>/dev/null || true)" = active ]
port_in_use 46A0
port_in_use 480B
[ "$(nginx_tree_hash)" = "$(cat "$nginx_tree_hash")" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
systemctl stop "$nginx_unit"
for unit in hardened-blikvm-server.service hardened-blikvm-video-proxy.service hardened-blikvm-platform-broker.service; do systemctl stop "$unit"; done
count=0
while [ "$count" -lt 100 ]; do
  if units_inactive && cgroup_empty hardened-blikvm-server.service && \
     cgroup_empty hardened-blikvm-video-proxy.service && \
     cgroup_empty hardened-blikvm-platform-broker.service && cgroup_empty "$nginx_unit"; then break; fi
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 100 ]
systemctl reset-failed "$nginx_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$nginx_unit" 2>/dev/null || true)" != not-found ]; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
remove_account_state
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
control_boundary_absent
socket_tail_clean
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
[ "$(nginx_tree_hash)" = "$(cat "$nginx_tree_hash")" ]
systemctl stop "$watchdog_unit" >/dev/null 2>&1
systemctl reset-failed "$watchdog_unit" >/dev/null 2>&1 || true
count=0
while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" != not-found ]; do sleep 0.1; count=$((count + 1)); done
[ "$count" -lt 50 ]
[ -f "$boot_unit_path" ]
[ "$(sha256sum "$boot_unit_path" | awk '{print $1}')" = "$boot_unit_sha256" ]
[ -L "$boot_unit_link" ]
[ "$(readlink "$boot_unit_link")" = "../$boot_unit" ]
rm -f -- "$boot_unit_link" "$boot_unit_path"
systemctl daemon-reload
remove_gate_root
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
emit units_inactive yes
emit cgroups_empty yes
emit state_root_removed yes
emit runtime_roots_removed yes
emit socket_tail_clean yes
emit nginx_shadow_removed yes
emit nginx_tree_unchanged yes
emit watchdog_removed yes
emit boot_rollback_removed yes
emit private_root_removed yes
""".strip()
    return (
        f"{_assignments(values)}\n{activation.PORT_FUNCTION}\n"
        f"{activation.STATE_FUNCTIONS}\n{provision.STATE_FUNCTIONS}\n"
        f"{STATE_CLEANUP}\n{REMOVE_ROOT}\n{body}"
    )


def build_cleanup_script(paths_value: Paths, assets: provision.Assets) -> str:
    boot_unit = build_boot_unit(paths_value)
    values = {
        **_path_values(paths_value),
        "boot_unit_part": paths_value.boot_unit_part,
        "marker_value": f"{paths_value.run_id}:{assets.package.sha256}",
        "boot_unit_sha256": hashlib.sha256(boot_unit.encode()).hexdigest(),
        "state_root": activation.STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -f "$rollback_script" ] && [ -f "$marker" ]; then
  /bin/sh "$rollback_script" explicit >/dev/null 2>&1
fi
systemctl stop "$watchdog_unit" >/dev/null 2>&1 || true
systemctl reset-failed "$watchdog_unit" "$nginx_unit" >/dev/null 2>&1 || true
for unit in "$watchdog_unit" "$nginx_unit"; do
  count=0
  while [ "$count" -lt 50 ] && [ "$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)" != not-found ]; do sleep 0.1; count=$((count + 1)); done
  [ "$count" -lt 50 ]
done
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
  [ "$(stat -c '%s' "$boot_unit_part")" -le 4096 ]
  rm -f -- "$boot_unit_part"
fi
systemctl daemon-reload >/dev/null 2>&1
if [ -e "$rollback_root" ]; then
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  remove_gate_root
fi
units_inactive
control_boundary_absent
[ ! -e "$state_root" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
[ ! -e "$rollback_root" ]
[ ! -e "$boot_unit_path" ]
[ ! -e "$boot_unit_part" ]
[ ! -e "$boot_unit_link" ]
[ "$(systemctl show -p LoadState --value "$nginx_unit" 2>/dev/null || true)" = not-found ]
[ "$(systemctl show -p LoadState --value "$watchdog_unit" 2>/dev/null || true)" = not-found ]
port_in_use 46A0 && exit 1
port_in_use 480B && exit 1
socket_tail_clean
emit cleanup_restored yes
""".strip()
    return (
        f"{_assignments(values)}\n{activation.PORT_FUNCTION}\n"
        f"{activation.STATE_FUNCTIONS}\n{REMOVE_ROOT}\n{body}"
    )


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    browser = tls.browser_runtime()
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths_value = paths(assets.package.sha256, secrets.token_hex(8))
    provision_paths = provision.remote_paths(
        assets.sysusers.sha256, secrets.token_hex(8)
    )
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    tunnel: subprocess.Popen[bytes] | None = None
    stage = "initialization"
    browser_summary: dict[str, object] | None = None
    try:
        stage = "provisioned-state"
        provisioned_result = prototype.run_ssh(
            target,
            secret,
            provision.build_final_script(provision_paths, assets),
            timeout=60,
        )
        if provisioned_result.returncode != 0:
            raise tls.TlsGateError(stage, "remote-command", return_code=provisioned_result.returncode)
        provisioned_before = provision.parse_facts(
            provisioned_result.stdout, provision.FINAL_KEYS
        )
        stage = "preflight"
        preflight_result = prototype.run_ssh(
            target, secret, build_preflight_script(paths_value), timeout=60
        )
        if preflight_result.returncode != 0:
            raise tls.TlsGateError(stage, "remote-command", return_code=preflight_result.returncode)
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        stage = "remote-start"
        cleanup_required = True
        start_result = prototype.run_ssh(
            target, secret, build_start_script(paths_value, assets), timeout=180
        )
        if start_result.returncode != 0:
            remote_phase, remote_cleanup = parse_failure(start_result.stdout)
            if remote_cleanup:
                cleanup_required = False
            raise tls.TlsGateError(
                stage,
                "remote-command",
                return_code=start_result.returncode,
                remote_phase=remote_phase,
                remote_cleanup_confirmed=remote_cleanup,
            )
        started = parse_facts(start_result.stdout, START_KEYS)
        stage = "ssh-tunnel"
        local_port = tls.free_local_port()
        tunnel = tls.start_with_secret_fd(
            secret,
            lambda fd: tls.tunnel_command(target, fd, local_port),
            stdout=subprocess.DEVNULL,
        )
        tls.wait_for_tunnel(tunnel, local_port, 15)
        stage = "browser"
        browser_result = subprocess.run(
            [
                str(browser.node),
                str(browser.script),
                str(browser.module),
                str(browser.executable),
                f"https://127.0.0.1:{local_port}",
            ],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=180,
        )
        if browser_result.returncode != 0:
            (
                browser_phase,
                browser_error,
                diagnostics_get_requests,
                diagnostics_response_statuses,
            ) = tls.validate_browser_failure(browser_result.stdout)
            raise tls.TlsGateError(
                stage,
                f"browser-{browser_phase}-{browser_error.casefold()}",
                return_code=browser_result.returncode,
                diagnostics_get_requests=diagnostics_get_requests,
                diagnostics_response_statuses=diagnostics_response_statuses,
            )
        browser_summary = tls.validate_browser(browser_result.stdout)
        stage = "commit-cleanup"
        commit_result = prototype.run_ssh(
            target, secret, build_commit_script(paths_value, assets), timeout=180
        )
        if commit_result.returncode != 0:
            raise tls.TlsGateError(stage, "remote-command", return_code=commit_result.returncode)
        committed = parse_facts(commit_result.stdout, COMMIT_KEYS)
        cleanup_required = False
        stage = "final-state"
        final_result = prototype.run_ssh(
            target,
            secret,
            provision.build_final_script(provision_paths, assets),
            timeout=60,
        )
        if final_result.returncode != 0:
            raise tls.TlsGateError(stage, "remote-command", return_code=final_result.returncode)
        final_state = provision.parse_facts(final_result.stdout, provision.FINAL_KEYS)
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-installed-runtime-tls-browser-gate",
            "result": "passed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "policy": {
                "tlsListener": "device-loopback-only",
                "browserTransport": "strict-host-key-ssh-forward",
                "certificate": "ephemeral-self-signed",
                "installedRuntime": True,
                "servicesEnabled": False,
                "controlServiceStarted": False,
                "workingNginxMutation": False,
                "persistentApplicationState": False,
                "physicalActionsInvoked": False,
                "secretEmitted": False,
            },
            "provisionedBefore": provisioned_before,
            "preflight": preflight,
            "started": started,
            "browser": browser_summary,
            "committed": committed,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (
        tls.TlsGateError,
        OSError,
        subprocess.SubprocessError,
        UnicodeError,
        ValueError,
    ) as error:
        cleanup: CleanupReceipt = {
            "attempted": False,
            "confirmed": not cleanup_required,
        }
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    build_cleanup_script(paths_value, assets),
                    timeout=180,
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
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup = {"attempted": True, "confirmed": False}
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-installed-runtime-tls-browser-gate",
            "result": "failed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "failure": {
                "stage": getattr(error, "stage", stage),
                "failureType": getattr(error, "failure_type", type(error).__name__),
                "returnCode": getattr(error, "return_code", None),
                "remotePhase": getattr(error, "remote_phase", None),
                "remoteCleanupConfirmed": getattr(
                    error, "remote_cleanup_confirmed", None
                ),
                "diagnosticsGetRequests": getattr(
                    error, "diagnostics_get_requests", None
                ),
                "diagnosticsResponseStatuses": getattr(
                    error, "diagnostics_response_statuses", None
                ),
            },
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        if tunnel is not None:
            tls.stop_process(tunnel)
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
