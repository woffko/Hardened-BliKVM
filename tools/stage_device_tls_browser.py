#!/usr/bin/env python3
"""Run the H616 TLS reverse-proxy and real-browser gate through SSH forwarding."""

from __future__ import annotations

import argparse
import base64
from collections.abc import Callable
from dataclasses import dataclass
import json
import os
from pathlib import Path
import re
import select
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import time
from typing import IO, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


TLS_PORT = 18443
DIAGNOSTIC_RUNTIME = "/run/hardened-blikvm-diagnostics"
DIAGNOSTIC_SOCKET = f"{DIAGNOSTIC_RUNTIME}/collector.sock"
PREFLIGHT_KEYS = {
    "architecture",
    "browser_runtime_ready",
    "diagnostic_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "nginx_ready",
    "ports_free_before",
    "rootfs_mode_before",
    "stage_absent",
    "tmp_mount_mode",
    "tools_ready",
    "ustreamer_socket",
    "ustreamer_socket_group",
    "ustreamer_socket_mode",
}
RUN_KEYS = {
    "broker_stopped",
    "bundle_sha256",
    "credential_isolation",
    "control_broker_stopped",
    "control_denials",
    "control_no_backend",
    "control_queue_pressure",
    "control_response_latency",
    "diagnostic_event",
    "diagnostic_runtime_removed",
    "diagnostic_stall",
    "nginx_config",
    "nginx_stopped",
    "payload_checksums",
    "ready",
    "rootfs_mode_after_run",
    "server_stopped",
    "stage_removed",
    "tls12",
    "tls13",
    "tls_loopback",
    "video_proxy_stopped",
}
FINAL_KEYS = {
    "diagnostic_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "ports_free_final",
    "rootfs_mode_final",
    "stage_absent_final",
}
FAILURE_KEYS = {"failure_cleanup", "failure_phase"}
CLEANUP_KEYS = {"cleanup_removed"}


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


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
class TlsPaths:
    stage: prototype.StagePaths
    runtime_root: str
    runtime_marker: str
    credentials: str
    broker_binary: str
    broker_socket: str
    control_binary: str
    control_runtime: str
    control_socket: str
    diagnostic_runtime: str
    diagnostic_runtime_marker: str
    diagnostic_socket: str
    diagnostic_capture: str
    diagnostic_seen: str
    video_binary: str
    video_runtime: str
    video_socket: str
    nginx_state: str
    nginx_config: str
    tls_certificate: str
    tls_key: str
    ready: str
    browser_done: str
    browser_result: str
    nginx_binary: str


@dataclass(frozen=True)
class BrowserRuntime:
    node: Path
    module: Path
    executable: Path
    script: Path


class TlsGateError(Exception):
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


