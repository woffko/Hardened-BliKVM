#!/usr/bin/env python3
"""Temporarily stage and smoke-test the AArch64 prototype on a test BliKVM.

The SSH password is read once from stdin into a mutable buffer. Every SSH
process receives it through a dedicated inherited file descriptor used by
``sshpass``; the bundle itself is streamed over the SSH process stdin. Raw SSH
output, stderr, endpoint data, hostnames, and credentials are never written to
the sanitized receipt.

The remote test is deliberately temporary. It never invokes the device's
``rw`` or ``ro`` helpers, never installs a unit, never starts the platform
broker, and removes only a freshly reserved, marker-protected directory below
``/tmp``.
"""

from __future__ import annotations

import argparse
from collections.abc import Callable
from dataclasses import dataclass
import hashlib
import ipaddress
import json
import os
from pathlib import Path, PurePosixPath
import re
import secrets
import shlex
import stat
import subprocess
import sys
import tarfile
from typing import IO, TypedDict, cast
from urllib import parse

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools.capture_pikvm_baseline import write_json_0600_atomic


EXPECTED_BUNDLE_ROOT = "hardened-blikvm-prototype-0.1.0-dev-aarch64"
EXPECTED_PRODUCT = "Hardened BliKVM"
EXPECTED_TARGET = "aarch64-unknown-linux-musl"
MAX_BUNDLE_BYTES = 64 * 1024 * 1024
MAX_BUNDLE_FILES = 2_048
MAX_UNCOMPRESSED_BYTES = 128 * 1024 * 1024
MAX_MANIFEST_BYTES = 64 * 1024
MAX_REMOTE_OUTPUT_BYTES = 16 * 1024
MAX_SECRET_BYTES = 4 * 1024
MIN_TMP_HEADROOM_BYTES = 8 * 1024 * 1024
HEALTH_REQUESTS = 20
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:,+/()=-]{1,240}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
USERNAME = re.compile(r"^[a-z_][a-z0-9_-]{0,31}$")
HOST_LABEL = re.compile(r"^[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?$")

PREFLIGHT_KEYS = {
    "architecture",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "nobody_gid_nonzero",
    "nobody_uid_nonzero",
    "port_free_before",
    "rootfs_mode_before",
    "setpriv_ready",
    "stage_absent",
    "tmp_available_bytes",
    "tmp_mount_mode",
    "tools_ready",
}
RUN_KEYS = {
    "broker_mode",
    "bundle_sha256",
    "capabilities_status",
    "graceful_stop",
    "health_requests",
    "health_status",
    "inventory_unauth_status",
    "kvmd_active_after_run",
    "kvmd_nginx_active_after_run",
    "loopback_listener",
    "nonloopback_refused",
    "no_new_privs",
    "payload_checksums",
    "rootfs_mode_after_run",
    "security_headers",
    "server_caps",
    "server_gid",
    "server_groups",
    "server_mode",
    "server_uid",
    "setup_status",
    "staging_removed",
    "static_status",
}
FINAL_KEYS = {
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "port_free_final",
    "rootfs_mode_final",
    "stage_absent_final",
}
CLEANUP_KEYS = {"cleanup_removed"}
RUNTIME_FAILURE_KEYS = {"failure_cleanup", "failure_phase"}


class BundleReceipt(TypedDict):
    sha256: str
    sizeBytes: int
    sourceCommit: str


class PolicyReceipt(TypedDict):
    brokerStarted: bool
    rootfsHelpersInvoked: bool
    stagingFilesystem: str
    persistentPathsChanged: bool


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


class FailureReceipt(TypedDict):
    stage: str
    failureType: str
    returnCode: int | None
    stdoutBytes: int
    remotePhase: str | None
    remoteCleanupConfirmed: bool | None


class PassedReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    bundle: BundleReceipt
    policy: PolicyReceipt
    preflight: dict[str, str]
    checks: dict[str, str]
    finalState: dict[str, str]
    cleanup: CleanupReceipt


class FailedReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    bundle: BundleReceipt
    policy: PolicyReceipt
    failure: FailureReceipt
    cleanup: CleanupReceipt


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
class BundleInfo:
    path: Path
    sha256: str
    size_bytes: int
    source_commit: str


@dataclass(frozen=True)
class RemoteTarget:
    host: str
    port: int
    username: str
    known_hosts: Path


@dataclass(frozen=True)
class StagePaths:
    run_id: str
    root: str
    marker: str
    archive: str
    payload: str
    server: str


class StageError(Exception):
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


def validate_hostname(host: str) -> str:
    try:
        _ = ipaddress.ip_address(host)
        return host
    except ValueError:
        pass
    if len(host) > 253 or any(not HOST_LABEL.fullmatch(label) for label in host.split(".")):
        raise ValueError("invalid SSH hostname")
    return host


