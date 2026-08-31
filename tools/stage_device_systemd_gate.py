#!/usr/bin/env python3
"""Run a reversible dedicated-account and systemd sandbox gate on test H616."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


ACCOUNT = "hardened-blikvm-broker"
BROKER_UNIT = "hardened-blikvm-platform-broker.service"
VIDEO_UNIT = "hardened-blikvm-video-proxy.service"
SERVER_UNIT = "hardened-blikvm-server.service"
INVENTORY_REQUESTS = 20
VIDEO_RECONNECTS = 5
JPEG_COUNTER_AWK = (
    "{for (field = 1; field <= NF; field++) "
    "{if (previous == 255 && $field == 216) frames++; previous = $field}} "
    "END {print frames + 0}"
)
PREFLIGHT_KEYS = {
    "account_absent",
    "architecture",
    "htpasswd_ready",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "port_free_before",
    "rootfs_mode_before",
    "stage_absent",
    "systemd_ready",
    "tmp_mount_mode",
    "tools_ready",
    "unit_names_free",
    "ustreamer_socket",
    "ustreamer_socket_group",
    "ustreamer_socket_mode",
}
RUN_KEYS = {
    "account_created",
    "account_removed",
    "broker_caps",
    "broker_load_credential",
    "broker_network_namespace",
    "broker_socket_mode",
    "broker_unit",
    "bundle_sha256",
    "capabilities_status",
    "dedicated_credential",
    "htpasswd_restored",
    "inventory_backend",
    "inventory_requests",
    "inventory_sanitized",
    "inventory_schema",
    "inventory_status",
    "kvmd_active_after_run",
    "kvmd_nginx_active_after_run",
    "login_status",
    "no_new_privs",
    "payload_checksums",
    "rootfs_mode_after_run",
    "rtc_capability",
    "rtc_evidence",
    "rtc_offset_seconds",
    "rtc_unix_seconds",
    "runtime_profile",
    "server_caps",
    "server_host_network",
    "server_no_credential",
    "server_no_kvmd_group",
    "server_unit",
    "setup_status",
    "staging_removed",
    "thermal_capability",
    "thermal_evidence",
    "thermal_millidegrees",
    "units_removed",
    "video_caps",
    "video_content_type",
    "video_frames",
    "video_network_namespace",
    "video_no_device_access",
    "video_reconnects",
    "video_session_revocation",
    "video_socket_mode",
    "video_status",
    "video_unit",
    "watchdog_armed",
}
FINAL_KEYS = {
    "account_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "port_free_final",
    "rootfs_mode_final",
    "runtime_absent_final",
    "stage_absent_final",
    "units_absent_final",
    "watchdog_inactive_final",
}
FAILURE_KEYS = {"failure_cleanup", "failure_phase"}
CLEANUP_KEYS = {"cleanup_restored"}
SAFE_PHASE = re.compile(r"^[a-z0-9-]{1,120}$")


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


class PolicyReceipt(TypedDict):
    accountMutation: str
    accountRollback: str
    unitInstallation: str
    watchdog: str
    physicalActionsInvoked: bool
    kvmdRestarted: bool
    rootfsHelpersInvoked: bool
    persistentChangeScope: str
    runtimeProfile: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    bundle: str
    expected_sha256: str
    output: str
    bind_port: int
    thermal_only: bool


@dataclass(frozen=True)
class GatePaths:
    stage: prototype.StagePaths
    test_root: str
    owner_marker: str
    credentials: str
    htpasswd: str
    htpasswd_backup: str
    rollback_script: str
    rollback_hashes: str
    broker_unit_path: str
    video_unit_path: str
    server_unit_path: str
    broker_dropin_dir: str
    video_dropin_dir: str
    server_dropin_dir: str
    watchdog: str
    server_state_name: str


class GateError(Exception):
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
    _ = parser.add_argument(
        "--thermal-only",
        action="store_true",
        help="stop after the authenticated thermal inventory gate and mark video stream checks skipped",
    )
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        bundle=cast(str, namespace.bundle),
        expected_sha256=cast(str, namespace.expected_sha256),
        output=cast(str, namespace.output),
        bind_port=cast(int, namespace.bind_port),
        thermal_only=cast(bool, namespace.thermal_only),
    )


def gate_paths(bundle_sha256: str, run_id: str) -> GatePaths:
    stage = prototype.stage_paths(bundle_sha256, run_id)
    test_root = f"/opt/hardened-blikvm-test-{run_id}"
    return GatePaths(
        stage=stage,
        test_root=test_root,
        owner_marker=f"/opt/.hardened-blikvm-test-owner-{run_id}",
        credentials=f"{test_root}/credentials",
        htpasswd="/etc/kvmd/htpasswd",
        htpasswd_backup=f"{test_root}/htpasswd.backup",
        rollback_script=f"{test_root}/rollback.sh",
        rollback_hashes=f"{test_root}/rollback.hashes",
        broker_unit_path=f"/run/systemd/system/{BROKER_UNIT}",
        video_unit_path=f"/run/systemd/system/{VIDEO_UNIT}",
        server_unit_path=f"/run/systemd/system/{SERVER_UNIT}",
        broker_dropin_dir=f"/run/systemd/system/{BROKER_UNIT}.d",
        video_dropin_dir=f"/run/systemd/system/{VIDEO_UNIT}.d",
        server_dropin_dir=f"/run/systemd/system/{SERVER_UNIT}.d",
        watchdog=f"hardened-blikvm-rollback-{run_id}",
        server_state_name=f"hardened-blikvm-server-test-{run_id}",
    )


def _assignments(values: dict[str, str | int]) -> str:
    return "\n".join(
        f"{name}={shlex.quote(str(value))}" for name, value in values.items()
    )


def _encoded(value: str) -> str:
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def build_broker_dropin(paths: GatePaths) -> str:
    return "\n".join(
        (
            "[Service]",
            "User=daemon",
            "Group=nogroup",
            "SupplementaryGroups=",
            "SupplementaryGroups=kvmd",
            "LoadCredential=",
            f"LoadCredential=kvmd-user:{paths.credentials}/kvmd-user",
            f"LoadCredential=kvmd-password:{paths.credentials}/kvmd-password",
            "ExecStart=",
            f"ExecStart={paths.test_root}/payload/libexec/bli-kvm-platform-broker",
            "",
        )
    )


def build_server_dropin(paths: GatePaths, bind_port: int) -> str:
    state_path = f"/run/{paths.server_state_name}"
    return "\n".join(
        (
            "[Unit]",
            "ConditionPathExists=",
            f"ConditionPathExists={paths.test_root}/payload/share/web/index.html",
            "",
            "[Service]",
            "User=nobody",
            "Group=nogroup",
            "StateDirectory=",
            f"RuntimeDirectory={paths.server_state_name}",
            "RuntimeDirectoryMode=0700",
            f"Environment=BLI_KVM_BIND=127.0.0.1:{bind_port}",
            f"Environment=BLI_KVM_WEB_ROOT={paths.test_root}/payload/share/web",
            f"Environment=BLI_KVM_STATE_DIR={state_path}",
            "Environment=BLI_KVM_BROKER_SOCKET=/run/hardened-blikvm/platform-broker.sock",
            "Environment=BLI_KVM_VIDEO_SOCKET=/run/hardened-blikvm-video/mjpeg.sock",
            "Environment=BLI_KVM_SECURE_COOKIES=0",
            "Environment=BLI_KVM_ALLOW_MISSING_ORIGIN=0",
            "ExecStart=",
            f"ExecStart={paths.test_root}/payload/bin/bli-kvm-server",
            "SocketBindAllow=",
            f"SocketBindAllow=tcp:{bind_port}",
            "",
        )
    )


def build_video_dropin(paths: GatePaths) -> str:
    return "\n".join(
        (
            "[Service]",
            "User=www-data",
            "Group=nogroup",
            "SupplementaryGroups=",
            "SupplementaryGroups=kvmd",
            "ExecStart=",
            f"ExecStart={paths.test_root}/payload/libexec/bli-kvm-video-proxy",
            "",
        )
    )


def build_rollback_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "test_root": paths.test_root,
            "owner_marker": paths.owner_marker,
            "run_id": paths.stage.run_id,
            "htpasswd": paths.htpasswd,
            "htpasswd_backup": paths.htpasswd_backup,
            "broker_unit_path": paths.broker_unit_path,
            "video_unit_path": paths.video_unit_path,
            "server_unit_path": paths.server_unit_path,
            "broker_dropin_dir": paths.broker_dropin_dir,
            "video_dropin_dir": paths.video_dropin_dir,
            "server_dropin_dir": paths.server_dropin_dir,
            "watchdog": paths.watchdog,
            "server_state": f"/run/{paths.server_state_name}",
        }
    )
    body = r"""
