#!/usr/bin/env python3
"""Temporarily verify the credential-isolated broker and inventory on H616.

This test-only transition runner reuses the enrolled test device password for
SSH and the device's current KVMD administrator account. The password remains
in one mutable host buffer, reaches SSH and the temporary remote credential
file through separate inherited pipes, and is never placed in argv,
environment, source, receipts, or logs.

The runner invokes only the read-only inventory protocol. It never calls a
physical power, reset, HID, GPIO, media, update, or reboot operation.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import os
import re
import secrets
import shlex
import subprocess
from typing import TypedDict, cast

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


INVENTORY_REQUESTS = 20
RUNTIME_ROOT = "/run/hardened-blikvm"
PREFLIGHT_KEYS = {
    "architecture",
    "broker_identity_ready",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "kvmd_socket_ready",
    "port_free_before",
    "rootfs_mode_before",
    "runtime_absent",
    "server_identity_ready",
    "stage_absent",
    "tmp_available_bytes",
    "tmp_mount_mode",
    "tools_ready",
}
RUN_KEYS = {
    "broker_caps",
    "broker_gid",
    "broker_groups",
    "broker_mode",
    "broker_socket_group",
    "broker_socket_mode",
    "broker_socket_owner",
    "broker_stopped",
    "broker_uid",
    "bundle_sha256",
    "credential_files",
    "credential_isolation",
    "inventory_backend",
    "inventory_requests",
    "inventory_sanitized",
    "inventory_schema",
    "inventory_status",
    "kvmd_active_after_run",
    "kvmd_nginx_active_after_run",
    "no_new_privs",
    "payload_checksums",
    "rootfs_mode_after_run",
    "runtime_removed",
    "server_caps",
    "server_gid",
    "server_groups",
    "server_stopped",
    "server_uid",
    "setup_status",
    "login_status",
    "stage_removed",
}
FINAL_KEYS = {
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "port_free_final",
    "rootfs_mode_final",
    "runtime_absent_final",
    "stage_absent_final",
}
FAILURE_KEYS = {"failure_cleanup", "failure_phase"}
CLEANUP_KEYS = {"cleanup_removed"}
SAFE_PHASE = re.compile(r"^[a-z0-9-]{1,120}$")


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


class PolicyReceipt(TypedDict):
    kvmdAccess: str
    credentialMode: str
    brokerStartAuthorized: bool
    physicalActionsInvoked: bool
    rootfsHelpersInvoked: bool
    persistentPathsChanged: bool
    stagingFilesystems: list[str]


class FailureReceipt(TypedDict):
    stage: str
    failureType: str
    returnCode: int | None
    stdoutBytes: int
    remotePhase: str | None
    remoteCleanupConfirmed: bool | None


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    bundle: str
    expected_sha256: str
    output: str
    bind_port: int


@dataclass(frozen=True)
class BrokerPaths:
    stage: prototype.StagePaths
    runtime_root: str
    runtime_marker: str
    credentials: str
    broker: str
    broker_socket: str


class BrokerStageError(Exception):
    def __init__(
        self,
        stage: str,
        failure_type: str,
        *,
        return_code: int | None = None,
        stdout_bytes: int = 0,
        remote_phase: str | None = None,
        remote_cleanup_confirmed: bool | None = None,
    ) -> None:
        super().__init__(stage)
        self.stage: str = stage
        self.failure_type: str = failure_type
        self.return_code: int | None = return_code
        self.stdout_bytes: int = stdout_bytes
        self.remote_phase: str | None = remote_phase
        self.remote_cleanup_confirmed: bool | None = remote_cleanup_confirmed


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--bind-port", type=int, default=18081)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        bundle=cast(str, namespace.bundle),
        expected_sha256=cast(str, namespace.expected_sha256),
        output=cast(str, namespace.output),
        bind_port=cast(int, namespace.bind_port),
    )


def broker_paths(bundle_sha256: str, run_id: str) -> BrokerPaths:
    stage = prototype.stage_paths(bundle_sha256, run_id)
    runtime_marker = f"/run/.hardened-blikvm-owner-{run_id}"
    return BrokerPaths(
        stage=stage,
        runtime_root=RUNTIME_ROOT,
        runtime_marker=runtime_marker,
        credentials=f"{stage.root}/credentials",
        broker=f"{stage.payload}/libexec/bli-kvm-platform-broker",
        broker_socket=f"{RUNTIME_ROOT}/platform-broker.sock",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def _write_all(fd: int, value: bytes | bytearray) -> None:
    view = memoryview(value)
    offset = 0
    try:
        while offset < len(view):
            offset += os.write(fd, view[offset:])
    finally:
        view.release()


def run_ssh_with_secret_stdin(
    target: prototype.RemoteTarget,
    secret: bytearray,
    remote_script: str,
    *,
    timeout: float,
) -> subprocess.CompletedProcess[bytes]:
    read_fd, write_fd = os.pipe()
    try:
        try:
            _write_all(write_fd, secret)
            _write_all(write_fd, b"\n")
        finally:
            os.close(write_fd)
        return prototype.run_ssh(
            target,
            secret,
            remote_script,
            stdin=read_fd,
            timeout=timeout,
        )
    finally:
        os.close(read_fd)


def build_preflight_script(
    paths: BrokerPaths, required_bytes: int, bind_port: int
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "required_bytes": required_bytes,
            "port_hex": f"{bind_port:04X}",
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) tmp_mount_mode=exec ;; esac
tmp_available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
case "$tmp_available" in ''|*[!0-9]*) exit 1 ;; esac
[ "$tmp_available" -ge "$required_bytes" ]
[ "$(uname -m)" = aarch64 ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ -S /run/kvmd/kvmd.sock ]
for tool in awk cat chmod chown curl df env findmnt getent grep head id mkdir mv od readlink rm sed setpriv sha256sum sleep stat systemctl tail tar tr wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
nobody=$(getent passwd nobody | head -n 1)
daemon=$(getent passwd daemon | head -n 1)
nobody_uid=$(printf '%s\n' "$nobody" | awk -F: '{print $3}')
nobody_gid=$(printf '%s\n' "$nobody" | awk -F: '{print $4}')
daemon_uid=$(printf '%s\n' "$daemon" | awk -F: '{print $3}')
kvmd_gid=$(getent group kvmd | head -n 1 | awk -F: '{print $3}')
case "$nobody_uid:$nobody_gid:$daemon_uid:$kvmd_gid" in *[!0-9:]*) exit 1 ;; esac
[ "$nobody_uid" -gt 0 ]
[ "$nobody_gid" -gt 0 ]
[ "$daemon_uid" -gt 0 ]
[ "$kvmd_gid" -gt 0 ]
[ "$daemon_uid" != "$nobody_uid" ]
server_identity_ready=no
if setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --bounding-set=-all --no-new-privs /bin/true >/dev/null 2>&1; then
  server_identity_ready=yes
fi
broker_identity_ready=no
if setpriv --reuid="$daemon_uid" --regid="$nobody_gid" --groups="$kvmd_gid" --bounding-set=-all --no-new-privs /bin/true >/dev/null 2>&1; then
  broker_identity_ready=yes
fi
[ ! -e "$stage_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
if port_in_use; then exit 1; fi
emit architecture aarch64
emit rootfs_mode_before "$rootfs_mode"
emit tmp_mount_mode "$tmp_mount_mode"
emit tmp_available_bytes "$tmp_available"
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit kvmd_socket_ready yes
emit server_identity_ready "$server_identity_ready"
emit broker_identity_ready "$broker_identity_ready"
emit tools_ready yes
emit stage_absent yes
emit runtime_absent yes
emit port_free_before yes
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


def build_credential_upload_script(paths: BrokerPaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "run_id": paths.stage.run_id,
            "credentials": paths.credentials,
        }
    )
    body = r"""