def parse_endpoint(endpoint: str) -> tuple[str, int]:
    parsed = parse.urlparse(endpoint)
    if (
        parsed.scheme != "ssh"
        or not parsed.hostname
        or parsed.username
        or parsed.password
        or parsed.path not in ("", "/")
        or parsed.query
        or parsed.fragment
    ):
        raise ValueError("endpoint must be a credential-free SSH URL")
    try:
        port = parsed.port or 22
    except ValueError as error:
        raise ValueError("invalid SSH port") from error
    if not 1 <= port <= 65_535:
        raise ValueError("invalid SSH port")
    return validate_hostname(parsed.hostname), port


def validate_username(username: str) -> str:
    if not USERNAME.fullmatch(username):
        raise ValueError("invalid SSH username")
    return username


def validate_sha256(value: str) -> str:
    if not SHA256.fullmatch(value):
        raise ValueError("expected SHA-256 must be 64 lowercase hexadecimal characters")
    return value


def validate_bind_port(port: int) -> int:
    if not 1_024 <= port <= 65_535:
        raise ValueError("bind port must be an unprivileged TCP port")
    return port


def validate_private_file(path_value: str) -> Path:
    path = Path(path_value).resolve(strict=True)
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or Path(path_value).is_symlink():
        raise ValueError("known-hosts must be a regular non-symlink file")
    if metadata.st_mode & 0o077:
        raise ValueError("known-hosts must not be accessible by group or others")
    if metadata.st_size == 0:
        raise ValueError("known-hosts must not be empty")
    return path


def validate_output_path(path_value: str) -> Path:
    path = Path(path_value).resolve()
    private_root = (Path.cwd() / "evidence" / "private").resolve()
    if not path.is_relative_to(private_root) or path == private_root:
        raise ValueError("receipt must be below evidence/private")
    return path


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def _read_bundle_manifest(archive: tarfile.TarFile) -> dict[str, object]:
    member = archive.getmember(f"{EXPECTED_BUNDLE_ROOT}/MANIFEST.json")
    if member.size > MAX_MANIFEST_BYTES:
        raise ValueError("bundle manifest exceeded the size limit")
    extracted = archive.extractfile(member)
    if extracted is None:
        raise ValueError("bundle manifest is not a regular file")
    raw = extracted.read(MAX_MANIFEST_BYTES + 1)
    if len(raw) > MAX_MANIFEST_BYTES:
        raise ValueError("bundle manifest exceeded the size limit")
    decoded = cast(object, json.loads(raw))
    if not isinstance(decoded, dict):
        raise ValueError("bundle manifest root must be an object")
    return cast(dict[str, object], decoded)


def validate_bundle(path_value: str, expected_sha256: str) -> BundleInfo:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("bundle must be a regular non-symlink file")
    path = source.resolve(strict=True)
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_BUNDLE_BYTES:
        raise ValueError("bundle size or type is invalid")
    actual_sha256 = _hash_file(path)
    if actual_sha256 != expected_sha256:
        raise ValueError("bundle SHA-256 mismatch")

    with tarfile.open(path, mode="r:gz") as archive:
        members = archive.getmembers()
        if not members or len(members) > MAX_BUNDLE_FILES:
            raise ValueError("bundle member count is invalid")
        total_size = 0
        names: set[str] = set()
        for member in members:
            member_path = PurePosixPath(member.name)
            if (
                member_path.is_absolute()
                or ".." in member_path.parts
                or not member_path.parts
                or member_path.parts[0] != EXPECTED_BUNDLE_ROOT
                or member.name in names
                or not (member.isfile() or member.isdir())
            ):
                raise ValueError("bundle contains an unsafe member")
            names.add(member.name)
            total_size += member.size
            if total_size > MAX_UNCOMPRESSED_BYTES:
                raise ValueError("bundle uncompressed size exceeded the limit")
        required = {
            f"{EXPECTED_BUNDLE_ROOT}/MANIFEST.json",
            f"{EXPECTED_BUNDLE_ROOT}/SHA256SUMS",
            f"{EXPECTED_BUNDLE_ROOT}/bin/bli-kvm-server",
            f"{EXPECTED_BUNDLE_ROOT}/libexec/bli-kvm-platform-broker",
            f"{EXPECTED_BUNDLE_ROOT}/share/web/index.html",
        }
        if not required.issubset(names):
            raise ValueError("bundle is missing a required prototype member")
        manifest = _read_bundle_manifest(archive)

    if (
        manifest.get("schemaVersion") != 1
        or manifest.get("product") != EXPECTED_PRODUCT
        or manifest.get("target") != EXPECTED_TARGET
        or manifest.get("releaseAccepted") is not False
        or manifest.get("deviceAccepted") is not False
    ):
        raise ValueError("bundle manifest does not describe the expected prototype")
    source_commit = manifest.get("sourceCommit")
    if not isinstance(source_commit, str) or not re.fullmatch(r"[0-9a-f]{40}", source_commit):
        raise ValueError("bundle source commit is invalid")
    return BundleInfo(path, actual_sha256, metadata.st_size, source_commit)