set -eu
before_sha=$1
modified_sha=$2
[ -f "$owner_marker" ]
[ "$(cat "$owner_marker" 2>/dev/null || true)" = "$run_id" ]
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${INVOCATION_ID:-}" ]; then systemctl stop "$watchdog.service" 2>/dev/null || true; fi
systemctl stop hardened-blikvm-server.service 2>/dev/null || true
systemctl stop hardened-blikvm-video-proxy.service 2>/dev/null || true
systemctl stop hardened-blikvm-platform-broker.service 2>/dev/null || true
current_sha=$(sha256sum "$htpasswd" | awk '{print $1}')
if [ "$current_sha" = "$modified_sha" ]; then
  restore_tmp="/etc/kvmd/.htpasswd.hardened-blikvm-$run_id"
  [ ! -e "$restore_tmp" ]
  cp -a -- "$htpasswd_backup" "$restore_tmp"
  mv -f -- "$restore_tmp" "$htpasswd"
elif [ "$current_sha" != "$before_sha" ]; then
  exit 3
fi
[ "$(sha256sum "$htpasswd" | awk '{print $1}')" = "$before_sha" ]
rm -f -- "$broker_unit_path" "$video_unit_path" "$server_unit_path"
rm -rf -- "$broker_dropin_dir" "$video_dropin_dir" "$server_dropin_dir"
systemctl daemon-reload
rm -rf -- /run/hardened-blikvm /run/hardened-blikvm-video "$server_state"
rm -rf -- "$stage_root"
rm -rf -- "$test_root"
rm -f -- "$owner_marker"
""".strip()
    return f"{values}\n{body}\n"


def build_preflight_script(
    paths: GatePaths, required_bytes: int, bind_port: int
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "test_root": paths.test_root,
            "owner_marker": paths.owner_marker,
            "htpasswd": paths.htpasswd,
            "broker_unit_path": paths.broker_unit_path,
            "video_unit_path": paths.video_unit_path,
            "server_unit_path": paths.server_unit_path,
            "broker_dropin_dir": paths.broker_dropin_dir,
            "video_dropin_dir": paths.video_dropin_dir,
            "server_dropin_dir": paths.server_dropin_dir,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "port_hex": f"{bind_port:04X}",
            "account": ACCOUNT,
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
root_available=$(df -B1 --output=avail /opt | tail -n 1 | tr -d ' ')
[ "$root_available" -ge "$required_bytes" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
ustreamer_socket=/run/kvmd/ustreamer.sock
[ -S "$ustreamer_socket" ]
ustreamer_mode=$(stat -c '%a' "$ustreamer_socket")
ustreamer_group=$(stat -c '%G' "$ustreamer_socket")
[ "$ustreamer_group" = kvmd ]
for tool in awk base64 cat chmod chown cp curl df env findmnt getent grep head id install kvmd-htpasswd mkdir mv od readlink rm sed setpriv sha256sum sleep stat systemctl systemd-run tail tar tr wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
[ -f "$htpasswd" ]
[ ! -L "$htpasswd" ]
[ "$(stat -c '%a' "$htpasswd")" -le 600 ]
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$account"; then exit 1; fi
[ ! -e "$stage_root" ]
[ ! -e "$test_root" ]
[ ! -e "$owner_marker" ]
[ ! -e "$broker_unit_path" ]
[ ! -e "$video_unit_path" ]
[ ! -e "$server_unit_path" ]
[ ! -e "$broker_dropin_dir" ]
[ ! -e "$video_dropin_dir" ]
[ ! -e "$server_dropin_dir" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
if systemctl list-unit-files "$watchdog.timer" --no-legend 2>/dev/null | grep -q .; then exit 1; fi
if port_in_use; then exit 1; fi
version=$(systemctl --version | head -n 1 | awk '{print $2}')
case "$version" in ''|*[!0-9]*) exit 1 ;; esac
[ "$version" -ge 249 ]
emit architecture aarch64
emit rootfs_mode_before "$rootfs_mode"
emit tmp_mount_mode "$tmp_mount_mode"
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit htpasswd_ready yes
emit account_absent yes
emit systemd_ready yes
emit tools_ready yes
emit stage_absent yes
emit unit_names_free yes
emit port_free_before yes
emit ustreamer_socket present
emit ustreamer_socket_mode "$ustreamer_mode"
emit ustreamer_socket_group "$ustreamer_group"
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


def build_runtime_script(
    paths: GatePaths,
    bundle: prototype.BundleInfo,
    bind_port: int,
    thermal_only: bool = False,
) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "test_root": paths.test_root,
            "owner_marker": paths.owner_marker,
            "run_id": paths.stage.run_id,
            "archive": paths.stage.archive,
            "payload": paths.stage.payload,
            "expected_sha256": bundle.sha256,
            "htpasswd": paths.htpasswd,
            "htpasswd_backup": paths.htpasswd_backup,
            "credentials": paths.credentials,
            "rollback_script": paths.rollback_script,
            "rollback_hashes": paths.rollback_hashes,
            "broker_unit_path": paths.broker_unit_path,
            "video_unit_path": paths.video_unit_path,
            "server_unit_path": paths.server_unit_path,
            "broker_dropin_dir": paths.broker_dropin_dir,
            "video_dropin_dir": paths.video_dropin_dir,
            "server_dropin_dir": paths.server_dropin_dir,
            "watchdog": paths.watchdog,
            "account": ACCOUNT,
            "bind_port": bind_port,
            "port_hex": f"{bind_port:04X}",
            "inventory_requests": INVENTORY_REQUESTS,
            "video_reconnects": VIDEO_RECONNECTS,
            "jpeg_counter_awk": JPEG_COUNTER_AWK,
            "runtime_profile": "thermal-only" if thermal_only else "full",
            "rollback_b64": _encoded(build_rollback_script(paths)),
            "broker_dropin_b64": _encoded(build_broker_dropin(paths)),
            "video_dropin_b64": _encoded(build_video_dropin(paths)),
            "server_dropin_b64": _encoded(build_server_dropin(paths, bind_port)),
        }
    )
    body = r"""
