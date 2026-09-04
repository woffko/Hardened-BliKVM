#!/usr/bin/env python3
"""Guard and verify one software reboot of the persistent Hardened H616 UI."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
from pathlib import Path
import re
import secrets
import shlex
import socket
import subprocess
import sys
import time
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


GUARD_DELAY_SECONDS = 240
STATE_ROOT = "/var/lib/hardened-blikvm"
PRE_KEYS = {
    "account_sha256",
    "boot_id_sha256",
    "control_boundary_absent",
    "kvmd_active",
    "ports_exact",
    "rootfs_mode",
    "setup_closed",
    "units_persistent",
    "vendor_nginx_disabled",
}
PREPARE_KEYS = {"guard_armed", "guard_delay_seconds", "guard_synced"}
POST_KEYS = {
    "account_unchanged",
    "boot_id_changed",
    "control_boundary_absent",
    "guard_armed",
    "kvmd_active",
    "ports_exact",
    "setup_closed",
    "tls12",
    "tls13",
    "units_persistent",
    "vendor_nginx_disabled",
}
FINAL_KEYS = {
    "account_unchanged",
    "control_boundary_absent",
    "guard_removed",
    "kvmd_active",
    "ports_exact",
    "setup_closed",
    "units_persistent",
    "vendor_nginx_disabled",
}
TOKEN_VALUES = {"active", "changed", "disabled", "passed", "rw", "yes"}


@dataclass(frozen=True)
class Paths:
    run_id: str
    root: str
    marker: str
    guard_script: str
    service: str
    service_path: str
    service_part: str
    timer: str
    timer_path: str
    timer_part: str
    timer_link: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    baseline: str
    output: str


class RebootReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    preflight: dict[str, str]
    prepared: dict[str, str]
    transport: dict[str, object]
    postBoot: dict[str, str]
    finalState: dict[str, str]
    policy: dict[str, object]


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        baseline=cast(str, namespace.baseline),
        output=cast(str, namespace.output),
    )


def paths(run_id: str) -> Paths:
    if not re.fullmatch(r"[0-9a-f]{16}", run_id):
        raise ValueError("reboot run identifier is invalid")
    stem = f"hardened-blikvm-reboot-guard-{run_id}"
    root = f"/var/lib/{stem}"
    service = f"{stem}.service"
    timer = f"{stem}.timer"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.owner",
        guard_script=f"{root}/guard.sh",
        service=service,
        service_path=f"/etc/systemd/system/{service}",
        service_part=f"/etc/systemd/system/.{service}.part",
        timer=timer,
        timer_path=f"/etc/systemd/system/{timer}",
        timer_part=f"/etc/systemd/system/.{timer}.part",
        timer_link=f"/etc/systemd/system/timers.target.wants/{timer}",
    )


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def _encoded(value: str) -> str:
    return base64.b64encode(value.encode()).decode("ascii")


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def parse_facts(raw: bytes, expected: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("reboot gate output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in expected or key in facts:
            raise ValueError("reboot gate returned an unexpected field")
        if value not in TOKEN_VALUES and not re.fullmatch(r"[0-9a-f]{64}|[0-9]{1,5}", value):
            raise ValueError("reboot gate returned an unsafe value")
        facts[key] = value
    if set(facts) != expected:
        raise ValueError("reboot gate result was incomplete")
    return dict(sorted(facts.items()))


REMOTE_HELPERS = rf"""
{cutover.REMOTE_HELPERS}
account_state_valid() {{
  server_uid=$(getent passwd hardened-blikvm-server | awk -F: '{{print $3}}')
  shared_gid=$(getent group hardened-blikvm | awk -F: '{{print $3}}')
  [ -f "$state_root/account.json" ] && [ ! -L "$state_root/account.json" ]
  [ "$(stat -c '%a:%u:%g:%h' "$state_root/account.json")" = "600:$server_uid:$shared_gid:1" ]
  [ "$(stat -c '%s' "$state_root/account.json")" -le 4096 ]
}}
setup_closed() {{
  account_state_valid
  response=$(curl --noproxy '*' --insecure --silent --max-time 5 https://127.0.0.1/api/auth/setup)
  case "$response" in *'"required":false'*) return 0 ;; *) return 1 ;; esac
}}
persistent_state() {{
  product_active_enabled &&
  [ "$(systemctl is-active hardened-blikvm-nginx.service 2>/dev/null || true)" = active ] &&
  [ "$(service_enabled hardened-blikvm-nginx.service)" = enabled ] &&
  [ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ] &&
  [ "$(service_enabled kvmd-nginx.service)" = disabled ] &&
  [ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ] &&
  control_boundary_absent && ports_exact && setup_closed &&
  curl --noproxy '*' --insecure --silent --fail --max-time 5 --output /dev/null https://127.0.0.1/api/health
}}
""".strip()


def _path_values(paths_value: Paths) -> dict[str, str]:
    return {
        "guard_root": paths_value.root,
        "marker": paths_value.marker,
        "guard_script": paths_value.guard_script,
        "guard_service": paths_value.service,
        "guard_service_path": paths_value.service_path,
        "guard_service_part": paths_value.service_part,
        "guard_timer": paths_value.timer,
        "guard_timer_path": paths_value.timer_path,
        "guard_timer_part": paths_value.timer_part,
        "guard_timer_link": paths_value.timer_link,
        "state_root": STATE_ROOT,
    }


def build_preflight_script(
    paths_value: Paths, baseline: Mapping[str, str]
) -> str:
    values = {
        **_path_values(paths_value),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) ;; *) exit 1 ;; esac
persistent_state
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
[ ! -e "$guard_root" ] && [ ! -e "$guard_service_path" ] && [ ! -e "$guard_service_part" ]
[ ! -e "$guard_timer_path" ] && [ ! -e "$guard_timer_part" ] && [ ! -e "$guard_timer_link" ]
account_hash=$(sha256sum "$state_root/account.json" | awk '{print $1}')
boot_hash=$(sha256sum /proc/sys/kernel/random/boot_id | awk '{print $1}')
emit account_sha256 "$account_hash"
emit boot_id_sha256 "$boot_hash"
emit units_persistent yes
emit vendor_nginx_disabled yes
emit ports_exact yes
emit setup_closed yes
emit kvmd_active active
emit control_boundary_absent yes
emit rootfs_mode rw
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_guard_service(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM reboot confirmation guard",
            "After=network-online.target",
            "Wants=network-online.target",
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
            f"ExecStart=/bin/sh {paths_value.guard_script} timer",
            "TimeoutStartSec=5min",
            "",
        )
    )


def build_guard_timer(paths_value: Paths) -> str:
    return "\n".join(
        (
            "[Unit]",
            "Description=Hardened BliKVM reboot confirmation deadline",
            f"ConditionPathExists={paths_value.marker}",
            "",
            "[Timer]",
            f"OnActiveSec={GUARD_DELAY_SECONDS}s",
            "AccuracySec=1s",
            f"Unit={paths_value.service}",
            "",
            "[Install]",
            "WantedBy=timers.target",
            "",
        )
    )


def build_guard_script(
    paths_value: Paths,
    account_sha256: str,
    baseline: Mapping[str, str],
) -> str:
    service = build_guard_service(paths_value)
    timer = build_guard_timer(paths_value)
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{account_sha256}",
        "account_sha256": account_sha256,
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
        "service_sha256": _digest(service),
        "timer_sha256": _digest(timer),
    }
    body = r"""