set -eu
umask 077
[ -f "$stage_marker" ]
[ "$(cat "$stage_marker")" = "$run_id" ]
daemon_uid=$(getent passwd daemon | head -n 1 | awk -F: '{print $3}')
nobody_gid=$(getent passwd nobody | head -n 1 | awk -F: '{print $4}')
mkdir -m 700 "$credentials"
chown "$daemon_uid:$nobody_gid" "$credentials"
printf '%s\n' admin > "$credentials/kvmd-user"
cat > "$credentials/kvmd-password.part"
bytes=$(wc -c < "$credentials/kvmd-password.part" | tr -d ' ')
case "$bytes" in ''|*[!0-9]*) exit 1 ;; esac
[ "$bytes" -ge 2 ]
[ "$bytes" -le 257 ]
mv "$credentials/kvmd-password.part" "$credentials/kvmd-password"
chown "$daemon_uid:$nobody_gid" "$credentials/kvmd-user" "$credentials/kvmd-password"
chmod 600 "$credentials/kvmd-user" "$credentials/kvmd-password"
""".strip()
    return f"{values}\n{body}"


PROCESS_FUNCTIONS = r"""
process_active() {
  checked_pid=$1
  case "$checked_pid" in *[!0-9]*|'') return 1 ;; esac
  kill -0 "$checked_pid" 2>/dev/null || return 1
  checked_state=$(awk '{print $3}' "/proc/$checked_pid/stat" 2>/dev/null || true)
  [ -n "$checked_state" ] && [ "$checked_state" != Z ]
}
stop_child() {
  child_pid=$1
  child_exe=$2
  if process_active "$child_pid"; then
    [ "$(readlink "/proc/$child_pid/exe" 2>/dev/null || true)" = "$child_exe" ] || return 1
    kill -TERM "$child_pid"
    count=0
    while process_active "$child_pid" && [ "$count" -lt 50 ]; do sleep 0.1; count=$((count + 1)); done
    process_active "$child_pid" && return 1
  fi
  wait "$child_pid" 2>/dev/null || true
}
""".strip()


def build_runtime_script(
    paths: BrokerPaths, bundle: prototype.BundleInfo, bind_port: int
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "run_id": paths.stage.run_id,
            "archive": paths.stage.archive,
            "payload": paths.stage.payload,
            "server": paths.stage.server,
            "broker": paths.broker,
            "credentials": paths.credentials,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "broker_socket": paths.broker_socket,
            "expected_sha256": bundle.sha256,
            "bind_port": bind_port,
            "port_hex": f"{bind_port:04X}",
            "inventory_requests": INVENTORY_REQUESTS,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
umask 077
phase=validate-stage
run_succeeded=no
server_pid=
broker_pid=
emit() { printf '%s=%s\n' "$1" "$2"; }
owned_stage() {
  [ -f "$stage_marker" ] && [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
}
owned_runtime() {
  [ -f "$runtime_marker" ] && [ "$(cat "$runtime_marker" 2>/dev/null || true)" = "$run_id" ]
}
emergency_cleanup() {
  set +e
  safe=yes
  if [ -n "$server_pid" ] && process_active "$server_pid"; then
    if [ "$(readlink "/proc/$server_pid/exe" 2>/dev/null || true)" = "$server" ]; then
      kill -TERM "$server_pid" 2>/dev/null
      count=0
      while process_active "$server_pid" && [ "$count" -lt 30 ]; do sleep 0.1; count=$((count + 1)); done
      if process_active "$server_pid"; then
        kill -KILL "$server_pid" 2>/dev/null
        count=0
        while process_active "$server_pid" && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
      fi
      process_active "$server_pid" && safe=no
    else safe=no; fi
  fi
  if [ -n "$broker_pid" ] && process_active "$broker_pid"; then
    if [ "$(readlink "/proc/$broker_pid/exe" 2>/dev/null || true)" = "$broker" ]; then
      kill -TERM "$broker_pid" 2>/dev/null
      count=0
      while process_active "$broker_pid" && [ "$count" -lt 30 ]; do sleep 0.1; count=$((count + 1)); done
      if process_active "$broker_pid"; then
        kill -KILL "$broker_pid" 2>/dev/null
        count=0
        while process_active "$broker_pid" && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
      fi
      process_active "$broker_pid" && safe=no
    else safe=no; fi
  fi
  if [ -n "$server_pid" ] && ! process_active "$server_pid"; then wait "$server_pid" 2>/dev/null || true; fi
  if [ -n "$broker_pid" ] && ! process_active "$broker_pid"; then wait "$broker_pid" 2>/dev/null || true; fi
  if [ "$safe" = yes ] && owned_runtime; then rm -rf -- "$runtime_root"; rm -f -- "$runtime_marker"; fi
  if [ "$safe" = yes ] && owned_stage; then rm -rf -- "$stage_root"; fi
}
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    emergency_cleanup
    if [ ! -e "$stage_root" ] && [ ! -e "$runtime_root" ] && [ ! -e "$runtime_marker" ]; then cleanup_state=removed; else cleanup_state=retained; fi
    emit failure_phase "$phase"
    emit failure_cleanup "$cleanup_state"
  fi
  exit "$exit_code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
owned_stage
[ -f "$archive" ]
[ -d "$credentials" ]
phase=verify-archive
actual_sha256=$(sha256sum "$archive" | awk '{print $1}')
[ "$actual_sha256" = "$expected_sha256" ]
phase=extract-payload
mkdir -m 755 "$payload"
tar -xzf "$archive" --strip-components=1 --no-same-owner -C "$payload"
phase=verify-payload
(cd "$payload" && sha256sum -c SHA256SUMS >/dev/null 2>&1)
[ "$(stat -c '%a' "$server")" = 755 ]
[ "$(stat -c '%a' "$broker")" = 755 ]
daemon_uid=$(getent passwd daemon | head -n 1 | awk -F: '{print $3}')
nobody_uid=$(getent passwd nobody | head -n 1 | awk -F: '{print $3}')
nobody_gid=$(getent passwd nobody | head -n 1 | awk -F: '{print $4}')
kvmd_gid=$(getent group kvmd | head -n 1 | awk -F: '{print $3}')
[ "$(stat -c '%u' "$credentials")" = "$daemon_uid" ]
[ "$(stat -c '%a' "$credentials")" = 700 ]
[ "$(stat -c '%a' "$credentials/kvmd-user")" = 600 ]
[ "$(stat -c '%a' "$credentials/kvmd-password")" = 600 ]
chmod 711 "$stage_root"
if setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --no-new-privs cat "$credentials/kvmd-password" >/dev/null 2>&1; then exit 1; fi
phase=prepare-runtime
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
printf '%s\n' "$run_id" > "$runtime_marker"
chmod 600 "$runtime_marker"
mkdir -m 750 "$runtime_root"
chown "$daemon_uid:$nobody_gid" "$runtime_root"
: > "$stage_root/broker.log"
chmod 600 "$stage_root/broker.log"
phase=launch-broker
setpriv --reuid="$daemon_uid" --regid="$nobody_gid" --groups="$kvmd_gid" --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME=/nonexistent TMPDIR=/tmp CREDENTIALS_DIRECTORY="$credentials" \
  "$broker" > "$stage_root/broker.log" 2>&1 < /dev/null &
broker_pid=$!
printf '%s\n' "$broker_pid" > "$stage_root/broker.pid"
phase=wait-broker
count=0
while [ "$count" -lt 50 ] && [ ! -S "$broker_socket" ]; do
  process_active "$broker_pid" || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S "$broker_socket" ]
phase=verify-broker
broker_status="/proc/$broker_pid/status"
[ "$(awk '/^Uid:/ {print $3}' "$broker_status")" = "$daemon_uid" ]
[ "$(awk '/^Gid:/ {print $3}' "$broker_status")" = "$nobody_gid" ]
[ "$(awk '/^CapEff:/ {print $2}' "$broker_status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "$broker_status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "$broker_status")" = 1 ]
broker_groups=$(awk '/^Groups:/ {$1=""; sub(/^ +/, ""); print}' "$broker_status")
case " $broker_groups " in *" $kvmd_gid "*) ;; *) exit 1 ;; esac
case " $broker_groups " in *' 0 '*) exit 1 ;; esac
[ "$(stat -c '%u' "$broker_socket")" = "$daemon_uid" ]
[ "$(stat -c '%g' "$broker_socket")" = "$nobody_gid" ]
[ "$(stat -c '%a' "$broker_socket")" = 660 ]
phase=prepare-server
state="$stage_root/state"
mkdir -m 700 "$state"
chown "$nobody_uid:$nobody_gid" "$state"
: > "$stage_root/server.log"
chmod 600 "$stage_root/server.log"
phase=launch-server
setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME="$state" TMPDIR="$state" \
  BLI_KVM_BIND="127.0.0.1:$bind_port" BLI_KVM_WEB_ROOT="$payload/share/web" \
  BLI_KVM_STATE_DIR="$state" BLI_KVM_BROKER_SOCKET="$broker_socket" \
  BLI_KVM_SECURE_COOKIES=0 BLI_KVM_ALLOW_MISSING_ORIGIN=0 \
  "$server" > "$stage_root/server.log" 2>&1 < /dev/null &
server_pid=$!
printf '%s\n' "$server_pid" > "$stage_root/server.pid"
phase=wait-server
ready=no
count=0
while [ "$count" -lt 50 ]; do
  if curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null "http://127.0.0.1:$bind_port/api/health" 2>/dev/null; then ready=yes; break; fi
  process_active "$server_pid" || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ "$ready" = yes ]
phase=verify-server
server_status="/proc/$server_pid/status"
[ "$(awk '/^Uid:/ {print $3}' "$server_status")" = "$nobody_uid" ]
[ "$(awk '/^Gid:/ {print $3}' "$server_status")" = "$nobody_gid" ]
[ "$(awk '/^CapEff:/ {print $2}' "$server_status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "$server_status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "$server_status")" = 1 ]
[ -z "$(awk '/^Groups:/ {$1=""; sub(/^ +/, ""); print}' "$server_status")" ]
if tr '\0' '\n' < "/proc/$server_pid/environ" | grep -Eq '^(CREDENTIALS_DIRECTORY|KVMD_)='; then exit 1; fi
phase=authenticate
account_password=$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')
case "$account_password" in ?????*) ;; *) exit 1 ;; esac
request="$stage_root/account-request.json"
printf '{"username":"prototype-admin","password":"%s"}' "$account_password" > "$request"
origin="http://127.0.0.1:$bind_port"
setup_status=$(curl --noproxy '*' --silent --max-time 3 --output "$stage_root/setup-response.json" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request" "$origin/api/auth/setup" 2>/dev/null)
[ "$setup_status" = 200 ]
cookie="$stage_root/session.cookies"
login_status=$(curl --noproxy '*' --silent --max-time 3 --cookie-jar "$cookie" --output "$stage_root/login-response.json" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request" "$origin/api/auth/login" 2>/dev/null)
[ "$login_status" = 200 ]
grep -q 'hardened-blikvm-session' "$cookie"
unset account_password
rm -f -- "$request"
phase=inventory
inventory_status=$(curl --noproxy '*' --silent --max-time 12 --cookie "$cookie" --output "$stage_root/inventory.json" --write-out '%{http_code}' "$origin/api/inventory" 2>/dev/null)
[ "$inventory_status" = 200 ]
grep -q '"schemaVersion":1' "$stage_root/inventory.json"
grep -q '"platformId":"bli-h616-pikvm"' "$stage_root/inventory.json"
grep -q '"backend":"kvmd_transition"' "$stage_root/inventory.json"
if grep -Eqi '"(password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress)"' "$stage_root/inventory.json"; then exit 1; fi
count=1
while [ "$count" -lt "$inventory_requests" ]; do
  code=$(curl --noproxy '*' --silent --max-time 12 --cookie "$cookie" --output /dev/null --write-out '%{http_code}' "$origin/api/inventory" 2>/dev/null)
  [ "$code" = 200 ]
  count=$((count + 1))
done
phase=verify-existing-services
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=stop-server
stop_child "$server_pid" "$server"
server_pid=
phase=stop-broker
stop_child "$broker_pid" "$broker"
broker_pid=
phase=remove-runtime
owned_runtime
rm -rf -- "$runtime_root"
rm -f -- "$runtime_marker"
phase=remove-stage
owned_stage
rm -rf -- "$stage_root"
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
[ ! -e "$stage_root" ]
run_succeeded=yes
trap - EXIT HUP INT TERM
emit bundle_sha256 "$actual_sha256"
emit payload_checksums verified
emit credential_files private
emit credential_isolation verified
emit broker_mode 755
emit broker_uid unprivileged-distinct
emit broker_gid shared-socket-group
emit broker_groups kvmd-only
emit broker_caps none
emit broker_socket_mode 660
emit broker_socket_owner broker
emit broker_socket_group server-shared
emit server_uid unprivileged-distinct
emit server_gid shared-socket-group
emit server_groups cleared
emit server_caps none
emit no_new_privs yes
emit setup_status "$setup_status"
emit login_status "$login_status"
emit inventory_status "$inventory_status"
emit inventory_schema 1
emit inventory_backend kvmd_transition
emit inventory_sanitized yes
emit inventory_requests "$inventory_requests"
emit kvmd_active_after_run active
emit kvmd_nginx_active_after_run active
emit rootfs_mode_after_run "$rootfs_mode"
emit server_stopped yes
emit broker_stopped yes
emit runtime_removed yes
emit stage_removed yes
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{PROCESS_FUNCTIONS}\n{body}"


def build_cleanup_script(paths: BrokerPaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "run_id": paths.stage.run_id,
            "server": paths.stage.server,
            "broker": paths.broker,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
stop_if_owned() {
  pid_file=$1
  expected_exe=$2
  [ -f "$pid_file" ] || return 0
  candidate=$(cat "$pid_file")
  case "$candidate" in *[!0-9]*|'') return 1 ;; esac
  if process_active "$candidate"; then
    [ "$(readlink "/proc/$candidate/exe" 2>/dev/null || true)" = "$expected_exe" ] || return 1
    kill -TERM "$candidate"
    count=0
    while process_active "$candidate" && [ "$count" -lt 50 ]; do sleep 0.1; count=$((count + 1)); done
    if process_active "$candidate"; then kill -KILL "$candidate"; fi
    count=0
    while process_active "$candidate" && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
    process_active "$candidate" && return 1
  fi
}
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ]
  [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
  stop_if_owned "$stage_root/server.pid" "$server"
  stop_if_owned "$stage_root/broker.pid" "$broker"
fi
if [ -e "$runtime_root" ] || [ -e "$runtime_marker" ]; then
  [ -f "$runtime_marker" ]
  [ "$(cat "$runtime_marker" 2>/dev/null || true)" = "$run_id" ]
  rm -rf -- "$runtime_root"
  rm -f -- "$runtime_marker"
fi
if [ -e "$stage_root" ]; then rm -rf -- "$stage_root"; fi
[ ! -e "$stage_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
emit cleanup_removed yes
""".strip()
    return f"{values}\n{PROCESS_FUNCTIONS}\n{body}"