set -eu
export LC_ALL=C
umask 077
phase=validate-stage
run_succeeded=no
mutation_started=no
before_sha=
modified_sha=
emit() { printf '%s=%s\n' "$1" "$2"; }
owned_stage() {
  [ -f "$stage_marker" ] && [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
}
owned_test() {
  [ -f "$owner_marker" ] && [ "$(cat "$owner_marker" 2>/dev/null || true)" = "$run_id" ]
}
minimal_cleanup() {
  set +e
  systemctl stop hardened-blikvm-server.service 2>/dev/null
  systemctl stop hardened-blikvm-video-proxy.service 2>/dev/null
  systemctl stop hardened-blikvm-platform-broker.service 2>/dev/null
  if owned_test; then
    rm -f -- "$broker_unit_path" "$video_unit_path" "$server_unit_path"
    rm -rf -- "$broker_dropin_dir" "$video_dropin_dir" "$server_dropin_dir"
    systemctl daemon-reload
    rm -rf -- /run/hardened-blikvm /run/hardened-blikvm-video "/run/hardened-blikvm-server-test-$run_id"
    rm -rf -- "$test_root"
    rm -f -- "$owner_marker"
  fi
  if owned_stage; then rm -rf -- "$stage_root"; fi
}
emergency_cleanup() {
  set +e
  cleanup_state=retained
  if [ -f "$rollback_hashes" ] && [ -x "$rollback_script" ]; then
    before=$(awk 'NR == 1 {print}' "$rollback_hashes")
    modified=$(awk 'NR == 2 {print}' "$rollback_hashes")
    if /bin/sh "$rollback_script" "$before" "$modified"; then cleanup_state=restored; fi
  elif [ "$mutation_started" = no ]; then
    minimal_cleanup
    cleanup_state=restored
  elif [ -f "$htpasswd_backup" ] && owned_test; then
    restore_tmp="/etc/kvmd/.htpasswd.hardened-blikvm-$run_id"
    cp -a -- "$htpasswd_backup" "$restore_tmp"
    mv -f -- "$restore_tmp" "$htpasswd"
    minimal_cleanup
    cleanup_state=restored
  fi
}
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then
    emergency_cleanup
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
mkdir -m 755 "$payload"
tar -xzf "$archive" --strip-components=1 --no-same-owner -C "$payload"
(cd "$payload" && sha256sum -c SHA256SUMS >/dev/null 2>&1)
[ -f "$payload/share/systemd/hardened-blikvm-platform-broker.service" ]
[ -f "$payload/share/systemd/hardened-blikvm-video-proxy.service" ]
[ -f "$payload/share/systemd/hardened-blikvm-server.service" ]
phase=prepare-test-root
[ ! -e "$test_root" ]
[ ! -e "$owner_marker" ]
printf '%s\n' "$run_id" > "$owner_marker"
chmod 600 "$owner_marker"
install -d -m 755 "$test_root"
cp -a -- "$payload" "$test_root/payload"
install -d -m 700 "$credentials"
cp -a -- "$htpasswd" "$htpasswd_backup"
chmod 600 "$htpasswd_backup"
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
printf '%s\n' "$account" > "$credentials/kvmd-user"
od -An -N24 -tx1 /dev/urandom | tr -d ' \n' > "$credentials/kvmd-password"
chmod 400 "$credentials/kvmd-user" "$credentials/kvmd-password"
phase=install-runtime-units
install -m 644 "$test_root/payload/share/systemd/hardened-blikvm-platform-broker.service" "$broker_unit_path"
install -m 644 "$test_root/payload/share/systemd/hardened-blikvm-video-proxy.service" "$video_unit_path"
install -m 644 "$test_root/payload/share/systemd/hardened-blikvm-server.service" "$server_unit_path"
install -d -m 755 "$broker_dropin_dir" "$video_dropin_dir" "$server_dropin_dir"
printf '%s' "$broker_dropin_b64" | base64 -d > "$broker_dropin_dir/test.conf"
printf '%s' "$video_dropin_b64" | base64 -d > "$video_dropin_dir/test.conf"
printf '%s' "$server_dropin_b64" | base64 -d > "$server_dropin_dir/test.conf"
chmod 644 "$broker_dropin_dir/test.conf" "$video_dropin_dir/test.conf" "$server_dropin_dir/test.conf"
systemctl daemon-reload
systemctl cat hardened-blikvm-platform-broker.service >/dev/null
systemctl cat hardened-blikvm-video-proxy.service >/dev/null
systemctl cat hardened-blikvm-server.service >/dev/null
phase=create-dedicated-account
before_sha=$(sha256sum "$htpasswd" | awk '{print $1}')
mutation_started=yes
kvmd-htpasswd set -i -q "$account" < "$credentials/kvmd-password" >/dev/null 2>&1
kvmd-htpasswd list 2>/dev/null | grep -Fxq "$account"
modified_sha=$(sha256sum "$htpasswd" | awk '{print $1}')
[ "$modified_sha" != "$before_sha" ]
printf '%s\n%s\n' "$before_sha" "$modified_sha" > "$rollback_hashes"
chmod 600 "$rollback_hashes"
phase=arm-watchdog
systemd-run --quiet --unit="$watchdog" --on-active=10min --timer-property=AccuracySec=1s \
  /bin/sh "$rollback_script" "$before_sha" "$modified_sha"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
phase=verify-dedicated-account
curl_config="$test_root/kvmd-curl.conf"
{
  printf 'header = "X-KVMD-User: %s"\n' "$account"
  printf 'header = "X-KVMD-Passwd: '
  cat "$credentials/kvmd-password"
  printf '"\n'
} > "$curl_config"
chmod 600 "$curl_config"
direct_status=$(curl --silent --max-time 8 --config "$curl_config" --unix-socket /run/kvmd/kvmd.sock \
  --output "$test_root/direct-info.json" --write-out '%{http_code}' http://localhost/info 2>/dev/null)
[ "$direct_status" = 200 ]
rm -f -- "$curl_config" "$test_root/direct-info.json"
phase=start-broker-unit
systemctl start hardened-blikvm-platform-broker.service
count=0
while [ "$count" -lt 100 ] && [ ! -S /run/hardened-blikvm/platform-broker.sock ]; do
  [ "$(systemctl is-active hardened-blikvm-platform-broker.service 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S /run/hardened-blikvm/platform-broker.sock ]
broker_pid=$(systemctl show -p MainPID --value hardened-blikvm-platform-broker.service)
case "$broker_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
phase=verify-broker-unit
[ "$(systemctl show -p User --value hardened-blikvm-platform-broker.service)" = daemon ]
[ "$(systemctl show -p Group --value hardened-blikvm-platform-broker.service)" = nogroup ]
[ "$(systemctl show -p PrivateNetwork --value hardened-blikvm-platform-broker.service)" = yes ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$broker_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$broker_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$broker_pid/status")" = 1 ]
[ "$(readlink "/proc/$broker_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
[ "$(stat -c '%a' /run/hardened-blikvm/platform-broker.sock)" = 660 ]
credentials_directory=$(tr '\0' '\n' < "/proc/$broker_pid/environ" | sed -n 's/^CREDENTIALS_DIRECTORY=//p')
[ -n "$credentials_directory" ]
[ "$(stat -c '%a' "$credentials_directory/kvmd-user")" = 400 ]
[ "$(stat -c '%a' "$credentials_directory/kvmd-password")" = 400 ]
phase=start-video-unit
systemctl start hardened-blikvm-video-proxy.service
count=0
while [ "$count" -lt 100 ] && [ ! -S /run/hardened-blikvm-video/mjpeg.sock ]; do
  [ "$(systemctl is-active hardened-blikvm-video-proxy.service 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S /run/hardened-blikvm-video/mjpeg.sock ]
video_pid=$(systemctl show -p MainPID --value hardened-blikvm-video-proxy.service)
case "$video_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
phase=verify-video-unit
[ "$(systemctl show -p User --value hardened-blikvm-video-proxy.service)" = www-data ]
[ "$(systemctl show -p Group --value hardened-blikvm-video-proxy.service)" = nogroup ]
[ "$(systemctl show -p PrivateNetwork --value hardened-blikvm-video-proxy.service)" = yes ]
[ "$(systemctl show -p DevicePolicy --value hardened-blikvm-video-proxy.service)" = closed ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$video_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$video_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$video_pid/status")" = 1 ]
[ "$(readlink "/proc/$video_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
kvmd_gid=$(getent group kvmd | head -n 1 | awk -F: '{print $3}')
case " $(awk '/^Groups:/ {$1=""; print}' "/proc/$video_pid/status") " in *" $kvmd_gid "*) ;; *) exit 1 ;; esac
[ "$(stat -c '%a' /run/hardened-blikvm-video)" = 750 ]
[ "$(stat -c '%U' /run/hardened-blikvm-video)" = www-data ]
[ "$(stat -c '%G' /run/hardened-blikvm-video)" = nogroup ]
[ "$(stat -c '%a' /run/hardened-blikvm-video/mjpeg.sock)" = 660 ]
[ "$(stat -c '%U' /run/hardened-blikvm-video/mjpeg.sock)" = www-data ]
[ "$(stat -c '%G' /run/hardened-blikvm-video/mjpeg.sock)" = nogroup ]
phase=start-server-unit
systemctl start hardened-blikvm-server.service
count=0
ready=no
while [ "$count" -lt 100 ]; do
  if curl --noproxy '*' --silent --fail --max-time 1 --output /dev/null "http://127.0.0.1:$bind_port/api/health" 2>/dev/null; then ready=yes; break; fi
  [ "$(systemctl is-active hardened-blikvm-server.service 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ "$ready" = yes ]
server_pid=$(systemctl show -p MainPID --value hardened-blikvm-server.service)
case "$server_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
phase=verify-server-unit
[ "$(systemctl show -p User --value hardened-blikvm-server.service)" = nobody ]
[ "$(systemctl show -p Group --value hardened-blikvm-server.service)" = nogroup ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$server_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$server_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$server_pid/status")" = 1 ]
[ "$(readlink "/proc/$server_pid/ns/net")" = "$(readlink /proc/1/ns/net)" ]
case " $(awk '/^Groups:/ {$1=""; print}' "/proc/$server_pid/status") " in *" $kvmd_gid "*) exit 1 ;; esac
if tr '\0' '\n' < "/proc/$server_pid/environ" | grep -Eq '^(CREDENTIALS_DIRECTORY|KVMD_)='; then exit 1; fi
phase=authenticate
account_password=$(od -An -N24 -tx1 /dev/urandom | tr -d ' \n')
request="$test_root/account-request.json"
printf '{"username":"prototype-admin","password":"%s"}' "$account_password" > "$request"
origin="http://127.0.0.1:$bind_port"
setup_status=$(curl --noproxy '*' --silent --max-time 3 --output "$test_root/setup.json" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request" "$origin/api/auth/setup" 2>/dev/null)
[ "$setup_status" = 200 ]
cookie="$test_root/session.cookies"
login_status=$(curl --noproxy '*' --silent --max-time 3 --cookie-jar "$cookie" --output "$test_root/login.json" --write-out '%{http_code}' \
  -H 'Content-Type: application/json' -H "Origin: $origin" --data-binary "@$request" "$origin/api/auth/login" 2>/dev/null)
[ "$login_status" = 200 ]
unset account_password
rm -f -- "$request"
phase=inventory
inventory_status=$(curl --noproxy '*' --silent --max-time 12 --cookie "$cookie" --output "$test_root/inventory.json" --write-out '%{http_code}' "$origin/api/inventory" 2>/dev/null)
[ "$inventory_status" = 200 ]
grep -q '"schemaVersion":3' "$test_root/inventory.json"
grep -q '"backend":"kvmd_transition"' "$test_root/inventory.json"
if grep -Eqi '"(password|credential|username|serial|ssid|token|cookie|endpoint|socketPath|macAddress)"' "$test_root/inventory.json"; then exit 1; fi
phase=inventory-thermal
thermal_millidegrees=$(sed -n 's/.*"temperatureMillidegreesCelsius":\(-\{0,1\}[0-9][0-9]*\).*/\1/p' "$test_root/inventory.json")
printf '%s\n' "$thermal_millidegrees" | grep -Eq '^-?[0-9]+$'
[ "$thermal_millidegrees" -ge -40000 ]
[ "$thermal_millidegrees" -le 150000 ]
phase=inventory-rtc
rtc_unix_seconds=$(sed -n 's/.*"unixSeconds":\([0-9][0-9]*\).*/\1/p' "$test_root/inventory.json")
rtc_offset_seconds=$(sed -n 's/.*"offsetFromSystemSeconds":\(-\{0,1\}[0-9][0-9]*\).*/\1/p' "$test_root/inventory.json")
printf '%s\n' "$rtc_unix_seconds" | grep -Eq '^[0-9]+$'
printf '%s\n' "$rtc_offset_seconds" | grep -Eq '^-?[0-9]+$'
[ "$rtc_unix_seconds" -ge 946684800 ]
[ "$rtc_unix_seconds" -le 4102444799 ]
[ "$rtc_offset_seconds" -ge -315576000 ]
[ "$rtc_offset_seconds" -le 315576000 ]
phase=thermal-capability
capabilities_status=$(curl --noproxy '*' --silent --max-time 3 --cookie "$cookie" --output "$test_root/capabilities.json" --write-out '%{http_code}' "$origin/api/capabilities" 2>/dev/null)
[ "$capabilities_status" = 200 ]
grep -q '"thermal":{"availability":"read_only","evidence":"runtime_verified"}' "$test_root/capabilities.json"
grep -q '"rtc":{"availability":"read_only","evidence":"source_verified"}' "$test_root/capabilities.json"
thermal_capability=read_only
thermal_evidence=runtime_verified
rtc_capability=read_only
rtc_evidence=source_verified
count=1
while [ "$count" -lt "$inventory_requests" ]; do
  code=$(curl --noproxy '*' --silent --max-time 12 --cookie "$cookie" --output /dev/null --write-out '%{http_code}' "$origin/api/inventory" 2>/dev/null)
  [ "$code" = 200 ]
  count=$((count + 1))
done
if [ "$runtime_profile" = thermal-only ]; then
  phase=thermal-only-complete
  video_status=skipped
  video_content_type=skipped
  video_frames=skipped
  video_reconnects_result=skipped
  video_session_revocation=skipped
else
phase=video-stream
capture_video() {
  capture_file=$1
  capture_headers=$2
  capture_seconds=$3
  set +e
  capture_status=$(curl --noproxy '*' --silent --max-time "$capture_seconds" --cookie "$cookie" \
    --dump-header "$capture_headers" --output "$capture_file" --write-out '%{http_code}' \
    "$origin/api/stream/mjpeg" 2>/dev/null)
  capture_rc=$?
  set -e
  [ "$capture_status" = 200 ] || return 11
  case "$capture_rc" in 0|18|28) ;; *) return 12 ;; esac
  if ! tr -d '\r' < "$capture_headers" | grep -Eqi '^content-type: multipart/x-mixed-replace([;[:space:]]|$)'; then return 13; fi
  capture_bytes=$(wc -c < "$capture_file" | tr -d ' ')
  [ "$capture_bytes" -ge 1024 ] || return 14
  capture_frames=$(od -An -v -t u1 "$capture_file" | awk "$jpeg_counter_awk")
  [ "$capture_frames" -ge 2 ] || return 15
}
capture_failure_class() {
  case "$1" in
    11) printf http ;;
    12) printf curl ;;
    13) printf content-type ;;
    14) printf bytes ;;
    15) printf frames ;;
    *) printf unknown ;;
  esac
}
capture_video "$test_root/video.mjpeg" "$test_root/video.headers" 3
video_status=$capture_status
video_frames=$capture_frames
count=0
while [ "$count" -lt "$video_reconnects" ]; do
  capture_video "$test_root/video-reconnect.mjpeg" "$test_root/video-reconnect.headers" 1
  count=$((count + 1))