def read_secret_stdin() -> bytearray:
    buffer = bytearray(MAX_SECRET_BYTES + 2)
    view = memoryview(buffer)
    offset = 0
    try:
        while offset < len(buffer):
            count = os.readv(sys.stdin.fileno(), [view[offset:]])
            if count == 0:
                break
            offset += count
    finally:
        view.release()
    if offset > MAX_SECRET_BYTES + 1:
        _zero(buffer)
        raise ValueError("SSH password exceeded the size limit")
    for index in range(offset, len(buffer)):
        buffer[index] = 0
    del buffer[offset:]
    if buffer.endswith(b"\n"):
        buffer[-1] = 0
        del buffer[-1]
    if buffer.endswith(b"\r"):
        buffer[-1] = 0
        del buffer[-1]
    if (
        not buffer
        or len(buffer) > MAX_SECRET_BYTES
        or b"\n" in buffer
        or b"\r" in buffer
        or b"\x00" in buffer
    ):
        _zero(buffer)
        raise ValueError("SSH password stdin is empty or malformed")
    return buffer


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def _write_all(fd: int, value: bytes | bytearray) -> None:
    view = memoryview(value)
    offset = 0
    try:
        while offset < len(view):
            offset += os.write(fd, view[offset:])
    finally:
        view.release()


CommandFactory = Callable[[int], list[str]]


def run_with_secret_fd(
    secret: bytearray,
    command_factory: CommandFactory,
    *,
    stdin: int | IO[bytes],
    timeout: float,
) -> subprocess.CompletedProcess[bytes]:
    read_fd, write_fd = os.pipe()
    try:
        try:
            _write_all(write_fd, secret)
            _write_all(write_fd, b"\n")
        finally:
            os.close(write_fd)
        command = command_factory(read_fd)
        return subprocess.run(
            command,
            stdin=stdin,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=timeout,
            close_fds=True,
            pass_fds=(read_fd,),
        )
    finally:
        os.close(read_fd)


def ssh_transport_command(
    target: RemoteTarget, password_fd: int, *, clear_forwardings: bool = True
) -> list[str]:
    return [
        "sshpass",
        "-d",
        str(password_fd),
        "ssh",
        "-F",
        "/dev/null",
        "-T",
        "-p",
        str(target.port),
        "-o",
        "BatchMode=no",
        *(["-o", "ClearAllForwardings=yes"] if clear_forwardings else []),
        "-o",
        "ConnectTimeout=8",
        "-o",
        "ConnectionAttempts=1",
        "-o",
        "GlobalKnownHostsFile=/dev/null",
        "-o",
        "IdentitiesOnly=yes",
        "-o",
        "KbdInteractiveAuthentication=no",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "PasswordAuthentication=yes",
        "-o",
        "PermitLocalCommand=no",
        "-o",
        "PreferredAuthentications=password",
        "-o",
        "ProxyCommand=none",
        "-o",
        "ProxyJump=none",
        "-o",
        "PubkeyAuthentication=no",
        "-o",
        "RequestTTY=no",
        "-o",
        "ServerAliveCountMax=2",
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        f"UserKnownHostsFile={target.known_hosts}",
        f"{target.username}@{target.host}",
    ]


def ssh_command(target: RemoteTarget, password_fd: int, remote_script: str) -> list[str]:
    return [
        *ssh_transport_command(target, password_fd),
        f"sh -c {shlex.quote(remote_script)}",
    ]


def run_ssh(
    target: RemoteTarget,
    secret: bytearray,
    remote_script: str,
    *,
    stdin: int | IO[bytes] = subprocess.DEVNULL,
    timeout: float = 30,
) -> subprocess.CompletedProcess[bytes]:
    return run_with_secret_fd(
        secret,
        lambda password_fd: ssh_command(target, password_fd, remote_script),
        stdin=stdin,
        timeout=timeout,
    )


def stage_paths(bundle_sha256: str, run_id: str) -> StagePaths:
    _ = validate_sha256(bundle_sha256)
    if not RUN_ID.fullmatch(run_id):
        raise ValueError("invalid staging run ID")
    root = f"/tmp/hardened-blikvm-prototype-{bundle_sha256[:12]}-{run_id}"
    return StagePaths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        archive=f"{root}/bundle.tar.gz",
        payload=f"{root}/payload",
        server=f"{root}/payload/bin/bli-kvm-server",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