def build_final_script(paths: BrokerPaths, bind_port: int) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "port_hex": f"{bind_port:04X}",
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
if port_in_use; then exit 1; fi
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
emit stage_absent_final yes
emit runtime_absent_final yes
emit port_free_final yes
emit rootfs_mode_final "$rootfs_mode"
emit kvmd_active_final active
emit kvmd_nginx_active_final active
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


def parse_failure(raw: bytes) -> tuple[str | None, bool | None]:
    try:
        facts = prototype.parse_facts(raw, FAILURE_KEYS)
    except (UnicodeError, ValueError):
        return None, None
    phase = facts["failure_phase"]
    cleanup = facts["failure_cleanup"]
    if not SAFE_PHASE.fullmatch(phase) or cleanup not in {"removed", "retained"}:
        return None, None
    return phase, cleanup == "removed"


def require_result(
    stage: str, result: subprocess.CompletedProcess[bytes]
) -> subprocess.CompletedProcess[bytes]:
    if result.returncode != 0:
        remote_phase: str | None = None
        remote_cleanup: bool | None = None
        if stage == "runtime":
            remote_phase, remote_cleanup = parse_failure(result.stdout)
        raise BrokerStageError(
            stage,
            "remote-command",
            return_code=result.returncode,
            stdout_bytes=len(result.stdout),
            remote_phase=remote_phase,
            remote_cleanup_confirmed=remote_cleanup,
        )
    if len(result.stdout) > prototype.MAX_REMOTE_OUTPUT_BYTES:
        raise BrokerStageError(stage, "output-limit", stdout_bytes=len(result.stdout))
    return result