set -eu
[ "$#" = 1 ] && [ "$1" = timer ]
[ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]
healthy=no
if persistent_state &&
   [ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ] &&
   [ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ] &&
   [ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]; then
  healthy=yes
fi
if [ "$healthy" != yes ]; then
  systemctl disable --now hardened-blikvm-nginx.service >/dev/null 2>&1 || true
  systemctl enable kvmd-nginx.service >/dev/null 2>&1
  systemctl start kvmd-nginx.service >/dev/null 2>&1
fi
systemctl disable "$guard_timer" >/dev/null 2>&1 || true
rm -f -- "$guard_timer_link"
[ -f "$guard_service_path" ] && [ "$(sha256sum "$guard_service_path" | awk '{print $1}')" = "$service_sha256" ]
[ -f "$guard_timer_path" ] && [ "$(sha256sum "$guard_timer_path" | awk '{print $1}')" = "$timer_sha256" ]
rm -f -- "$guard_service_path" "$guard_timer_path" "$marker" "$guard_script"
systemctl daemon-reload >/dev/null 2>&1
rmdir "$guard_root"
""".strip()
    return f"#!/bin/sh\n{_assignments(values)}\n{REMOTE_HELPERS}\n{body}\n"


def build_prepare_script(
    paths_value: Paths,
    account_sha256: str,
    baseline: Mapping[str, str],
) -> str:
    service = build_guard_service(paths_value)
    timer = build_guard_timer(paths_value)
    guard = build_guard_script(paths_value, account_sha256, baseline)
    values = {
        **_path_values(paths_value),
        "marker_value": f"{paths_value.run_id}:{account_sha256}",
        "guard_b64": _encoded(guard),
        "guard_sha256": _digest(guard),
        "service_b64": _encoded(service),
        "service_sha256": _digest(service),
        "timer_b64": _encoded(timer),
        "timer_sha256": _digest(timer),
    }
    body = rf"""