PORT_FUNCTION = r"""
port_in_use() {
  for table in /proc/net/tcp /proc/net/tcp6; do
    [ -r "$table" ] || continue
    if awk -v suffix=":$port_hex" '
      NR > 1 && toupper($2) ~ (suffix "$") && $4 == "0A" { found = 1 }
      END { exit(found ? 0 : 1) }
    ' "$table"; then
      return 0
    fi
  done
  return 1
}
""".strip()


def build_preflight_script(paths: StagePaths, required_bytes: int, bind_port: int) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "required_bytes": required_bytes,
            "port_hex": f"{bind_port:04X}",
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
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
for tool in awk cat chmod chown curl df env findmnt getent grep head mkdir mv readlink rm sed setpriv sha256sum sleep stat systemctl tail tar tr; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
account=$(getent passwd nobody | head -n 1)
uid=$(printf '%s\n' "$account" | awk -F: '{print $3}')
gid=$(printf '%s\n' "$account" | awk -F: '{print $4}')
case "$uid:$gid" in *[!0-9:]*) exit 1 ;; esac
[ "$uid" -gt 0 ]
[ "$gid" -gt 0 ]
setpriv_ready=no
if setpriv --reuid="$uid" --regid="$gid" --clear-groups --bounding-set=-all --no-new-privs /bin/true >/dev/null 2>&1; then
  setpriv_ready=yes
fi
[ ! -e "$stage_root" ]
if port_in_use; then exit 1; fi
emit architecture aarch64
emit rootfs_mode_before "$rootfs_mode"
emit tmp_mount_mode "$tmp_mount_mode"
emit tmp_available_bytes "$tmp_available"
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit nobody_uid_nonzero yes
emit nobody_gid_nonzero yes
emit setpriv_ready "$setpriv_ready"
emit tools_ready yes
emit stage_absent yes
emit port_free_before yes
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{body}"


def build_upload_script(paths: StagePaths) -> str:
    values = _assignments(
        {"stage_root": paths.root, "marker": paths.marker, "run_id": paths.run_id}
    )
    body = r"""
set -eu
umask 077
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$run_id" > "$marker"
cat > "$stage_root/bundle.tar.gz.part"
[ -s "$stage_root/bundle.tar.gz.part" ]
mv "$stage_root/bundle.tar.gz.part" "$stage_root/bundle.tar.gz"
""".strip()
    return f"{values}\n{body}"


def build_run_script(paths: StagePaths, bundle: BundleInfo, bind_port: int) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "run_id": paths.run_id,
            "archive": paths.archive,
            "payload": paths.payload,
            "server": paths.server,
            "expected_sha256": bundle.sha256,
            "bind_port": bind_port,
            "port_hex": f"{bind_port:04X}",
            "health_requests": HEALTH_REQUESTS,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
