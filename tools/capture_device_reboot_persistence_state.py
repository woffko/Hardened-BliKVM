#!/usr/bin/env python3
"""Capture a bounded read-only audit after a reboot persistence fallback."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


FACT_KEYS = {
    "account_state",
    "activation_rollback_seen",
    "broker_state",
    "config_credentials",
    "control_boundary",
    "cutover_rollback_seen",
    "etc_fstype",
    "etc_same_root_fs",
    "hardened_config",
    "hardened_nginx_state",
    "hardened_unit_file",
    "identity_set",
    "installed_tls_rollback_seen",
    "journal_hardened_lines",
    "package_state",
    "reboot_guard_artifacts",
    "reboot_guard_seen",
    "root_fstype",
    "rootfs_mode",
    "runtime_activation_rollback_seen",
    "server_state",
    "state_cleanup_seen",
    "systemd_failure_seen",
    "var_lib_fstype",
    "var_lib_same_root_fs",
    "vendor_nginx_state",
    "video_state",
    "web_cutover_artifacts",
    "web_cutover_boot_links",
    "web_cutover_boot_units",
    "web_cutover_roots",
}
SAFE_VALUE = re.compile(r"^[a-z0-9_.:+-]{1,96}$")


REMOTE_PROBE = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
unit_state() {
  active=$(systemctl is-active "$1" 2>/dev/null || true)
  enabled=$(systemctl is-enabled "$1" 2>/dev/null || true)
  case "$active:$enabled" in
    active:enabled|inactive:disabled|failed:enabled|failed:disabled) printf '%s:%s' "$active" "$enabled" ;;
    *) printf 'other' ;;
  esac
}
flag() {
  name=$1 pattern=$2
  if printf '%s\n' "$journal" | grep -Eqi "$pattern"; then emit "$name" yes; else emit "$name" no; fi
}
for tool in awk dpkg-query find findmnt getent grep journalctl sha256sum stat systemctl wc; do
  command -v "$tool" >/dev/null 2>&1
done
[ "$(id -u)" = 0 ]
root_fstype=$(findmnt -n -T / -o FSTYPE)
etc_fstype=$(findmnt -n -T /etc -o FSTYPE)
var_lib_fstype=$(findmnt -n -T /var/lib -o FSTYPE)
case "$root_fstype" in ext4|overlay|tmpfs) ;; *) root_fstype=other ;; esac
case "$etc_fstype" in ext4|overlay|tmpfs) ;; *) etc_fstype=other ;; esac
case "$var_lib_fstype" in ext4|overlay|tmpfs) ;; *) var_lib_fstype=other ;; esac
root_dev=$(stat -c '%d' /)
[ "$(stat -c '%d' /etc)" = "$root_dev" ] && etc_same=yes || etc_same=no
[ "$(stat -c '%d' /var/lib)" = "$root_dev" ] && var_lib_same=yes || var_lib_same=no
root_options=$(findmnt -n -T / -o OPTIONS)
case ",$root_options," in *,rw,*) root_mode=rw ;; *) root_mode=ro ;; esac
package_status=$(dpkg-query -W -f='${db:Status-Abbrev}:${Architecture}' hardened-blikvm-runtime 2>/dev/null || true)
[ "$package_status" = 'ii :arm64' ] && package_state=installed || package_state=other
identities=yes
for name in hardened-blikvm-broker hardened-blikvm-video hardened-blikvm-control hardened-blikvm-server hardened-blikvm-update; do
  getent passwd "$name" >/dev/null 2>&1 || identities=no
done
if [ -f /etc/hardened-blikvm/kvmd-user ] && [ -f /etc/hardened-blikvm/kvmd-password ]; then credentials=present; else credentials=absent; fi
if [ -f /etc/hardened-blikvm/nginx.conf ]; then hardened_config=present; else hardened_config=absent; fi
if [ -f /etc/systemd/system/hardened-blikvm-nginx.service ]; then hardened_unit=present; else hardened_unit=absent; fi
if [ -f /var/lib/hardened-blikvm/account.json ]; then account_state=present; else account_state=absent; fi
if [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ] &&
   [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]; then control=absent; else control=present; fi
if find /var/lib /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-reboot-guard-*' -print -quit 2>/dev/null | grep -q .; then guard_artifacts=present; else guard_artifacts=absent; fi
web_roots=$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-web-cutover-*' -print 2>/dev/null | wc -l)
web_units=$(find /etc/systemd/system -maxdepth 1 -type f -name 'hardened-blikvm-web-cutover-*-rollback.service' -print 2>/dev/null | wc -l)
web_links=$(find /etc/systemd/system/sysinit.target.wants -maxdepth 1 -type l -name 'hardened-blikvm-web-cutover-*-rollback.service' -print 2>/dev/null | wc -l)
if [ "$web_roots" = 0 ] && [ "$web_units" = 0 ] && [ "$web_links" = 0 ]; then web_artifacts=absent; else web_artifacts=present; fi
journal=$(journalctl -b 0 --no-pager -o cat 2>/dev/null | grep -i 'hardened blikvm\|hardened-blikvm' || true)
lines=$(printf '%s\n' "$journal" | awk 'NF { count++ } END { print count + 0 }')
[ "$lines" -le 9999 ]
emit root_fstype "$root_fstype"
emit etc_fstype "$etc_fstype"
emit var_lib_fstype "$var_lib_fstype"
emit etc_same_root_fs "$etc_same"
emit var_lib_same_root_fs "$var_lib_same"
emit rootfs_mode "$root_mode"
emit package_state "$package_state"
emit identity_set "$identities"
emit config_credentials "$credentials"
emit hardened_config "$hardened_config"
emit hardened_unit_file "$hardened_unit"
emit account_state "$account_state"
emit broker_state "$(unit_state hardened-blikvm-platform-broker.service)"
emit video_state "$(unit_state hardened-blikvm-video-proxy.service)"
emit server_state "$(unit_state hardened-blikvm-server.service)"
emit hardened_nginx_state "$(unit_state hardened-blikvm-nginx.service)"
emit vendor_nginx_state "$(unit_state kvmd-nginx.service)"
emit control_boundary "$control"
emit reboot_guard_artifacts "$guard_artifacts"
emit web_cutover_roots "$web_roots"
emit web_cutover_boot_units "$web_units"
emit web_cutover_boot_links "$web_links"
emit web_cutover_artifacts "$web_artifacts"
emit journal_hardened_lines "$lines"
flag activation_rollback_seen 'activation emergency rollback'
flag runtime_activation_rollback_seen 'activation-rollback|activation rollback'
flag installed_tls_rollback_seen 'installed TLS gate rollback|installed-tls.*rollback'
flag cutover_rollback_seen 'web cutover emergency rollback|web-cutover.*rollback'
flag reboot_guard_seen 'reboot confirmation guard|reboot-guard'
flag state_cleanup_seen 'remove_account_state|state cleanup|account state.*remov'
flag systemd_failure_seen 'failed to start hardened|dependency failed for hardened|hardened.*failed with result'
""".strip()


class AuditReceipt(TypedDict):
    schemaVersion: int
    probe: str
    facts: dict[str, str]


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
    )


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("reboot persistence audit output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in FACT_KEYS
            or key in facts
            or not SAFE_VALUE.fullmatch(value)
        ):
            raise ValueError("reboot persistence audit returned an invalid fact")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("reboot persistence audit result was incomplete")
    return dict(sorted(facts.items()))


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=60)
        if result.returncode != 0:
            raise ValueError("reboot persistence audit SSH probe failed")
        receipt: AuditReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-reboot-persistence-audit",
            "facts": parse_facts(result.stdout),
        }
        write_json_0600_atomic(output, receipt)
        return 0
    finally:
        _zero(secret)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"reboot persistence audit rejected: {type(error).__name__}", file=sys.stderr)
        raise SystemExit(1) from None