def tls_paths(bundle_sha256: str, run_id: str) -> TlsPaths:
    stage = prototype.stage_paths(bundle_sha256, run_id)
    root = stage.root
    return TlsPaths(
        stage=stage,
        runtime_root="/run/hardened-blikvm",
        runtime_marker=f"/run/.hardened-blikvm-owner-{run_id}",
        credentials=f"{root}/credentials",
        broker_binary=f"{stage.payload}/libexec/bli-kvm-platform-broker",
        broker_socket="/run/hardened-blikvm/platform-broker.sock",
        control_binary=f"{stage.payload}/libexec/bli-kvm-control-broker",
        control_runtime=f"{root}/control-runtime",
        control_socket=f"{root}/control-runtime/control.sock",
        diagnostic_runtime=DIAGNOSTIC_RUNTIME,
        diagnostic_runtime_marker=f"/run/.hardened-blikvm-diagnostics-owner-{run_id}",
        diagnostic_socket=DIAGNOSTIC_SOCKET,
        diagnostic_capture=f"{root}/diagnostic-capture.py",
        diagnostic_seen=f"{root}/diagnostic.seen",
        video_binary=f"{stage.payload}/libexec/bli-kvm-video-proxy",
        video_runtime=f"{root}/video-runtime",
        video_socket=f"{root}/video-runtime/mjpeg.sock",
        nginx_state=f"{root}/nginx-state",
        nginx_config=f"{root}/nginx.conf",
        tls_certificate=f"{root}/tls.crt",
        tls_key=f"{root}/tls.key",
        ready=f"{root}/browser.ready",
        browser_done=f"{root}/browser.done",
        browser_result=f"{root}/browser.result",
        nginx_binary="/usr/sbin/nginx",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


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

DIAGNOSTIC_CAPTURE = r"""
import os
import socket
import sys
import time

socket_path, seen_path = sys.argv[1:]
expected = bytes((1, 3, 2, 0, 0, 0, 0, 0))

listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
connection = None
try:
    listener.bind(socket_path)
    os.chmod(socket_path, 0o660)
    listener.listen(1)
    connection, _ = listener.accept()
    listener.close()
    connection.settimeout(3)
    frame = b""
    while len(frame) < len(expected):
        chunk = connection.recv(len(expected) - len(frame))
        if not chunk:
            raise RuntimeError("short diagnostic frame")
        frame += chunk
    if frame != expected:
        raise RuntimeError("unexpected diagnostic frame")
    descriptor = os.open(seen_path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    time.sleep(7)
finally:
    if connection is not None:
        connection.close()
    listener.close()
    try:
        os.unlink(socket_path)
    except FileNotFoundError:
        pass
""".strip()

CONTROL_PROBE = r"""
import json
import socket
import struct
import sys
import time

socket_path = sys.argv[1]
base_requests = [
    ("hid", {"type": "hid", "report": {"mode": "keyboard", "data": [0] * 8}}),
    ("atx", {"type": "atx", "action": "power_short"}),
    ("media", {"type": "virtual_media", "command": {"action": "set_connected", "connected": False}}),
]
requests = base_requests + [base_requests[0]] * 29

def read_exact(stream, length):
    value = b""
    while len(value) < length:
        chunk = stream.recv(length - len(value))
        if not chunk:
            raise RuntimeError("short control response")
        value += chunk
    return value

started = time.monotonic()
maximum = 0.0
observed = {}
for request_id, (name, request) in enumerate(requests, 1):
    request_started = time.monotonic()
    operation_id = f"{request_id:032x}"
    envelope = {
        "schemaVersion": 1,
        "requestId": request_id,
        "operationId": operation_id,
        "request": request,
    }
    payload = json.dumps(envelope, separators=(",", ":")).encode("ascii")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(3)
        stream.connect(socket_path)
        stream.sendall(struct.pack(">I", len(payload)) + payload)
        stream.shutdown(socket.SHUT_WR)
        length = struct.unpack(">I", read_exact(stream, 4))[0]
        if length > 4096:
            raise RuntimeError("oversized control response")
        response = json.loads(read_exact(stream, length))
    expected = {
        "schemaVersion": 1,
        "requestId": request_id,
        "operationId": operation_id,
        "response": {"type": "rejected", "code": "capability_disabled"},
    }
    if response != expected:
        raise RuntimeError("unexpected control response")
    elapsed = time.monotonic() - request_started
    maximum = max(maximum, elapsed)
    observed[name] = "capability_disabled"

total = time.monotonic() - started
if maximum >= 1.0 or total >= 4.0:
    raise RuntimeError("control response latency exceeded isolation bound")
for name in ("hid", "atx", "media"):
    print(f"{name}={observed[name]}")
print("requests=32")
print("latency=bounded")
""".strip()


def build_credential_upload_script(paths: TlsPaths) -> str:
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


def browser_runtime() -> BrowserRuntime:
    home = Path.home()
    node_name = shutil.which("node")
    if not node_name:
        raise ValueError("node is unavailable")
    module = home / ".local/share/playwright/node_modules/playwright/index.mjs"
    candidates = sorted(
        (home / ".cache/ms-playwright").glob("chromium-*/chrome-linux64/chrome"),
        reverse=True,
    )
    script = Path(__file__).resolve().parent / "playwright/device_tls_auth_smoke.mjs"
    if not module.is_file() or not candidates or not os.access(candidates[0], os.X_OK):
        raise ValueError("local Playwright Chromium runtime is unavailable")
    if not script.is_file():
        raise ValueError("browser smoke script is unavailable")
    return BrowserRuntime(Path(node_name).resolve(), module, candidates[0], script)


def render_test_nginx(paths: TlsPaths, bind_port: int) -> str:
    template = (
        Path(__file__).resolve().parent.parent
        / "packaging/nginx/hardened-blikvm.conf"
    ).read_text()
    template = template.replace("    listen 443 ssl;", f"    listen 127.0.0.1:{TLS_PORT} ssl;")
    template = template.replace("    listen [::]:443 ssl;\n", "")
    template = template.replace(
        "/etc/hardened-blikvm/tls.crt", paths.tls_certificate
    )
    template = template.replace("/etc/hardened-blikvm/tls.key", paths.tls_key)
    template = template.replace("http://127.0.0.1:18080", f"http://127.0.0.1:{bind_port}")
    state = paths.nginx_state
    return f"""worker_processes 1;
pid {state}/nginx.pid;
error_log {state}/error.log notice;

events {{
    worker_connections 128;
}}

http {{
    access_log off;
    include /etc/nginx/mime.types;
    client_body_temp_path {state}/client-body;
    proxy_temp_path {state}/proxy;
{template}
}}
"""


def build_preflight_script(
    paths: TlsPaths, required_bytes: int, bind_port: int
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "diagnostic_runtime": paths.diagnostic_runtime,
            "diagnostic_runtime_marker": paths.diagnostic_runtime_marker,
            "diagnostic_socket": paths.diagnostic_socket,
            "required_bytes": required_bytes,
            "server_port_hex": f"{bind_port:04X}",
            "tls_port_hex": f"{TLS_PORT:04X}",
            "nginx_binary": paths.nginx_binary,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) tmp_mount_mode=exec ;; esac
tmp_available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
[ "$tmp_available" -ge "$required_bytes" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ -S /run/kvmd/kvmd.sock ]
[ -S /run/kvmd/ustreamer.sock ]
ustreamer_mode=$(stat -c '%a' /run/kvmd/ustreamer.sock)
ustreamer_group=$(stat -c '%G' /run/kvmd/ustreamer.sock)
[ "$ustreamer_mode" = 660 ]
[ "$ustreamer_group" = kvmd ]
for tool in awk base64 cat chmod chown curl df env find findmnt getent grep head mkdir mv openssl python3 readlink rm rmdir sed setpriv sha256sum sleep stat systemctl tail tar tr wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
[ -x "$nginx_binary" ]
[ -f /etc/nginx/mime.types ]
getent passwd daemon >/dev/null
getent passwd nobody >/dev/null
getent passwd www-data >/dev/null
getent group kvmd >/dev/null
[ ! -e "$stage_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
[ ! -e "$diagnostic_runtime" ]
[ ! -e "$diagnostic_runtime_marker" ]
[ ! -e "$diagnostic_socket" ]
port_hex=$server_port_hex
if port_in_use; then exit 1; fi
port_hex=$tls_port_hex
if port_in_use; then exit 1; fi
emit architecture aarch64
emit rootfs_mode_before "$rootfs_mode"
emit tmp_mount_mode "$tmp_mount_mode"
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit nginx_ready yes
emit browser_runtime_ready yes
emit diagnostic_absent_before yes
emit tools_ready yes
emit stage_absent yes
emit ports_free_before yes
emit ustreamer_socket present
emit ustreamer_socket_mode "$ustreamer_mode"
emit ustreamer_socket_group "$ustreamer_group"
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


def build_remote_script(
    paths: TlsPaths, bundle: prototype.BundleInfo, bind_port: int
) -> str:
    nginx_b64 = base64.b64encode(
        render_test_nginx(paths, bind_port).encode("utf-8")
    ).decode("ascii")
    control_probe_b64 = base64.b64encode(CONTROL_PROBE.encode("utf-8")).decode(
        "ascii"
    )
    diagnostic_capture_b64 = base64.b64encode(
        DIAGNOSTIC_CAPTURE.encode("utf-8")
    ).decode("ascii")
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "run_id": paths.stage.run_id,
            "archive": paths.stage.archive,
            "payload": paths.stage.payload,
            "server": paths.stage.server,
            "broker": paths.broker_binary,
            "credentials": paths.credentials,
            "broker_socket": paths.broker_socket,
            "control": paths.control_binary,
            "control_runtime": paths.control_runtime,
            "control_socket": paths.control_socket,
            "diagnostic_runtime": paths.diagnostic_runtime,
            "diagnostic_runtime_marker": paths.diagnostic_runtime_marker,
            "diagnostic_socket": paths.diagnostic_socket,
            "diagnostic_capture": paths.diagnostic_capture,
            "diagnostic_seen": paths.diagnostic_seen,
            "video": paths.video_binary,
            "video_runtime": paths.video_runtime,
            "video_socket": paths.video_socket,
            "expected_sha256": bundle.sha256,
            "bind_port": bind_port,
            "tls_port": TLS_PORT,
            "nginx_binary": paths.nginx_binary,
            "nginx_state": paths.nginx_state,
            "nginx_config": paths.nginx_config,
            "tls_certificate": paths.tls_certificate,
            "tls_key": paths.tls_key,
            "ready_file": paths.ready,
            "browser_done": paths.browser_done,
            "browser_result": paths.browser_result,
            "nginx_b64": nginx_b64,
            "control_probe_b64": control_probe_b64,
            "diagnostic_capture_b64": diagnostic_capture_b64,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
umask 077
phase=validate-stage
run_succeeded=no
broker_pid=
control_pid=
diagnostic_pid=
video_pid=
server_pid=
nginx_pid=
python_binary=
emit() { printf '%s=%s\n' "$1" "$2"; }
owned_stage() {
  [ -f "$stage_marker" ] && [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
}
owned_runtime() {
  [ -f "$runtime_marker" ] && [ "$(cat "$runtime_marker" 2>/dev/null || true)" = "$run_id" ]
}
owned_diagnostics() {
  [ -f "$diagnostic_runtime_marker" ] && [ "$(cat "$diagnostic_runtime_marker" 2>/dev/null || true)" = "$run_id" ]
}
cleanup_diagnostics() {
  owned_diagnostics || return 1
  if [ -e "$diagnostic_runtime" ]; then
    [ -d "$diagnostic_runtime" ]
    [ ! -L "$diagnostic_runtime" ]
    unexpected=$(find "$diagnostic_runtime" -mindepth 1 -maxdepth 1 ! -name collector.sock -print -quit)
    [ -z "$unexpected" ]
    if [ -e "$diagnostic_socket" ]; then
      [ -S "$diagnostic_socket" ]
      rm -f -- "$diagnostic_socket"
    fi
    rmdir "$diagnostic_runtime"
  fi
  rm -f -- "$diagnostic_runtime_marker"
}
emergency_cleanup() {
  set +e
  safe=yes
  for pair in "$nginx_pid:$nginx_binary" "$server_pid:$server" "$video_pid:$video" "$control_pid:$control" "$diagnostic_pid:$python_binary" "$broker_pid:$broker"; do
    child_pid=${pair%%:*}
    child_exe=${pair#*:}
    [ -n "$child_pid" ] || continue
    if process_active "$child_pid"; then
      if [ "$(readlink "/proc/$child_pid/exe" 2>/dev/null || true)" = "$child_exe" ]; then
        kill -TERM "$child_pid" 2>/dev/null
        count=0
        while process_active "$child_pid" && [ "$count" -lt 40 ]; do sleep 0.1; count=$((count + 1)); done
        if process_active "$child_pid"; then
          kill -KILL "$child_pid" 2>/dev/null
          count=0
          while process_active "$child_pid" && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
        fi
        process_active "$child_pid" && safe=no
      else safe=no; fi
    fi
  done
  if [ "$safe" = yes ] && { [ -e "$diagnostic_runtime" ] || [ -e "$diagnostic_runtime_marker" ]; }; then cleanup_diagnostics || safe=no; fi
  if [ "$safe" = yes ] && owned_runtime; then rm -rf -- "$runtime_root"; rm -f -- "$runtime_marker"; fi
  if [ "$safe" = yes ] && owned_stage; then rm -rf -- "$stage_root"; fi
}
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    emergency_cleanup
    if [ ! -e "$stage_root" ] && [ ! -e "$runtime_root" ] && [ ! -e "$runtime_marker" ] && [ ! -e "$diagnostic_runtime" ] && [ ! -e "$diagnostic_runtime_marker" ]; then cleanup_state=removed; else cleanup_state=retained; fi
    emit failure_phase "$phase"
    emit failure_cleanup "$cleanup_state"
  fi
  exit "$exit_code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
owned_stage
[ -f "$archive" ]
phase=verify-bundle
actual_sha256=$(sha256sum "$archive" | awk '{print $1}')
[ "$actual_sha256" = "$expected_sha256" ]
mkdir -m 755 "$payload"
tar -xzf "$archive" --strip-components=1 --no-same-owner -C "$payload"
(cd "$payload" && sha256sum -c SHA256SUMS >/dev/null 2>&1)
[ -x "$video" ]
[ -x "$control" ]
python_binary=$(readlink -f "$(command -v python3)")
[ -x "$python_binary" ]
daemon_uid=$(getent passwd daemon | head -n 1 | awk -F: '{print $3}')
nobody_uid=$(getent passwd nobody | head -n 1 | awk -F: '{print $3}')
nobody_gid=$(getent passwd nobody | head -n 1 | awk -F: '{print $4}')
www_uid=$(getent passwd www-data | head -n 1 | awk -F: '{print $3}')
www_gid=$(getent passwd www-data | head -n 1 | awk -F: '{print $4}')
kvmd_gid=$(getent group kvmd | head -n 1 | awk -F: '{print $3}')
chmod 711 "$stage_root"
if setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --no-new-privs cat "$credentials/kvmd-password" >/dev/null 2>&1; then exit 1; fi
phase=prepare-runtime
printf '%s\n' "$run_id" > "$runtime_marker"
chmod 600 "$runtime_marker"
mkdir -m 750 "$runtime_root"
chown "$daemon_uid:$nobody_gid" "$runtime_root"
: > "$stage_root/broker.log"
: > "$stage_root/control.log"
: > "$stage_root/diagnostic.log"
: > "$stage_root/video.log"
: > "$stage_root/server.log"
: > "$stage_root/nginx.log"
chmod 600 "$stage_root"/*.log
phase=start-broker
setpriv --reuid="$daemon_uid" --regid="$nobody_gid" --groups="$kvmd_gid" --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME=/nonexistent TMPDIR=/tmp CREDENTIALS_DIRECTORY="$credentials" \
  "$broker" > "$stage_root/broker.log" 2>&1 < /dev/null &
broker_pid=$!
printf '%s\n' "$broker_pid" > "$stage_root/broker.pid"
count=0
while [ "$count" -lt 60 ] && [ ! -S "$broker_socket" ]; do process_active "$broker_pid" || exit 1; sleep 0.1; count=$((count + 1)); done
[ -S "$broker_socket" ]
phase=start-diagnostic-stall
printf '%s\n' "$run_id" > "$diagnostic_runtime_marker"
chmod 600 "$diagnostic_runtime_marker"
mkdir -m 750 "$diagnostic_runtime"
chown "0:$nobody_gid" "$diagnostic_runtime"
printf '%s' "$diagnostic_capture_b64" | base64 -d > "$diagnostic_capture"
chmod 500 "$diagnostic_capture"
"$python_binary" "$diagnostic_capture" "$diagnostic_socket" "$diagnostic_seen" \
  > "$stage_root/diagnostic.log" 2>&1 < /dev/null &
diagnostic_pid=$!
printf '%s\n' "$diagnostic_pid" > "$stage_root/diagnostic.pid"
count=0
while [ "$count" -lt 60 ] && [ ! -S "$diagnostic_socket" ]; do process_active "$diagnostic_pid" || exit 1; sleep 0.1; count=$((count + 1)); done
[ -S "$diagnostic_socket" ]
chown "0:$nobody_gid" "$diagnostic_socket"
chmod 660 "$diagnostic_socket"
phase=start-control-denial
mkdir -m 750 "$control_runtime"
chown "$www_uid:$nobody_gid" "$control_runtime"
setpriv --reuid="$www_uid" --regid="$nobody_gid" --clear-groups --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME=/nonexistent TMPDIR=/tmp BLI_KVM_CONTROL_SOCKET="$control_socket" \
  "$control" > "$stage_root/control.log" 2>&1 < /dev/null &
control_pid=$!
printf '%s\n' "$control_pid" > "$stage_root/control.pid"
count=0
while [ "$count" -lt 60 ] && [ ! -S "$control_socket" ]; do process_active "$control_pid" || exit 1; sleep 0.1; count=$((count + 1)); done
[ -S "$control_socket" ]
[ "$(stat -c '%a' "$control_socket")" = 660 ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$control_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$control_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$control_pid/status")" = 1 ]
case " $(awk '/^Groups:/ {$1=""; print}' "/proc/$control_pid/status") " in *" $kvmd_gid "*) exit 1 ;; esac
if tr '\0' '\n' < "/proc/$control_pid/environ" | grep -Eq '^(CREDENTIALS_DIRECTORY|KVMD_)='; then exit 1; fi
printf '%s' "$control_probe_b64" | base64 -d > "$stage_root/control-probe.py"
chown "$nobody_uid:$nobody_gid" "$stage_root/control-probe.py"
chmod 500 "$stage_root/control-probe.py"
setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --bounding-set=-all --no-new-privs \
  python3 "$stage_root/control-probe.py" "$control_socket" > "$stage_root/control.result"
[ "$(sed -n '1p' "$stage_root/control.result")" = hid=capability_disabled ]
[ "$(sed -n '2p' "$stage_root/control.result")" = atx=capability_disabled ]
[ "$(sed -n '3p' "$stage_root/control.result")" = media=capability_disabled ]
[ "$(sed -n '4p' "$stage_root/control.result")" = requests=32 ]
[ "$(sed -n '5p' "$stage_root/control.result")" = latency=bounded ]
[ "$(wc -l < "$stage_root/control.result" | tr -d ' ')" = 5 ]
count=0
while [ "$count" -lt 20 ] && [ ! -f "$diagnostic_seen" ]; do process_active "$diagnostic_pid" || exit 1; sleep 0.1; count=$((count + 1)); done
[ -f "$diagnostic_seen" ]
phase=stop-control-during-diagnostic-stall
process_active "$diagnostic_pid"
stop_child "$control_pid" "$control"
control_pid=
phase=start-video
mkdir -m 750 "$video_runtime"
chown "$www_uid:$nobody_gid" "$video_runtime"
setpriv --reuid="$www_uid" --regid="$nobody_gid" --groups="$kvmd_gid" --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME=/nonexistent TMPDIR=/tmp \
  BLI_KVM_VIDEO_PROXY_SOCKET="$video_socket" BLI_KVM_USTREAMER_SOCKET=/run/kvmd/ustreamer.sock \
  "$video" > "$stage_root/video.log" 2>&1 < /dev/null &
video_pid=$!
printf '%s\n' "$video_pid" > "$stage_root/video.pid"
count=0
while [ "$count" -lt 60 ] && [ ! -S "$video_socket" ]; do process_active "$video_pid" || exit 1; sleep 0.1; count=$((count + 1)); done
[ -S "$video_socket" ]
[ "$(stat -c '%a' "$video_socket")" = 660 ]
phase=start-server
state="$stage_root/state"
mkdir -m 700 "$state"
chown "$nobody_uid:$nobody_gid" "$state"
setpriv --reuid="$nobody_uid" --regid="$nobody_gid" --clear-groups --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME="$state" TMPDIR="$state" \
  BLI_KVM_BIND="127.0.0.1:$bind_port" BLI_KVM_WEB_ROOT="$payload/share/web" \
  BLI_KVM_STATE_DIR="$state" BLI_KVM_BROKER_SOCKET="$broker_socket" BLI_KVM_VIDEO_SOCKET="$video_socket" \
  BLI_KVM_SECURE_COOKIES=1 BLI_KVM_ALLOW_MISSING_ORIGIN=0 \
  "$server" > "$stage_root/server.log" 2>&1 < /dev/null &
server_pid=$!
printf '%s\n' "$server_pid" > "$stage_root/server.pid"
count=0
while [ "$count" -lt 60 ]; do
  if curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null "http://127.0.0.1:$bind_port/api/health" 2>/dev/null; then break; fi
  process_active "$server_pid" || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 60 ]
phase=prepare-tls
install -d -m 700 "$nginx_state"
install -d -m 700 "$nginx_state/client-body" "$nginx_state/proxy"
openssl req -x509 -newkey rsa:2048 -sha256 -nodes -days 1 -subj /CN=localhost \
  -keyout "$tls_key" -out "$tls_certificate" >/dev/null 2>&1
printf '%s' "$nginx_b64" | base64 -d > "$nginx_config"
chmod 400 "$tls_key" "$tls_certificate" "$nginx_config"
chown -R "$www_uid:$www_gid" "$nginx_state" "$tls_key" "$tls_certificate" "$nginx_config"
setpriv --reuid="$www_uid" --regid="$www_gid" --clear-groups --bounding-set=-all --no-new-privs \
  "$nginx_binary" -t -c "$nginx_config" -p "$nginx_state" >/dev/null 2>&1
phase=start-tls
setpriv --reuid="$www_uid" --regid="$www_gid" --clear-groups --bounding-set=-all --no-new-privs \
  "$nginx_binary" -c "$nginx_config" -p "$nginx_state" -g 'daemon off;' \
  > "$stage_root/nginx.log" 2>&1 < /dev/null &
nginx_pid=$!
printf '%s\n' "$nginx_pid" > "$stage_root/nginx.pid"
count=0
while [ "$count" -lt 80 ]; do
  if curl --noproxy '*' --insecure --silent --fail --max-time 1 --output /dev/null "https://127.0.0.1:$tls_port/api/health" 2>/dev/null; then break; fi
  process_active "$nginx_pid" || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 80 ]
phase=verify-tls
openssl s_client -connect "127.0.0.1:$tls_port" -servername localhost -tls1_2 </dev/null 2>/dev/null | grep -q 'BEGIN CERTIFICATE'
openssl s_client -connect "127.0.0.1:$tls_port" -servername localhost -tls1_3 </dev/null 2>/dev/null | grep -q 'BEGIN CERTIFICATE'
printf '%s\n' ready > "$ready_file"
chmod 600 "$ready_file"
emit ready yes
phase=wait-browser
count=0
while [ "$count" -lt 240 ] && [ ! -f "$browser_done" ]; do
  process_active "$broker_pid" || exit 1
  process_active "$video_pid" || exit 1
  process_active "$server_pid" || exit 1
  process_active "$nginx_pid" || exit 1
  sleep 0.5
  count=$((count + 1))
done
[ -f "$browser_done" ]
[ "$(cat "$browser_result" 2>/dev/null || true)" = passed ]
phase=stop-services
stop_child "$nginx_pid" "$nginx_binary"
nginx_pid=
stop_child "$server_pid" "$server"
server_pid=
stop_child "$video_pid" "$video"
video_pid=
stop_child "$diagnostic_pid" "$python_binary"
diagnostic_pid=
stop_child "$broker_pid" "$broker"
broker_pid=
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
owned_runtime
rm -rf -- "$runtime_root"
rm -f -- "$runtime_marker"
cleanup_diagnostics
owned_stage
rm -rf -- "$stage_root"
run_succeeded=yes
trap - EXIT HUP INT TERM
emit bundle_sha256 "$actual_sha256"
emit payload_checksums verified
emit credential_isolation verified
emit nginx_config verified
emit tls_loopback yes
emit tls12 passed
emit tls13 passed
emit nginx_stopped yes
emit server_stopped yes
emit video_proxy_stopped yes
emit control_broker_stopped yes
emit control_denials verified
emit control_no_backend verified
emit control_queue_pressure 32-denials
emit control_response_latency independent
emit diagnostic_event control-capability-disabled
emit diagnostic_runtime_removed yes
emit diagnostic_stall seven-second-no-response
emit broker_stopped yes
emit stage_removed yes
emit rootfs_mode_after_run "$rootfs_mode"
""".strip()
    return f"{values}\n{PROCESS_FUNCTIONS}\n{body}"


def build_signal_script(paths: TlsPaths, result: str) -> str:
    if result not in {"passed", "failed"}:
        raise ValueError("invalid browser result")
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "run_id": paths.stage.run_id,
            "browser_done": paths.browser_done,
            "browser_result": paths.browser_result,
            "result": result,
        }
    )
    body = r"""
set -eu
[ -f "$stage_marker" ]
[ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
[ -f "$stage_root/browser.ready" ]
printf '%s\n' "$result" > "$browser_result"
: > "$browser_done"
chmod 600 "$browser_result" "$browser_done"
""".strip()
    return f"{values}\n{body}"


def build_cleanup_script(paths: TlsPaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "diagnostic_runtime": paths.diagnostic_runtime,
            "diagnostic_runtime_marker": paths.diagnostic_runtime_marker,
            "diagnostic_socket": paths.diagnostic_socket,
            "run_id": paths.stage.run_id,
            "server": paths.stage.server,
            "broker": paths.broker_binary,
            "control": paths.control_binary,
            "video": paths.video_binary,
            "nginx_binary": paths.nginx_binary,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
python_binary=$(readlink -f "$(command -v python3)")
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ]
  [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
  for item in "nginx.pid:$nginx_binary" "server.pid:$server" "video.pid:$video" "control.pid:$control" "diagnostic.pid:$python_binary" "broker.pid:$broker"; do
    pid_file="$stage_root/${item%%:*}"
    expected=${item#*:}
    [ -f "$pid_file" ] || continue
    candidate=$(cat "$pid_file")
    if process_active "$candidate"; then
      [ "$(readlink "/proc/$candidate/exe" 2>/dev/null || true)" = "$expected" ]
      kill -TERM "$candidate"
      count=0
      while process_active "$candidate" && [ "$count" -lt 50 ]; do sleep 0.1; count=$((count + 1)); done
      if process_active "$candidate"; then
        kill -KILL "$candidate"
        count=0
        while process_active "$candidate" && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
      fi
      process_active "$candidate" && exit 1
    fi
  done
fi
if [ -e "$diagnostic_runtime" ] || [ -e "$diagnostic_runtime_marker" ]; then
  [ -e "$stage_root" ]
  [ -f "$diagnostic_runtime_marker" ]
  [ "$(cat "$diagnostic_runtime_marker" 2>/dev/null || true)" = "$run_id" ]
  if [ -e "$diagnostic_runtime" ]; then
    [ -d "$diagnostic_runtime" ]
    [ ! -L "$diagnostic_runtime" ]
    unexpected=$(find "$diagnostic_runtime" -mindepth 1 -maxdepth 1 ! -name collector.sock -print -quit)
    [ -z "$unexpected" ]
    if [ -e "$diagnostic_socket" ]; then [ -S "$diagnostic_socket" ]; rm -f -- "$diagnostic_socket"; fi
    rmdir "$diagnostic_runtime"
  fi
  rm -f -- "$diagnostic_runtime_marker"
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
[ ! -e "$diagnostic_runtime" ]
[ ! -e "$diagnostic_runtime_marker" ]
[ ! -e "$diagnostic_socket" ]
emit cleanup_removed yes
""".strip()
    return f"{values}\n{PROCESS_FUNCTIONS}\n{body}"


def build_final_script(paths: TlsPaths, bind_port: int) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "runtime_root": paths.runtime_root,
            "runtime_marker": paths.runtime_marker,
            "diagnostic_runtime": paths.diagnostic_runtime,
            "diagnostic_runtime_marker": paths.diagnostic_runtime_marker,
            "diagnostic_socket": paths.diagnostic_socket,
            "server_port_hex": f"{bind_port:04X}",
            "tls_port_hex": f"{TLS_PORT:04X}",
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$runtime_root" ]
[ ! -e "$runtime_marker" ]
[ ! -e "$diagnostic_runtime" ]
[ ! -e "$diagnostic_runtime_marker" ]
[ ! -e "$diagnostic_socket" ]
port_hex=$server_port_hex
if port_in_use; then exit 1; fi
port_hex=$tls_port_hex
if port_in_use; then exit 1; fi
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
emit stage_absent_final yes
emit diagnostic_absent_final yes
emit ports_free_final yes
emit rootfs_mode_final "$rootfs_mode"
emit kvmd_active_final active
emit kvmd_nginx_active_final active
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


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


CommandFactory = Callable[[int], list[str]]


def start_with_secret_fd(
    secret: bytearray,
    command_factory: CommandFactory,
    *,
    stdout: int,
) -> subprocess.Popen[bytes]:
    read_fd, write_fd = os.pipe()
    try:
        try:
            _write_all(write_fd, secret)
            _write_all(write_fd, b"\n")
        finally:
            os.close(write_fd)
        return subprocess.Popen(
            command_factory(read_fd),
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            pass_fds=(read_fd,),
        )
    finally:
        os.close(read_fd)


def tunnel_command(
    target: prototype.RemoteTarget, password_fd: int, local_port: int
) -> list[str]:
    transport = prototype.ssh_transport_command(
        target, password_fd, clear_forwardings=False
    )
    return [
        *transport[:-1],
        "-o",
        "ExitOnForwardFailure=yes",
        "-N",
        "-L",
        f"127.0.0.1:{local_port}:127.0.0.1:{TLS_PORT}",
        transport[-1],
    ]


def free_local_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return cast(int, listener.getsockname()[1])


def wait_for_line(process: subprocess.Popen[bytes], expected: bytes, timeout: float) -> bytes:
    if process.stdout is None:
        raise ValueError("remote process stdout is unavailable")
    stream = cast(IO[bytes], process.stdout)
    deadline = time.monotonic() + timeout
    captured = bytearray()
    while time.monotonic() < deadline:
        ready, _, _ = select.select([stream], [], [], 0.5)
        if ready:
            line = stream.readline()
            if not line:
                break
            captured.extend(line)
            if line == expected:
                return bytes(captured)
        if process.poll() is not None:
            break
    raise TlsGateError(
        "remote-readiness",
        "remote-not-ready",
        return_code=process.poll(),
        stdout_bytes=len(captured),
    )


def wait_for_tunnel(process: subprocess.Popen[bytes], port: int, timeout: float) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise TlsGateError("ssh-tunnel", "tunnel-exited", return_code=process.returncode)
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.5):
                return
        except OSError:
            time.sleep(0.1)
    raise TlsGateError("ssh-tunnel", "tunnel-timeout")


def stop_process(process: subprocess.Popen[bytes]) -> None:
    if process.poll() is not None:
        return
    process.terminate()
    try:
        _ = process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        _ = process.wait(timeout=5)


def parse_failure(raw: bytes) -> tuple[str | None, bool | None]:
    lines = [line for line in raw.splitlines() if line != b"ready=yes"]
    try:
        facts = prototype.parse_facts(b"\n".join(lines) + b"\n", FAILURE_KEYS)
    except (UnicodeError, ValueError):
        return None, None
    cleanup = facts["failure_cleanup"]
    if cleanup not in {"removed", "retained"}:
        return None, None
    return facts["failure_phase"], cleanup == "removed"


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "architecture": "aarch64",
        "diagnostic_absent_before": "yes",
        "rootfs_mode_before": "rw",
        "tmp_mount_mode": "exec",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "nginx_ready": "yes",
        "browser_runtime_ready": "yes",
        "tools_ready": "yes",
        "stage_absent": "yes",
        "ports_free_before": "yes",
        "ustreamer_socket": "present",
        "ustreamer_socket_mode": "660",
        "ustreamer_socket_group": "kvmd",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("TLS preflight invariant failed")


def validate_remote(facts: dict[str, str], bundle_sha256: str) -> None:
    expected = {
        "ready": "yes",
        "bundle_sha256": bundle_sha256,
        "payload_checksums": "verified",
        "credential_isolation": "verified",
        "control_denials": "verified",
        "control_no_backend": "verified",
        "control_queue_pressure": "32-denials",
        "control_response_latency": "independent",
        "diagnostic_event": "control-capability-disabled",
        "diagnostic_runtime_removed": "yes",
        "diagnostic_stall": "seven-second-no-response",
        "nginx_config": "verified",
        "tls_loopback": "yes",
        "tls12": "passed",
        "tls13": "passed",
        "nginx_stopped": "yes",
        "server_stopped": "yes",
        "video_proxy_stopped": "yes",
        "control_broker_stopped": "yes",
        "broker_stopped": "yes",
        "stage_removed": "yes",
        "rootfs_mode_after_run": "rw",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("TLS remote invariant failed")


def validate_final(facts: dict[str, str]) -> None:
    expected = {
        "diagnostic_absent_final": "yes",
        "stage_absent_final": "yes",
        "ports_free_final": "yes",
        "rootfs_mode_final": "rw",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("TLS final-state invariant failed")


def validate_browser(raw: bytes) -> dict[str, object]:
    if len(raw) > 16 * 1024:
        raise ValueError("browser output exceeded the limit")
    decoded = cast(object, json.loads(raw))
    if not isinstance(decoded, dict):
        raise ValueError("browser output root is invalid")
    summary = cast(dict[str, object], decoded)
    expected: dict[str, object] = {
        "schemaVersion": 1,
        "result": "passed",
        "protocol": "https",
        "title": "Remote Desktop - Hardened BliKVM",
        "sessionCookie": "secure-httponly-strict",
        "csrfCookie": "secure-readable-strict",
        "inventoryStatus": 200,
        "inventorySchema": 3,
        "inventoryBackend": "kvmd_transition",
        "thermalObservation": "bounded_millidegrees",
        "thermalCapability": "read_only",
        "thermalEvidence": "runtime_verified",
        "thermalUi": "rendered",
        "rtcObservation": "bounded_epoch_and_offset",
        "rtcCapability": "read_only",
        "rtcEvidence": "runtime_verified",
        "rtcUi": "rendered",
        "diagnosticsApi": "authenticated-current-only",
        "diagnosticsUi": "rendered-action-free",
        "diagnosticsMobileUi": "zh-tw-responsive-action-free",
        "diagnosticsHistory": "persistence-not-provisioned",
        "videoCapability": "read_only",
        "hidCapability": "unavailable",
        "powerCapability": "unavailable",
        "powerLedCapability": "read_only",
        "mediaCapability": "read_only",
        "videoTransport": "mjpeg",
        "videoDecoded": "passed",
        "streamRevocation": "passed",
        "controlRequestsObserved": 0,
        "webSocketsObserved": 0,
        "passwordRotation": "passed",
        "logout": "passed",
    }
    if any(summary.get(key) != value for key, value in expected.items()):
        raise ValueError("browser invariant failed")
    observed = summary.get("apiResponsesObserved")
    if not isinstance(observed, int) or observed < 5 or observed > 200:
        raise ValueError("browser response count is invalid")
    reconnects = summary.get("videoReconnects")
    if not isinstance(reconnects, int) or reconnects < 2 or reconnects > 10:
        raise ValueError("browser video reconnect count is invalid")
    return dict(sorted(summary.items()))


def validate_browser_failure(raw: bytes) -> tuple[str, str]:
    if len(raw) > 4096:
        raise ValueError("browser failure output exceeded the limit")
    decoded = cast(object, json.loads(raw))
    if not isinstance(decoded, dict):
        raise ValueError("browser failure output root is invalid")
    summary = cast(dict[str, object], decoded)
    phase = summary.get("phase")
    error_type = summary.get("errorType")
    if (
        summary.get("schemaVersion") != 1
        or summary.get("result") != "failed"
        or not isinstance(phase, str)
        or not re.fullmatch(r"[a-z-]{1,64}", phase)
        or error_type not in {"AssertionError", "Error", "TimeoutError", "TypeError"}
    ):
        raise ValueError("browser failure envelope is invalid")
    return phase, cast(str, error_type)


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
    if bind_port == TLS_PORT:
        raise ValueError("server and TLS ports must differ")
    browser = browser_runtime()
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths = tls_paths(bundle.sha256, secrets.token_hex(8))
    required_bytes = bundle.size_bytes * 4 + prototype.MIN_TMP_HEADROOM_BYTES
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    remote_process: subprocess.Popen[bytes] | None = None
    tunnel: subprocess.Popen[bytes] | None = None
    stage = "initialization"
    browser_summary: dict[str, object] | None = None
    remote_ready = False
    try:
        stage = "preflight"
        preflight_result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(paths, required_bytes, bind_port),
            timeout=25,
        )
        if preflight_result.returncode != 0:
            raise TlsGateError(stage, "remote-command", return_code=preflight_result.returncode)
        preflight = prototype.parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        with bundle.path.open("rb") as bundle_stream:
            upload = prototype.run_ssh(
                target,
                secret,
                prototype.build_upload_script(paths.stage),
                stdin=bundle_stream,
                timeout=60,
            )
        if upload.returncode != 0:
            raise TlsGateError(stage, "remote-command", return_code=upload.returncode)

        stage = "credential-upload"
        credential = run_ssh_with_secret_stdin(
            target,
            secret,
            build_credential_upload_script(paths),
            timeout=25,
        )
        if credential.returncode != 0:
            raise TlsGateError(stage, "remote-command", return_code=credential.returncode)

        stage = "remote-start"
        remote_script = build_remote_script(paths, bundle, bind_port)
        remote_process = start_with_secret_fd(
            secret,
            lambda fd: prototype.ssh_command(target, fd, remote_script),
            stdout=subprocess.PIPE,
        )
        prefix = wait_for_line(remote_process, b"ready=yes\n", 45)
        remote_ready = True

        stage = "ssh-tunnel"
        local_port = free_local_port()
        tunnel = start_with_secret_fd(
            secret,
            lambda fd: tunnel_command(target, fd, local_port),
            stdout=subprocess.DEVNULL,
        )
        wait_for_tunnel(tunnel, local_port, 15)

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
            timeout=120,
        )
        if browser_result.returncode != 0:
            browser_phase, browser_error = validate_browser_failure(
                browser_result.stdout
            )
            raise TlsGateError(
                stage,
                f"browser-{browser_phase}-{browser_error.casefold()}",
                return_code=browser_result.returncode,
            )
        browser_summary = validate_browser(browser_result.stdout)

        stage = "remote-finish"
        signal = prototype.run_ssh(
            target, secret, build_signal_script(paths, "passed"), timeout=15
        )
        if signal.returncode != 0:
            raise TlsGateError(stage, "signal-failed", return_code=signal.returncode)
        output_bytes, _ = remote_process.communicate(timeout=60)
        remote_output = prefix + output_bytes
        if remote_process.returncode != 0:
            remote_phase, remote_cleanup = parse_failure(remote_output)
            raise TlsGateError(
                stage,
                "remote-command",
                return_code=remote_process.returncode,
                stdout_bytes=len(remote_output),
                remote_phase=remote_phase,
                remote_cleanup_confirmed=remote_cleanup,
            )
        checks = prototype.parse_facts(remote_output, RUN_KEYS)
        validate_remote(checks, bundle.sha256)

        stage = "final-state"
        final_result = prototype.run_ssh(
            target, secret, build_final_script(paths, bind_port), timeout=25
        )
        if final_result.returncode != 0:
            raise TlsGateError(stage, "remote-command", return_code=final_result.returncode)
        final_state = prototype.parse_facts(final_result.stdout, FINAL_KEYS)
        validate_final(final_state)
        cleanup_required = False

        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-tls-browser-gate",
            "result": "passed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": {
                "tlsListener": "device-loopback-only",
                "browserTransport": "strict-host-key-ssh-forward",
                "certificate": "ephemeral-self-signed",
                "kvmdAccess": "read-only-inventory",
                "controlBroker": "denial-only-no-backend",
                "diagnostics": "exact-event-stall-and-queue-pressure",
                "physicalActionsInvoked": False,
                "persistentPathsChanged": False,
            },
            "preflight": preflight,
            "checks": checks,
            "browser": browser_summary,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (TlsGateError, OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        if remote_ready and remote_process is not None and remote_process.poll() is None:
            try:
                _ = prototype.run_ssh(
                    target, secret, build_signal_script(paths, "failed"), timeout=15
                )
                _ = remote_process.communicate(timeout=45)
            except (OSError, subprocess.SubprocessError):
                pass
        cleanup: CleanupReceipt = {"attempted": False, "confirmed": not cleanup_required}
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target, secret, build_cleanup_script(paths), timeout=30
                )
                facts = (
                    prototype.parse_facts(result.stdout, CLEANUP_KEYS)
                    if result.returncode == 0
                    else {}
                )
                cleanup = {
                    "attempted": True,
                    "confirmed": facts == {"cleanup_removed": "yes"},
                }
            except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
                cleanup = {"attempted": True, "confirmed": False}
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-tls-browser-gate",
            "result": "failed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "failure": {
                "stage": getattr(error, "stage", stage),
                "failureType": getattr(error, "failure_type", type(error).__name__),
                "returnCode": getattr(error, "return_code", None),
                "stdoutBytes": getattr(error, "stdout_bytes", 0),
                "remotePhase": getattr(error, "remote_phase", None),
                "remoteCleanupConfirmed": getattr(
                    error, "remote_cleanup_confirmed", None
                ),
            },
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        if tunnel is not None:
            stop_process(tunnel)
        if remote_process is not None and remote_process.poll() is None:
            stop_process(remote_process)
        _zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