done
phase=video-concurrency
capture_video "$test_root/video-concurrent-a.mjpeg" "$test_root/video-concurrent-a.headers" 2 &
video_a_pid=$!
capture_video "$test_root/video-concurrent-b.mjpeg" "$test_root/video-concurrent-b.headers" 2 &
video_b_pid=$!
set +e
wait "$video_a_pid"
video_a_rc=$?
wait "$video_b_pid"
video_b_rc=$?
set -e
if [ "$video_a_rc" -ne 0 ] && [ "$video_b_rc" -ne 0 ]; then
  phase="video-concurrency-both-$(capture_failure_class "$video_a_rc")-$(capture_failure_class "$video_b_rc")"
  exit 1
elif [ "$video_a_rc" -ne 0 ]; then
  phase="video-concurrency-a-$(capture_failure_class "$video_a_rc")"
  exit 1
elif [ "$video_b_rc" -ne 0 ]; then
  phase="video-concurrency-b-$(capture_failure_class "$video_b_rc")"
  exit 1
fi
phase=video-session-revocation
set +e
curl --noproxy '*' --silent --max-time 20 --cookie "$cookie" \
  --output "$test_root/video-revoked.mjpeg" "$origin/api/stream/mjpeg" 2>/dev/null &
revoked_stream_pid=$!
set -e
count=0
while [ "$count" -lt 50 ]; do
  if [ -f "$test_root/video-revoked.mjpeg" ] && [ "$(wc -c < "$test_root/video-revoked.mjpeg" | tr -d ' ')" -ge 1024 ]; then break; fi
  kill -0 "$revoked_stream_pid" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ "$count" -lt 50 ]