set -eu
umask 077
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
[ ! -e "$guard_root" ]
mkdir -m 700 "$guard_root"
printf '%s\n' "$marker_value" > "$marker"
printf '%s' "$guard_b64" | base64 -d > "$guard_script"
printf '%s' "$service_b64" | base64 -d > "$guard_service_part"
printf '%s' "$timer_b64" | base64 -d > "$guard_timer_part"
[ "$(sha256sum "$guard_script" | awk '{{print $1}}')" = "$guard_sha256" ]
[ "$(sha256sum "$guard_service_part" | awk '{{print $1}}')" = "$service_sha256" ]
[ "$(sha256sum "$guard_timer_part" | awk '{{print $1}}')" = "$timer_sha256" ]
chmod 600 "$marker"
chmod 700 "$guard_script"
chmod 644 "$guard_service_part" "$guard_timer_part"
mv "$guard_service_part" "$guard_service_path"
mv "$guard_timer_part" "$guard_timer_path"
sync
systemctl daemon-reload
systemctl enable --now "$guard_timer" >/dev/null
[ "$(systemctl is-active "$guard_timer" 2>/dev/null || true)" = active ]
[ "$(service_enabled "$guard_timer")" = enabled ]
sync
emit guard_armed yes
emit guard_delay_seconds {GUARD_DELAY_SECONDS}
emit guard_synced yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_postboot_script(
    paths_value: Paths,
    prior_boot_sha256: str,
    account_sha256: str,
    baseline: Mapping[str, str],
) -> str:
    values = {
        **_path_values(paths_value),
        "prior_boot_sha256": prior_boot_sha256,
        "account_sha256": account_sha256,
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
boot_hash=$(sha256sum /proc/sys/kernel/random/boot_id | awk '{print $1}')
[ "$boot_hash" != "$prior_boot_sha256" ]
persistent_state
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
[ -f "$marker" ] && [ -f "$guard_script" ]
[ "$(systemctl is-active "$guard_timer" 2>/dev/null || true)" = active ]
[ "$(service_enabled "$guard_timer")" = enabled ]
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1/api/health
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1/api/health
emit boot_id_changed changed
emit account_unchanged yes
emit guard_armed yes
emit units_persistent yes
emit vendor_nginx_disabled yes
emit ports_exact yes
emit setup_closed yes
emit tls12 passed
emit tls13 passed
emit kvmd_active active
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def build_disarm_script(
    paths_value: Paths,
    account_sha256: str,
    baseline: Mapping[str, str],
) -> str:
    service = build_guard_service(paths_value)
    timer = build_guard_timer(paths_value)
    values = {
        **_path_values(paths_value),
        "account_sha256": account_sha256,
        "service_sha256": _digest(service),
        "timer_sha256": _digest(timer),
        "certificate_path": baseline["certificate_path"],
    }
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
persistent_state
[ "$(sha256sum "$state_root/account.json" | awk '{print $1}')" = "$account_sha256" ]
systemctl disable --now "$guard_timer" >/dev/null 2>&1
systemctl reset-failed "$guard_timer" "$guard_service" >/dev/null 2>&1 || true
[ -f "$guard_service_path" ] && [ "$(sha256sum "$guard_service_path" | awk '{print $1}')" = "$service_sha256" ]
[ -f "$guard_timer_path" ] && [ "$(sha256sum "$guard_timer_path" | awk '{print $1}')" = "$timer_sha256" ]
rm -f -- "$guard_timer_link" "$guard_service_path" "$guard_timer_path" "$marker" "$guard_script"
systemctl daemon-reload
rmdir "$guard_root"
persistent_state
[ ! -e "$guard_root" ] && [ ! -e "$guard_service_path" ] && [ ! -e "$guard_timer_path" ] && [ ! -e "$guard_timer_link" ]
emit account_unchanged yes
emit guard_removed yes
emit units_persistent yes
emit vendor_nginx_disabled yes
emit ports_exact yes
emit setup_closed yes
emit kvmd_active active
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{REMOTE_HELPERS}\n{body}"


def tcp_reachable(host: str, port: int, timeout: float = 1.0) -> bool:
    try:
        with socket.create_connection((host, port), timeout=timeout):
            return True
    except OSError:
        return False


def wait_for_transition(
    host: str, ssh_port: int, *, down_timeout: float, up_timeout: float
) -> tuple[float, float]:
    start = time.monotonic()
    deadline = start + down_timeout
    while time.monotonic() < deadline:
        if not tcp_reachable(host, ssh_port) and not tcp_reachable(host, 443):
            down_at = time.monotonic()
            break
        time.sleep(1)
    else:
        raise ValueError("device did not become unreachable after reboot")
    deadline = down_at + up_timeout
    while time.monotonic() < deadline:
        if tcp_reachable(host, ssh_port):
            return down_at - start, time.monotonic() - down_at
        time.sleep(2)
    raise ValueError("device did not restore SSH after reboot")


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    baseline = cutover.load_baseline(options.baseline)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths_value = paths(secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    guard_armed = False
    try:
        result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(paths_value, baseline),
            timeout=60,
        )
        if result.returncode != 0:
            raise ValueError("reboot preflight failed")
        preflight = parse_facts(result.stdout, PRE_KEYS)
        stage = "prepare-guard"
        result = prototype.run_ssh(
            target,
            secret,
            build_prepare_script(
                paths_value, preflight["account_sha256"], baseline
            ),
            timeout=60,
        )
        if result.returncode != 0:
            raise ValueError("reboot guard prepare failed")
        prepared = parse_facts(result.stdout, PREPARE_KEYS)
        guard_armed = True
        stage = "reboot-request"
        reboot_result = prototype.run_ssh(
            target, secret, "systemctl reboot", timeout=20
        )
        if reboot_result.returncode not in {0, 255}:
            raise ValueError("software reboot request was rejected")
        stage = "network-transition"
        down_seconds, up_seconds = wait_for_transition(
            host, ssh_port, down_timeout=45, up_timeout=240
        )
        stage = "postboot"
        post_result = None
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            candidate = prototype.run_ssh(
                target,
                secret,
                build_postboot_script(
                    paths_value,
                    preflight["boot_id_sha256"],
                    preflight["account_sha256"],
                    baseline,
                ),
                timeout=30,
            )
            if candidate.returncode == 0:
                post_result = candidate
                break
            time.sleep(3)
        if post_result is None:
            raise ValueError("postboot state did not become ready")
        postboot = parse_facts(post_result.stdout, POST_KEYS)
        stage = "disarm-guard"
        result = prototype.run_ssh(
            target,
            secret,
            build_disarm_script(
                paths_value, preflight["account_sha256"], baseline
            ),
            timeout=60,
        )
        if result.returncode != 0:
            raise ValueError("reboot guard disarm failed")
        final_state = parse_facts(result.stdout, FINAL_KEYS)
        guard_armed = False
        receipt: RebootReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-persistent-software-reboot",
            "result": "passed",
            "preflight": preflight,
            "prepared": prepared,
            "transport": {
                "sshAndHttpsDisappeared": True,
                "sshReturned": True,
                "downDetectedSeconds": round(down_seconds, 1),
                "sshRecoverySeconds": round(up_seconds, 1),
            },
            "postBoot": postboot,
            "finalState": final_state,
            "policy": {
                "softwareRebootOfTestDevice": True,
                "targetPowerControl": False,
                "atxPowerReset": False,
                "accountChanged": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        write_json_0600_atomic(
            output,
            {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-persistent-software-reboot",
                "result": "failed",
                "failure": {
                    "stage": stage,
                    "failureType": type(error).__name__,
                    "guardArmed": guard_armed,
                },
                "policy": {
                    "softwareRebootOfTestDevice": stage not in {"preflight", "prepare-guard"},
                    "targetPowerControl": False,
                    "atxPowerReset": False,
                    "secretEmitted": False,
                },
            },
        )
        return 1
    finally:
        _zero(secret)


if __name__ == "__main__":
    raise SystemExit(main())