pid=
phase=validate-stage
run_succeeded=no
emit() { printf '%s=%s\n' "$1" "$2"; }
owned_stage() {
  [ -f "$marker" ] && [ "$(cat "$marker" 2>/dev/null || true)" = "$run_id" ]
}
process_active() {
  [ -n "$pid" ] || return 1
  case "$pid" in *[!0-9]*|'') return 1 ;; esac
  kill -0 "$pid" 2>/dev/null || return 1
  state=$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)
  [ -n "$state" ] && [ "$state" != Z ]
}
classify_server_exit() {
  server_status=$1
  error_code=$(sed -n -e 's/.*code: \([0-9][0-9]*\).*/\1/p' -e 's/.*os error \([0-9][0-9]*\).*/\1/p' "$stage_root/server.log" | head -n 1)
  if grep -q '^setpriv:' "$stage_root/server.log"; then
    phase="wait-health-exited-setpriv-status-$server_status"
  elif grep -q '^env:' "$stage_root/server.log"; then
    phase="wait-health-exited-env-status-$server_status"
  elif [ -n "$error_code" ]; then
    phase="wait-health-exited-os-error-$error_code-status-$server_status"
  elif grep -q 'web root does not contain index.html' "$stage_root/server.log"; then
    phase="wait-health-exited-web-root-status-$server_status"
  elif grep -q 'Address already in use' "$stage_root/server.log"; then
    phase="wait-health-exited-address-in-use-status-$server_status"
  elif grep -q 'Permission denied' "$stage_root/server.log"; then
    phase="wait-health-exited-permission-status-$server_status"
  elif grep -q 'BLi-KVM development server listening' "$stage_root/server.log"; then
    phase="wait-health-exited-after-listen-status-$server_status"
  elif grep -q 'panicked at' "$stage_root/server.log"; then
    phase="wait-health-exited-panic-status-$server_status"
  elif grep -q '^Error:' "$stage_root/server.log"; then
    phase="wait-health-exited-rust-error-status-$server_status"
  else
    phase="wait-health-exited-other-status-$server_status"
  fi
}
stop_server() {
  [ -n "$pid" ] || return 0
  case "$pid" in *[!0-9]*|'') return 1 ;; esac
  if process_active; then
    [ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" = "$server" ] || return 1
    kill -TERM "$pid"
    count=0
    while process_active && [ "$count" -lt 50 ]; do
      sleep 0.1
      count=$((count + 1))
    done
    process_active && return 1
  fi
  wait "$pid" 2>/dev/null || true
  pid=
}
emergency_cleanup() {
  set +e
  safe_to_remove=yes
  if process_active; then
    if [ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" != "$server" ]; then
      safe_to_remove=no
    else
      kill -TERM "$pid" 2>/dev/null
      count=0
      while process_active && [ "$count" -lt 30 ]; do sleep 0.1; count=$((count + 1)); done
      if process_active; then
        kill -KILL "$pid" 2>/dev/null
        count=0
        while process_active && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
      fi
      process_active && safe_to_remove=no
    fi
  fi
  if [ -n "$pid" ] && ! process_active; then wait "$pid" 2>/dev/null || true; fi
  if [ "$safe_to_remove" = yes ] && owned_stage; then rm -rf -- "$stage_root"; fi
}
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    emergency_cleanup
    if [ ! -e "$stage_root" ]; then cleanup_state=removed; else cleanup_state=retained; fi
    emit failure_phase "$phase"
    emit failure_cleanup "$cleanup_state"
  fi
  exit "$exit_code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
owned_stage
[ -f "$archive" ]
phase=verify-archive
actual_sha256=$(sha256sum "$archive" | awk '{print $1}')
[ "$actual_sha256" = "$expected_sha256" ]
phase=extract-payload
mkdir -m 755 "$payload"
tar -xzf "$archive" --strip-components=1 --no-same-owner -C "$payload"
phase=verify-payload
(cd "$payload" && sha256sum -c SHA256SUMS >/dev/null 2>&1)
[ "$(stat -c '%a' "$server")" = 755 ]
[ "$(stat -c '%a' "$payload/libexec/bli-kvm-platform-broker")" = 755 ]
[ -f "$payload/share/web/index.html" ]
phase=prepare-runtime
chmod 711 "$stage_root"
uid=$(getent passwd nobody | head -n 1 | awk -F: '{print $3}')
gid=$(getent passwd nobody | head -n 1 | awk -F: '{print $4}')
state="$stage_root/state"
mkdir -m 700 "$state"
chown "$uid:$gid" "$state"
: > "$stage_root/server.log"
chmod 600 "$stage_root/server.log"
phase=launch-server
setpriv --reuid="$uid" --regid="$gid" --clear-groups --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME="$state" TMPDIR="$state" \
  BLI_KVM_BIND="127.0.0.1:$bind_port" \
  BLI_KVM_WEB_ROOT="$payload/share/web" \
  BLI_KVM_STATE_DIR="$state" \
  BLI_KVM_BROKER_SOCKET="$stage_root/missing-broker.sock" \
  BLI_KVM_SECURE_COOKIES=0 BLI_KVM_ALLOW_MISSING_ORIGIN=0 \
  "$server" > "$stage_root/server.log" 2>&1 < /dev/null &
pid=$!
printf '%s\n' "$pid" > "$stage_root/server.pid"
phase=wait-health
ready=no
count=0
while [ "$count" -lt 50 ]; do
  if curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null "http://127.0.0.1:$bind_port/api/health" 2>/dev/null; then
    ready=yes
    break
  fi
  if ! process_active; then
    set +e
    wait "$pid"
    server_status=$?
    set -e
    pid=
    classify_server_exit "$server_status"
    exit 1
  fi
  sleep 0.1
  count=$((count + 1))
done
if [ "$ready" != yes ]; then
  listener_count=$(awk -v address="0100007F:$port_hex" 'NR > 1 && toupper($2) == address && $4 == "0A" {count++} END {print count + 0}' /proc/net/tcp)
  if [ "$listener_count" -gt 0 ]; then phase=wait-health-curl; else phase=wait-health-no-listener; fi
  exit 1
fi
phase=verify-process
status_file="/proc/$pid/status"
[ -r "$status_file" ]
uids=$(awk '/^Uid:/ {print $2 " " $3 " " $4 " " $5}' "$status_file")
gids=$(awk '/^Gid:/ {print $2 " " $3 " " $4 " " $5}' "$status_file")
case " $uids " in *' 0 '*) exit 1 ;; esac
case " $gids " in *' 0 '*) exit 1 ;; esac
[ "$(awk '/^Uid:/ {print $3}' "$status_file")" = "$uid" ]
[ "$(awk '/^Gid:/ {print $3}' "$status_file")" = "$gid" ]
[ "$(awk '/^CapEff:/ {print $2}' "$status_file")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "$status_file")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "$status_file")" = 1 ]
[ -z "$(awk '/^Groups:/ {$1=""; sub(/^ +/, ""); print}' "$status_file")" ]
phase=verify-listener
listener_count=$(awk -v address="0100007F:$port_hex" 'NR > 1 && toupper($2) == address && $4 == "0A" {count++} END {print count + 0}' /proc/net/tcp)
[ "$listener_count" = 1 ]
other_listener=no
for table in /proc/net/tcp /proc/net/tcp6; do
  [ -r "$table" ] || continue
  if awk -v suffix=":$port_hex" -v expected="0100007F:$port_hex" '
    NR > 1 && toupper($2) ~ (suffix "$") && $4 == "0A" && toupper($2) != expected { found = 1 }
    END { exit(found ? 0 : 1) }
  ' "$table"; then other_listener=yes; fi