kill -0 "$revoked_stream_pid" 2>/dev/null
csrf_token=$(sed -n 's/.*"csrfToken":"\([^"]*\)".*/\1/p' "$test_root/login.json")
[ -n "$csrf_token" ]
logout_status=$(curl --noproxy '*' --silent --max-time 3 --cookie "$cookie" --output "$test_root/logout.json" --write-out '%{http_code}' \
  -H "Origin: $origin" -H "x-csrf-token: $csrf_token" -X POST "$origin/api/auth/logout" 2>/dev/null)
unset csrf_token
[ "$logout_status" = 200 ]
count=0
while kill -0 "$revoked_stream_pid" 2>/dev/null && [ "$count" -lt 50 ]; do
  sleep 0.1
  count=$((count + 1))
done
if kill -0 "$revoked_stream_pid" 2>/dev/null; then exit 1; fi
set +e
wait "$revoked_stream_pid"
revoked_stream_rc=$?
set -e
case "$revoked_stream_rc" in 0|18|23|56) ;; *) exit 1 ;; esac
revoked_status=$(curl --noproxy '*' --silent --max-time 3 --cookie "$cookie" --output /dev/null --write-out '%{http_code}' "$origin/api/stream/mjpeg" 2>/dev/null)
[ "$revoked_status" = 401 ]
video_content_type=multipart-mjpeg
video_reconnects_result=$video_reconnects
video_session_revocation=passed
fi
phase=verify-existing-services
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
phase=rollback
/bin/sh "$rollback_script" "$before_sha" "$modified_sha"
[ "$(sha256sum "$htpasswd" | awk '{print $1}')" = "$before_sha" ]
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$account"; then exit 1; fi
[ ! -e "$stage_root" ]
[ ! -e "$test_root" ]
[ ! -e "$owner_marker" ]
[ ! -e "$broker_unit_path" ]
[ ! -e "$video_unit_path" ]
[ ! -e "$server_unit_path" ]
run_succeeded=yes
trap - EXIT HUP INT TERM
emit bundle_sha256 "$actual_sha256"
emit payload_checksums verified
emit runtime_profile "$runtime_profile"
emit dedicated_credential verified
emit account_created yes
emit watchdog_armed yes
emit broker_unit passed
emit broker_load_credential verified
emit broker_network_namespace private
emit broker_caps none
emit broker_socket_mode 660
emit video_unit passed
emit video_network_namespace private
emit video_caps none
emit video_no_device_access verified
emit video_socket_mode 660
emit server_unit passed
emit server_host_network loopback-restricted
emit server_no_credential verified
emit server_no_kvmd_group verified
emit server_caps none
emit no_new_privs yes
emit setup_status "$setup_status"
emit login_status "$login_status"
emit capabilities_status "$capabilities_status"
emit inventory_status "$inventory_status"
emit inventory_schema 3
emit inventory_backend kvmd_transition
emit inventory_sanitized yes
emit inventory_requests "$inventory_requests"
emit thermal_millidegrees "$thermal_millidegrees"
emit thermal_capability "$thermal_capability"
emit thermal_evidence "$thermal_evidence"
emit rtc_unix_seconds "$rtc_unix_seconds"
emit rtc_offset_seconds "$rtc_offset_seconds"
emit rtc_capability "$rtc_capability"
emit rtc_evidence "$rtc_evidence"
emit video_status "$video_status"
emit video_content_type "$video_content_type"
emit video_frames "$video_frames"
emit video_reconnects "$video_reconnects_result"
emit video_session_revocation "$video_session_revocation"
emit htpasswd_restored yes
emit account_removed yes
emit units_removed yes
emit staging_removed yes
emit kvmd_active_after_run active
emit kvmd_nginx_active_after_run active
emit rootfs_mode_after_run "$rootfs_mode"
""".strip()
    return f"{values}\n{prototype.PORT_FUNCTION}\n{body}"


def build_cleanup_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "stage_marker": paths.stage.marker,
            "test_root": paths.test_root,
            "owner_marker": paths.owner_marker,
            "run_id": paths.stage.run_id,
            "rollback_script": paths.rollback_script,
            "rollback_hashes": paths.rollback_hashes,
            "broker_unit_path": paths.broker_unit_path,
            "video_unit_path": paths.video_unit_path,
            "server_unit_path": paths.server_unit_path,
            "broker_dropin_dir": paths.broker_dropin_dir,
            "video_dropin_dir": paths.video_dropin_dir,
            "server_dropin_dir": paths.server_dropin_dir,
        }
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -f "$rollback_script" ] && [ -f "$rollback_hashes" ]; then
  before=$(awk 'NR == 1 {print}' "$rollback_hashes")
  modified=$(awk 'NR == 2 {print}' "$rollback_hashes")
  /bin/sh "$rollback_script" "$before" "$modified"
elif [ -e "$test_root" ] || [ -e "$owner_marker" ]; then
  [ -f "$owner_marker" ]
  [ "$(cat "$owner_marker" 2>/dev/null || true)" = "$run_id" ]
  if [ -f "$test_root/htpasswd.backup" ]; then
    before=$(sha256sum "$test_root/htpasswd.backup" | awk '{print $1}')
    current=$(sha256sum /etc/kvmd/htpasswd | awk '{print $1}')
    [ "$current" = "$before" ] || exit 4
  fi
  systemctl stop hardened-blikvm-server.service 2>/dev/null || true
  systemctl stop hardened-blikvm-video-proxy.service 2>/dev/null || true
  systemctl stop hardened-blikvm-platform-broker.service 2>/dev/null || true
  rm -f -- "$broker_unit_path" "$video_unit_path" "$server_unit_path"
  rm -rf -- "$broker_dropin_dir" "$video_dropin_dir" "$server_dropin_dir"
  systemctl daemon-reload
  rm -rf -- /run/hardened-blikvm /run/hardened-blikvm-video
  rm -rf -- "$test_root"
  rm -f -- "$owner_marker"
fi
if [ -e "$stage_root" ]; then
  [ -f "$stage_marker" ]
  [ "$(cat "$stage_marker" 2>/dev/null || true)" = "$run_id" ]
  rm -rf -- "$stage_root"
fi
[ ! -e "$stage_root" ]
[ ! -e "$test_root" ]
[ ! -e "$owner_marker" ]
emit cleanup_restored yes
""".strip()
    return f"{values}\n{body}"