def validate_preflight(facts: dict[str, str], required_bytes: int) -> None:
    expected = {
        "architecture": "aarch64",
        "rootfs_mode_before": "rw",
        "tmp_mount_mode": "exec",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "kvmd_socket_ready": "yes",
        "server_identity_ready": "yes",
        "broker_identity_ready": "yes",
        "tools_ready": "yes",
        "stage_absent": "yes",
        "runtime_absent": "yes",
        "port_free_before": "yes",
    }
    if any(facts.get(key) != value for key, value in expected.items()):
        raise ValueError("broker preflight invariant failed")
    available = facts.get("tmp_available_bytes", "")
    if not available.isdigit() or int(available) < required_bytes:
        raise ValueError("temporary broker staging space is insufficient")


def validate_run(facts: dict[str, str], bundle_sha256: str) -> None:
    expected = {
        "bundle_sha256": bundle_sha256,
        "payload_checksums": "verified",
        "credential_files": "private",
        "credential_isolation": "verified",
        "broker_mode": "755",
        "broker_uid": "unprivileged-distinct",
        "broker_gid": "shared-socket-group",
        "broker_groups": "kvmd-only",
        "broker_caps": "none",
        "broker_socket_mode": "660",
        "broker_socket_owner": "broker",
        "broker_socket_group": "server-shared",
        "server_uid": "unprivileged-distinct",
        "server_gid": "shared-socket-group",
        "server_groups": "cleared",
        "server_caps": "none",
        "no_new_privs": "yes",
        "setup_status": "200",
        "login_status": "200",
        "inventory_status": "200",
        "inventory_schema": "1",
        "inventory_backend": "kvmd_transition",
        "inventory_sanitized": "yes",
        "inventory_requests": str(INVENTORY_REQUESTS),
        "kvmd_active_after_run": "active",
        "kvmd_nginx_active_after_run": "active",
        "rootfs_mode_after_run": "rw",
        "server_stopped": "yes",
        "broker_stopped": "yes",
        "runtime_removed": "yes",
        "stage_removed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("broker runtime invariant failed")


def validate_final(facts: dict[str, str]) -> None:
    expected = {
        "stage_absent_final": "yes",
        "runtime_absent_final": "yes",
        "port_free_final": "yes",
        "rootfs_mode_final": "rw",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("broker final-state invariant failed")


def policy_receipt() -> PolicyReceipt:
    return {
        "kvmdAccess": "five-read-only-GET-endpoints",
        "credentialMode": "test-default-reused-through-secret-stdin",
        "brokerStartAuthorized": True,
        "physicalActionsInvoked": False,
        "rootfsHelpersInvoked": False,
        "persistentPathsChanged": False,
        "stagingFilesystems": ["/tmp", "/run"],
    }


def cleanup_receipt(
    target: prototype.RemoteTarget,
    secret: bytearray,
    paths: BrokerPaths,
) -> CleanupReceipt:
    try:
        result = prototype.run_ssh(
            target, secret, build_cleanup_script(paths), timeout=25
        )
        if result.returncode != 0:
            return {"attempted": True, "confirmed": False}
        facts = prototype.parse_facts(result.stdout, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_removed": "yes"}}
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    expected_sha256 = prototype.validate_sha256(options.expected_sha256)
    bundle = prototype.validate_bundle(options.bundle, expected_sha256)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    bind_port = prototype.validate_bind_port(options.bind_port)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths = broker_paths(bundle.sha256, secrets.token_hex(8))
    required_bytes = bundle.size_bytes * 4 + prototype.MIN_TMP_HEADROOM_BYTES
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    stage = "initialization"
    try:
        stage = "preflight"
        preflight_result = require_result(
            stage,
            prototype.run_ssh(
                target,
                secret,
                build_preflight_script(paths, required_bytes, bind_port),
                timeout=20,
            ),
        )
        preflight = prototype.parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        validate_preflight(preflight, required_bytes)

        stage = "upload"
        cleanup_required = True
        with bundle.path.open("rb") as bundle_stream:
            _ = require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    prototype.build_upload_script(paths.stage),
                    stdin=bundle_stream,
                    timeout=60,
                ),
            )

        stage = "credential-upload"
        _ = require_result(
            stage,
            run_ssh_with_secret_stdin(
                target,
                secret,
                build_credential_upload_script(paths),
                timeout=20,
            ),
        )

        stage = "runtime"
        run_result = require_result(
            stage,
            prototype.run_ssh(
                target,
                secret,
                build_runtime_script(paths, bundle, bind_port),
                timeout=240,
            ),
        )
        checks = prototype.parse_facts(run_result.stdout, RUN_KEYS)
        validate_run(checks, bundle.sha256)

        stage = "final-state"
        final_result = require_result(
            stage,
            prototype.run_ssh(
                target, secret, build_final_script(paths, bind_port), timeout=20
            ),
        )
        final_state = prototype.parse_facts(final_result.stdout, FINAL_KEYS)
        validate_final(final_state)
        cleanup_required = False

        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-broker-inventory",
            "result": "passed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(),
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except BrokerStageError as error:
        cleanup = (
            cleanup_receipt(target, secret, paths)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        failure: FailureReceipt = {
            "stage": error.stage,
            "failureType": error.failure_type,
            "returnCode": error.return_code,
            "stdoutBytes": error.stdout_bytes,
            "remotePhase": error.remote_phase,
            "remoteCleanupConfirmed": error.remote_cleanup_confirmed,
        }
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-broker-inventory",
            "result": "failed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(),
            "failure": failure,
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            cleanup_receipt(target, secret, paths)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-broker-inventory",
            "result": "failed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(),
            "failure": {
                "stage": stage,
                "failureType": type(error).__name__,
                "returnCode": None,
                "stdoutBytes": 0,
                "remotePhase": None,
                "remoteCleanupConfirmed": None,
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
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