done
[ "$other_listener" = no ]
phase=verify-http
health_code=$(curl --noproxy '*' --silent --max-time 2 --output "$stage_root/health.json" --write-out '%{http_code}' "http://127.0.0.1:$bind_port/api/health" 2>/dev/null)
[ "$health_code" = 200 ]
grep -q '"status":"ok"' "$stage_root/health.json"
grep -q '"backend":"rust"' "$stage_root/health.json"
capabilities_code=$(curl --noproxy '*' --silent --max-time 2 --output "$stage_root/capabilities.json" --write-out '%{http_code}' "http://127.0.0.1:$bind_port/api/capabilities" 2>/dev/null)
[ "$capabilities_code" = 200 ]
grep -q '"schemaVersion":1' "$stage_root/capabilities.json"
setup_code=$(curl --noproxy '*' --silent --max-time 2 --output "$stage_root/setup.json" --write-out '%{http_code}' "http://127.0.0.1:$bind_port/api/auth/setup" 2>/dev/null)
[ "$setup_code" = 200 ]
grep -q '"required":true' "$stage_root/setup.json"
static_code=$(curl --noproxy '*' --silent --max-time 2 --dump-header "$stage_root/headers.txt" --output "$stage_root/index.html" --write-out '%{http_code}' "http://127.0.0.1:$bind_port/" 2>/dev/null)
[ "$static_code" = 200 ]
grep -q '<title>Hardened BliKVM</title>' "$stage_root/index.html"
grep -qi '^x-content-type-options: nosniff' "$stage_root/headers.txt"
grep -qi '^x-frame-options: DENY' "$stage_root/headers.txt"
grep -qi '^referrer-policy: no-referrer' "$stage_root/headers.txt"
inventory_code=$(curl --noproxy '*' --silent --max-time 2 --output "$stage_root/inventory.json" --write-out '%{http_code}' "http://127.0.0.1:$bind_port/api/inventory" 2>/dev/null)
[ "$inventory_code" = 401 ]
phase=stress-health
count=0
while [ "$count" -lt "$health_requests" ]; do
  code=$(curl --noproxy '*' --silent --max-time 2 --output /dev/null --write-out '%{http_code}' "http://127.0.0.1:$bind_port/api/health" 2>/dev/null)
  [ "$code" = 200 ]
  count=$((count + 1))
done
phase=reject-public-bind
if setpriv --reuid="$uid" --regid="$gid" --clear-groups --bounding-set=-all --no-new-privs \
  env -i PATH=/usr/bin:/bin HOME="$state" TMPDIR="$state" \
  BLI_KVM_BIND="0.0.0.0:$bind_port" BLI_KVM_WEB_ROOT="$payload/share/web" \
  BLI_KVM_STATE_DIR="$state" BLI_KVM_BROKER_SOCKET="$stage_root/missing-broker.sock" \
  "$server" > "$stage_root/nonloopback.log" 2>&1 < /dev/null; then
  exit 1
fi
grep -q 'refusing a non-loopback bind' "$stage_root/nonloopback.log"
phase=verify-existing-services
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=stop-server
stop_server
phase=remove-stage
owned_stage
rm -rf -- "$stage_root"
[ ! -e "$stage_root" ]
run_succeeded=yes
trap - EXIT HUP INT TERM
emit bundle_sha256 "$actual_sha256"
emit payload_checksums verified
emit server_mode 755
emit broker_mode 755
emit server_uid unprivileged
emit server_gid unprivileged
emit server_caps none
emit server_groups cleared
emit no_new_privs yes
emit loopback_listener exclusive
emit health_status "$health_code"
emit capabilities_status "$capabilities_code"
emit setup_status "$setup_code"
emit static_status "$static_code"
emit inventory_unauth_status "$inventory_code"
emit health_requests "$health_requests"
emit security_headers verified
emit nonloopback_refused yes
emit graceful_stop yes
emit kvmd_active_after_run active
emit kvmd_nginx_active_after_run active
emit rootfs_mode_after_run "$rootfs_mode"
emit staging_removed yes
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{body}"


