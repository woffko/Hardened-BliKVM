#!/usr/bin/env python3
"""Capture bounded, read-only facts needed to choose the H616 package format."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


SAFE_KEYS = {
    "architecture",
    "os_id",
    "os_codename",
    "rootfs_mode",
    "dpkg",
    "dpkg_deb",
    "systemd_sysusers",
    "systemd_tmpfiles",
    "nginx",
    "openssl",
    "package_state",
    "libexec_root",
    "web_root",
    "config_root",
    "state_root",
    "slot_root",
    "update_state_root",
    "main_unit_file",
    "broker_unit_file",
    "video_unit_file",
    "control_unit_file",
    "server_account",
    "broker_account",
    "video_account",
    "control_account",
    "update_account",
    "runtime_group",
    "server_active",
    "broker_active",
    "video_active",
    "control_active",
    "server_enabled",
    "broker_enabled",
    "video_enabled",
    "control_enabled",
    "port_18080",
    "port_18443",
    "nginx_sites_enabled",
    "nginx_conf_d",
    "kvmd_htpasswd",
    "kvmd_htpasswd_mode",
    "usr_available_bytes",
    "var_available_bytes",
}

ENUMS: dict[str, set[str]] = {
    "architecture": {"aarch64", "other"},
    "os_id": {"ubuntu", "debian", "armbian", "other"},
    "os_codename": {"jammy", "bookworm", "bullseye", "other", "unknown"},
    "rootfs_mode": {"ro", "rw"},
    "dpkg": {"available", "missing"},
    "dpkg_deb": {"available", "missing"},
    "systemd_sysusers": {"available", "missing"},
    "systemd_tmpfiles": {"available", "missing"},
    "nginx": {"available", "missing"},
    "openssl": {"available", "missing"},
    "package_state": {"absent", "installed"},
    "libexec_root": {"absent", "directory", "other"},
    "web_root": {"absent", "directory", "other"},
    "config_root": {"absent", "directory", "other"},
    "state_root": {"absent", "directory", "other"},
    "slot_root": {"absent", "directory", "other"},
    "update_state_root": {"absent", "directory", "other"},
    "main_unit_file": {"absent", "present"},
    "broker_unit_file": {"absent", "present"},
    "video_unit_file": {"absent", "present"},
    "control_unit_file": {"absent", "present"},
    "server_account": {"absent", "present"},
    "broker_account": {"absent", "present"},
    "video_account": {"absent", "present"},
    "control_account": {"absent", "present"},
    "update_account": {"absent", "present"},
    "runtime_group": {"absent", "present"},
    "server_active": {"active", "inactive", "failed", "not_found"},
    "broker_active": {"active", "inactive", "failed", "not_found"},
    "video_active": {"active", "inactive", "failed", "not_found"},
    "control_active": {"active", "inactive", "failed", "not_found"},
    "server_enabled": {"enabled", "disabled", "static", "not_found"},
    "broker_enabled": {"enabled", "disabled", "static", "not_found"},
    "video_enabled": {"enabled", "disabled", "static", "not_found"},
    "control_enabled": {"enabled", "disabled", "static", "not_found"},
    "port_18080": {"free", "listening"},
    "port_18443": {"free", "listening"},
    "nginx_sites_enabled": {"absent", "directory", "other"},
    "nginx_conf_d": {"absent", "directory", "other"},
    "kvmd_htpasswd": {"absent", "present"},
    "kvmd_htpasswd_mode": {"unavailable", "600", "640", "644", "other"},
}

INTEGER_KEYS = {"usr_available_bytes", "var_available_bytes"}

REMOTE_PROBE = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
available() { if command -v "$1" >/dev/null 2>&1; then emit "$2" available; else emit "$2" missing; fi; }
path_state() { if [ ! -e "$1" ]; then emit "$2" absent; elif [ -d "$1" ] && [ ! -L "$1" ]; then emit "$2" directory; else emit "$2" other; fi; }
account_state() { if getent passwd "$1" >/dev/null 2>&1; then emit "$2" present; else emit "$2" absent; fi; }
group_state() { if getent group "$1" >/dev/null 2>&1; then emit "$2" present; else emit "$2" absent; fi; }
unit_file_state() {
  found=no
  for root in /etc/systemd/system /lib/systemd/system /usr/lib/systemd/system; do
    if [ -e "$root/$1" ]; then found=yes; fi
  done
  if [ "$found" = yes ]; then emit "$2" present; else emit "$2" absent; fi
}
unit_active() {
  state=$(systemctl is-active "$1" 2>/dev/null || true)
  case "$state" in active|inactive|failed) emit "$2" "$state" ;; *) emit "$2" not_found ;; esac
}
unit_enabled() {
  state=$(systemctl is-enabled "$1" 2>/dev/null || true)
  case "$state" in enabled|disabled|static) emit "$2" "$state" ;; *) emit "$2" not_found ;; esac
}
port_state() {
  if awk -v wanted="$1" '$2 ~ (":" wanted "$") && $4 == "0A" {found=1} END {exit !found}' /proc/net/tcp /proc/net/tcp6 2>/dev/null; then
    emit "$2" listening
  else
    emit "$2" free
  fi
}
[ "$(id -u)" = 0 ]
case "$(uname -m)" in aarch64) emit architecture aarch64 ;; *) emit architecture other ;; esac
. /etc/os-release
case "${ID:-}" in ubuntu|debian|armbian) emit os_id "$ID" ;; *) emit os_id other ;; esac
codename=${VERSION_CODENAME:-unknown}
case "$codename" in jammy|bookworm|bullseye) emit os_codename "$codename" ;; unknown|'') emit os_codename unknown ;; *) emit os_codename other ;; esac
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) emit rootfs_mode rw ;; *) emit rootfs_mode ro ;; esac
available dpkg dpkg
available dpkg-deb dpkg_deb
available systemd-sysusers systemd_sysusers
available systemd-tmpfiles systemd_tmpfiles
available nginx nginx
available openssl openssl
if command -v dpkg-query >/dev/null 2>&1 && dpkg-query -W -f='${db:Status-Abbrev}' hardened-blikvm 2>/dev/null | grep -q '^ii '; then emit package_state installed; else emit package_state absent; fi
path_state /usr/libexec/hardened-blikvm libexec_root
path_state /usr/share/hardened-blikvm/web web_root
path_state /etc/hardened-blikvm config_root
path_state /var/lib/hardened-blikvm state_root
path_state /var/lib/hardened-blikvm-slots slot_root
path_state /var/lib/hardened-blikvm-update update_state_root
unit_file_state hardened-blikvm-server.service main_unit_file
unit_file_state hardened-blikvm-platform-broker.service broker_unit_file
unit_file_state hardened-blikvm-video-proxy.service video_unit_file
unit_file_state hardened-blikvm-control-broker.service control_unit_file
account_state hardened-blikvm-server server_account
account_state hardened-blikvm-broker broker_account
account_state hardened-blikvm-video video_account
account_state hardened-blikvm-control control_account
account_state hardened-blikvm-update update_account
group_state hardened-blikvm runtime_group
unit_active hardened-blikvm-server.service server_active
unit_active hardened-blikvm-platform-broker.service broker_active
unit_active hardened-blikvm-video-proxy.service video_active
unit_active hardened-blikvm-control-broker.service control_active
unit_enabled hardened-blikvm-server.service server_enabled
unit_enabled hardened-blikvm-platform-broker.service broker_enabled
unit_enabled hardened-blikvm-video-proxy.service video_enabled
unit_enabled hardened-blikvm-control-broker.service control_enabled
port_state 46A0 port_18080
port_state 480B port_18443
path_state /etc/nginx/sites-enabled nginx_sites_enabled
path_state /etc/nginx/conf.d nginx_conf_d
if [ -f /etc/kvmd/htpasswd ] && [ ! -L /etc/kvmd/htpasswd ]; then
  emit kvmd_htpasswd present
  mode=$(stat -c '%a' /etc/kvmd/htpasswd)
  case "$mode" in 600|640|644) emit kvmd_htpasswd_mode "$mode" ;; *) emit kvmd_htpasswd_mode other ;; esac
else
  emit kvmd_htpasswd absent
  emit kvmd_htpasswd_mode unavailable
fi
emit usr_available_bytes "$(df -B1 --output=avail /usr | tail -n 1 | tr -d ' ')"
emit var_available_bytes "$(df -B1 --output=avail /var | tail -n 1 | tr -d ' ')"
""".strip()


class Failure(TypedDict):
    stage: str
    returnCode: int | None


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", default="evidence/private/ssh-known-hosts")
    _ = parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
    )


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("packaging preflight output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in SAFE_KEYS or key in facts:
            raise ValueError("packaging preflight returned an unexpected field")
        if key in INTEGER_KEYS:
            if not re.fullmatch(r"[1-9][0-9]{0,19}", value):
                raise ValueError("packaging preflight returned an invalid size")
        elif value not in ENUMS[key]:
            raise ValueError("packaging preflight returned an invalid enum")
        facts[key] = value
    if set(facts) != SAFE_KEYS:
        raise ValueError("packaging preflight result was incomplete")
    return dict(sorted(facts.items()))


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=30)
        if result.returncode != 0:
            receipt: dict[str, object] = {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-packaging-preflight",
                "result": "failed",
                "failure": {"stage": "remote", "returnCode": result.returncode},
            }
            write_json_0600_atomic(output, receipt)
            return 1
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-packaging-preflight",
            "result": "passed",
            "facts": parse_facts(result.stdout),
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, UnicodeError, ValueError) as error:
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-packaging-preflight",
            "result": "failed",
            "failure": {"stage": type(error).__name__, "returnCode": None},
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