def build_final_script(paths: GatePaths, bind_port: int) -> str:
    values = _assignments(
        {
            "stage_root": paths.stage.root,
            "test_root": paths.test_root,
            "owner_marker": paths.owner_marker,
            "broker_unit_path": paths.broker_unit_path,
            "video_unit_path": paths.video_unit_path,
            "server_unit_path": paths.server_unit_path,
            "port_hex": f"{bind_port:04X}",
            "account": ACCOUNT,
            "watchdog": paths.watchdog,
            "server_state": f"/run/{paths.server_state_name}",
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$test_root" ]
[ ! -e "$owner_marker" ]
[ ! -e "$broker_unit_path" ]
[ ! -e "$video_unit_path" ]
[ ! -e "$server_unit_path" ]
[ ! -e /run/hardened-blikvm ]
[ ! -e /run/hardened-blikvm-video ]
[ ! -e "$server_state" ]
if kvmd-htpasswd list 2>/dev/null | grep -Fxq "$account"; then exit 1; fi
if port_in_use; then exit 1; fi
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
emit stage_absent_final yes
emit units_absent_final yes
emit runtime_absent_final yes
emit watchdog_inactive_final yes
emit account_absent_final yes
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
    if not SAFE_PHASE.fullmatch(phase) or cleanup not in {"restored", "retained"}:
        return None, None
    return phase, cleanup == "restored"


def require_result(
    stage: str, result: subprocess.CompletedProcess[bytes]
) -> subprocess.CompletedProcess[bytes]:
    if result.returncode != 0:
        remote_phase: str | None = None
        remote_cleanup: bool | None = None
        if stage == "runtime":
            remote_phase, remote_cleanup = parse_failure(result.stdout)
        raise GateError(
            stage,
            "remote-command",
            return_code=result.returncode,
            stdout_bytes=len(result.stdout),
            remote_phase=remote_phase,
            remote_cleanup_confirmed=remote_cleanup,
        )
    if len(result.stdout) > prototype.MAX_REMOTE_OUTPUT_BYTES:
        raise GateError(stage, "output-limit", stdout_bytes=len(result.stdout))
    return result


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "architecture": "aarch64",
        "rootfs_mode_before": "rw",
        "tmp_mount_mode": "exec",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "htpasswd_ready": "yes",
        "account_absent": "yes",
        "systemd_ready": "yes",
        "tools_ready": "yes",
        "stage_absent": "yes",
        "unit_names_free": "yes",
        "port_free_before": "yes",
        "ustreamer_socket": "present",
        "ustreamer_socket_mode": "660",
        "ustreamer_socket_group": "kvmd",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("systemd preflight invariant failed")


def validate_run(
    facts: dict[str, str], bundle_sha256: str, thermal_only: bool = False
) -> None:
    if thermal_only:
        video_status = "skipped"
        video_content_type = "skipped"
        video_frames = "skipped"
        video_reconnects = "skipped"
        video_session_revocation = "skipped"
    else:
        video_status = "200"
        video_content_type = "multipart-mjpeg"
        video_frames = facts.get("video_frames", "")
        if not video_frames.isdecimal() or int(video_frames) < 2:
            raise ValueError("systemd video frame invariant failed")
        video_reconnects = str(VIDEO_RECONNECTS)
        video_session_revocation = "passed"
    thermal = facts.get("thermal_millidegrees", "")
    if not re.fullmatch(r"-?[0-9]+", thermal) or not -40_000 <= int(thermal) <= 150_000:
        raise ValueError("systemd thermal invariant failed")
    rtc_unix = facts.get("rtc_unix_seconds", "")
    rtc_offset = facts.get("rtc_offset_seconds", "")
    if (
        not rtc_unix.isdecimal()
        or not 946_684_800 <= int(rtc_unix) <= 4_102_444_799
        or not re.fullmatch(r"-?[0-9]+", rtc_offset)
        or not -315_576_000 <= int(rtc_offset) <= 315_576_000
    ):
        raise ValueError("systemd RTC invariant failed")
    expected = {
        "bundle_sha256": bundle_sha256,
        "payload_checksums": "verified",
        "runtime_profile": "thermal-only" if thermal_only else "full",
        "dedicated_credential": "verified",
        "account_created": "yes",
        "watchdog_armed": "yes",
        "broker_unit": "passed",
        "broker_load_credential": "verified",
        "broker_network_namespace": "private",
        "broker_caps": "none",
        "broker_socket_mode": "660",
        "video_unit": "passed",
        "video_network_namespace": "private",
        "video_caps": "none",
        "video_no_device_access": "verified",
        "video_socket_mode": "660",
        "server_unit": "passed",
        "server_host_network": "loopback-restricted",
        "server_no_credential": "verified",
        "server_no_kvmd_group": "verified",
        "server_caps": "none",
        "no_new_privs": "yes",
        "setup_status": "200",
        "login_status": "200",
        "capabilities_status": "200",
        "inventory_status": "200",
        "inventory_schema": "3",
        "inventory_backend": "kvmd_transition",
        "inventory_sanitized": "yes",
        "inventory_requests": str(INVENTORY_REQUESTS),
        "thermal_millidegrees": thermal,
        "thermal_capability": "read_only",
        "thermal_evidence": "runtime_verified",
        "rtc_unix_seconds": rtc_unix,
        "rtc_offset_seconds": rtc_offset,
        "rtc_capability": "read_only",
        "rtc_evidence": "source_verified",
        "video_status": video_status,
        "video_content_type": video_content_type,
        "video_frames": video_frames,
        "video_reconnects": video_reconnects,
        "video_session_revocation": video_session_revocation,
        "htpasswd_restored": "yes",
        "account_removed": "yes",
        "units_removed": "yes",
        "staging_removed": "yes",
        "kvmd_active_after_run": "active",
        "kvmd_nginx_active_after_run": "active",
        "rootfs_mode_after_run": "rw",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("systemd runtime invariant failed")


def validate_final(facts: dict[str, str]) -> None:
    expected = {
        "stage_absent_final": "yes",
        "units_absent_final": "yes",
        "runtime_absent_final": "yes",
        "watchdog_inactive_final": "yes",
        "account_absent_final": "yes",
        "port_free_final": "yes",
        "rootfs_mode_final": "rw",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("systemd final-state invariant failed")


def policy_receipt(thermal_only: bool = False) -> PolicyReceipt:
    return {
        "accountMutation": "temporary-dedicated-kvmd-account",
        "accountRollback": "byte-identical-htpasswd-restore",
        "unitInstallation": "runtime-only-with-test-overrides",
        "watchdog": "ten-minute-systemd-rollback",
        "physicalActionsInvoked": False,
        "kvmdRestarted": False,
        "rootfsHelpersInvoked": False,
        "persistentChangeScope": "temporary-and-rollback-required",
        "runtimeProfile": "thermal-only" if thermal_only else "full",
    }


def cleanup_receipt(
    target: prototype.RemoteTarget, secret: bytearray, paths: GatePaths
) -> CleanupReceipt:
    try:
        result = prototype.run_ssh(
            target, secret, build_cleanup_script(paths), timeout=40
        )
        if result.returncode != 0:
            return {"attempted": True, "confirmed": False}
        facts = prototype.parse_facts(result.stdout, CLEANUP_KEYS)
        return {
            "attempted": True,
            "confirmed": facts == {"cleanup_restored": "yes"},
        }
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
    paths = gate_paths(bundle.sha256, secrets.token_hex(8))
    required_bytes = bundle.size_bytes * 5 + prototype.MIN_TMP_HEADROOM_BYTES
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    stage = "initialization"
    probe = (
        "hardened-blikvm-reversible-thermal-systemd-gate"
        if options.thermal_only
        else "hardened-blikvm-reversible-systemd-gate"
    )
    try:
        stage = "preflight"
        preflight_result = require_result(
            stage,
            prototype.run_ssh(
                target,
                secret,
                build_preflight_script(paths, required_bytes, bind_port),
                timeout=25,
            ),
        )
        preflight = prototype.parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        validate_preflight(preflight)

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

        stage = "runtime"
        run_result = require_result(
            stage,
            prototype.run_ssh(
                target,
                secret,
                build_runtime_script(paths, bundle, bind_port, options.thermal_only),
                timeout=300,
            ),
        )
        checks = prototype.parse_facts(run_result.stdout, RUN_KEYS)
        validate_run(checks, bundle.sha256, options.thermal_only)

        stage = "final-state"
        final_result = require_result(
            stage,
            prototype.run_ssh(
                target, secret, build_final_script(paths, bind_port), timeout=25
            ),
        )
        final_state = prototype.parse_facts(final_result.stdout, FINAL_KEYS)
        validate_final(final_state)
        cleanup_required = False

        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": probe,
            "result": "passed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(options.thermal_only),
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except GateError as error:
        cleanup = (
            cleanup_receipt(target, secret, paths)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": probe,
            "result": "failed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(options.thermal_only),
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
            "probe": probe,
            "result": "failed",
            "bundle": {
                "sha256": bundle.sha256,
                "sizeBytes": bundle.size_bytes,
                "sourceCommit": bundle.source_commit,
            },
            "policy": policy_receipt(options.thermal_only),
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