def build_cleanup_script(paths: StagePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "run_id": paths.run_id,
            "server": paths.server,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ ! -e "$stage_root" ]; then emit cleanup_removed yes; exit 0; fi
[ -f "$marker" ]
[ "$(cat "$marker" 2>/dev/null || true)" = "$run_id" ]
if [ -f "$stage_root/server.pid" ]; then
  pid=$(cat "$stage_root/server.pid")
  case "$pid" in *[!0-9]*|'') exit 1 ;; esac
  if kill -0 "$pid" 2>/dev/null; then
    state=$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)
    if [ -n "$state" ] && [ "$state" != Z ]; then
      [ "$(readlink "/proc/$pid/exe" 2>/dev/null || true)" = "$server" ]
      kill -TERM "$pid"
      count=0
      while kill -0 "$pid" 2>/dev/null && [ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)" != Z ] && [ "$count" -lt 50 ]; do sleep 0.1; count=$((count + 1)); done
      state=$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)
      if kill -0 "$pid" 2>/dev/null && [ -n "$state" ] && [ "$state" != Z ]; then
        kill -KILL "$pid"
        count=0
        while kill -0 "$pid" 2>/dev/null && [ "$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)" != Z ] && [ "$count" -lt 20 ]; do sleep 0.1; count=$((count + 1)); done
        state=$(awk '{print $3}' "/proc/$pid/stat" 2>/dev/null || true)
        if kill -0 "$pid" 2>/dev/null && [ -n "$state" ] && [ "$state" != Z ]; then exit 1; fi
      fi
    fi
  fi
fi
rm -rf -- "$stage_root"
[ ! -e "$stage_root" ]
emit cleanup_removed yes
""".strip()
    return f"{values}\n{body}"


def build_final_script(paths: StagePaths, bind_port: int) -> str:
    values = _assignments(
        {"stage_root": paths.root, "port_hex": f"{bind_port:04X}"}
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
if port_in_use; then exit 1; fi
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
emit stage_absent_final yes
emit port_free_final yes
emit rootfs_mode_final "$rootfs_mode"
emit kvmd_active_final active
emit kvmd_nginx_active_final active
""".strip()
    return f"{values}\n{PORT_FUNCTION}\n{body}"


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > MAX_REMOTE_OUTPUT_BYTES:
        raise ValueError("remote output exceeded the size limit")
    decoded = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in decoded.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in expected_keys or key in facts:
            raise ValueError("remote output contained an unexpected field")
        if not SAFE_VALUE.fullmatch(value):
            raise ValueError("remote output contained an unsafe value")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("remote output was incomplete")
    return dict(sorted(facts.items()))


def parse_runtime_failure(raw: bytes) -> tuple[str | None, bool | None]:
    try:
        facts = parse_facts(raw, RUNTIME_FAILURE_KEYS)
    except (UnicodeError, ValueError):
        return None, None
    phase = facts["failure_phase"]
    cleanup = facts["failure_cleanup"]
    if cleanup not in {"removed", "retained"}:
        return None, None
    return phase, cleanup == "removed"


def _require_result(
    stage: str, result: subprocess.CompletedProcess[bytes]
) -> subprocess.CompletedProcess[bytes]:
    if result.returncode != 0:
        remote_phase: str | None = None
        remote_cleanup_confirmed: bool | None = None
        if stage == "runtime":
            remote_phase, remote_cleanup_confirmed = parse_runtime_failure(result.stdout)
        raise StageError(
            stage,
            "remote-command",
            return_code=result.returncode,
            stdout_bytes=len(result.stdout),
            remote_phase=remote_phase,
            remote_cleanup_confirmed=remote_cleanup_confirmed,
        )
    if len(result.stdout) > MAX_REMOTE_OUTPUT_BYTES:
        raise StageError(stage, "output-limit", stdout_bytes=len(result.stdout))
    return result


def _validate_preflight(facts: dict[str, str], required_bytes: int) -> None:
    expected = {
        "architecture": "aarch64",
        "rootfs_mode_before": "rw",
        "tmp_mount_mode": "exec",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "nobody_uid_nonzero": "yes",
        "nobody_gid_nonzero": "yes",
        "setpriv_ready": "yes",
        "tools_ready": "yes",
        "stage_absent": "yes",
        "port_free_before": "yes",
    }
    if any(facts.get(key) != value for key, value in expected.items()):
        raise ValueError("preflight invariant failed")
    available = facts.get("tmp_available_bytes", "")
    if not available.isdigit() or int(available) < required_bytes:
        raise ValueError("temporary staging space is insufficient")


def _validate_run(facts: dict[str, str], bundle_sha256: str) -> None:
    expected = {
        "bundle_sha256": bundle_sha256,
        "payload_checksums": "verified",
        "server_mode": "755",
        "broker_mode": "755",
        "server_uid": "unprivileged",
        "server_gid": "unprivileged",
        "server_caps": "none",
        "server_groups": "cleared",
        "no_new_privs": "yes",
        "loopback_listener": "exclusive",
        "health_status": "200",
        "capabilities_status": "200",
        "setup_status": "200",
        "static_status": "200",
        "inventory_unauth_status": "401",
        "health_requests": str(HEALTH_REQUESTS),
        "security_headers": "verified",
        "nonloopback_refused": "yes",
        "graceful_stop": "yes",
        "kvmd_active_after_run": "active",
        "kvmd_nginx_active_after_run": "active",
        "rootfs_mode_after_run": "rw",
        "staging_removed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def _validate_final(facts: dict[str, str]) -> None:
    expected = {
        "stage_absent_final": "yes",
        "port_free_final": "yes",
        "rootfs_mode_final": "rw",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def _bundle_receipt(bundle: BundleInfo) -> BundleReceipt:
    return {
        "sha256": bundle.sha256,
        "sizeBytes": bundle.size_bytes,
        "sourceCommit": bundle.source_commit,
    }


def _policy_receipt() -> PolicyReceipt:
    return {
        "brokerStarted": False,
        "rootfsHelpersInvoked": False,
        "stagingFilesystem": "/tmp",
        "persistentPathsChanged": False,
    }


def _attempt_cleanup(
    target: RemoteTarget, secret: bytearray, paths: StagePaths
) -> CleanupReceipt:
    try:
        result = run_ssh(target, secret, build_cleanup_script(paths), timeout=20)
        if result.returncode != 0:
            return {"attempted": True, "confirmed": False}
        facts = parse_facts(result.stdout, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_removed": "yes"}}
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


def _no_cleanup_needed() -> CleanupReceipt:
    return {"attempted": False, "confirmed": True}


def execute(options: Options) -> int:
    output = validate_output_path(options.output)
    expected_sha256 = validate_sha256(options.expected_sha256)
    bundle = validate_bundle(options.bundle, expected_sha256)
    host, ssh_port = parse_endpoint(options.endpoint)
    username = validate_username(options.username)
    known_hosts = validate_private_file(options.known_hosts)
    bind_port = validate_bind_port(options.bind_port)
    target = RemoteTarget(host, ssh_port, username, known_hosts)
    paths = stage_paths(bundle.sha256, secrets.token_hex(8))
    required_bytes = bundle.size_bytes * 4 + MIN_TMP_HEADROOM_BYTES
    cleanup_required = False
    secret = read_secret_stdin()
    stage = "initialization"
    try:
        stage = "preflight"
        preflight_result = _require_result(
            stage,
            run_ssh(
                target,
                secret,
                build_preflight_script(paths, required_bytes, bind_port),
                timeout=20,
            ),
        )
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        _validate_preflight(preflight, required_bytes)

        stage = "upload"
        cleanup_required = True
        with bundle.path.open("rb") as bundle_stream:
            _ = _require_result(
                stage,
                run_ssh(
                    target,
                    secret,
                    build_upload_script(paths),
                    stdin=bundle_stream,
                    timeout=60,
                ),
            )

        stage = "runtime"
        run_result = _require_result(
            stage,
            run_ssh(
                target,
                secret,
                build_run_script(paths, bundle, bind_port),
                timeout=60,
            ),
        )
        checks = parse_facts(run_result.stdout, RUN_KEYS)
        _validate_run(checks, bundle.sha256)

        stage = "final-state"
        final_result = _require_result(
            stage,
            run_ssh(target, secret, build_final_script(paths, bind_port), timeout=20),
        )
        final_state = parse_facts(final_result.stdout, FINAL_KEYS)
        _validate_final(final_state)
        cleanup_required = False

        receipt: PassedReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-device-prototype",
            "result": "passed",
            "bundle": _bundle_receipt(bundle),
            "policy": _policy_receipt(),
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except StageError as error:
        cleanup = (
            _attempt_cleanup(target, secret, paths)
            if cleanup_required
            else _no_cleanup_needed()
        )
        failed: FailedReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-device-prototype",
            "result": "failed",
            "bundle": _bundle_receipt(bundle),
            "policy": _policy_receipt(),
            "failure": {
                "stage": error.stage,
                "failureType": error.failure_type,
                "returnCode": error.return_code,
                "stdoutBytes": error.stdout_bytes,
                "remotePhase": error.remote_phase,
                "remoteCleanupConfirmed": error.remote_cleanup_confirmed,
            },
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, failed)
        return 1
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            _attempt_cleanup(target, secret, paths)
            if cleanup_required
            else _no_cleanup_needed()
        )
        failed = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-temporary-device-prototype",
            "result": "failed",
            "bundle": _bundle_receipt(bundle),
            "policy": _policy_receipt(),
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
        write_json_0600_atomic(output, failed)
        return 1
    finally:
        _zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, tarfile.TarError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
