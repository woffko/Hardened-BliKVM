#!/usr/bin/env python3
"""Remove only retained inactive Hardened web files after guarded fallback."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
from pathlib import Path
import shlex
import sys
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


FACT_KEYS = {
    "account_state_absent",
    "config_removed",
    "control_boundary_absent",
    "hardened_units_inactive",
    "kvmd_active",
    "orphan_cutover_artifacts_absent",
    "ports_restored",
    "unit_removed",
    "vendor_nginx_active",
}


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    baseline: str
    output: str


class RecoveryReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    facts: dict[str, str]
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


def _assignments(values: Mapping[str, str]) -> str:
    return "\n".join(f"{key}={shlex.quote(value)}" for key, value in values.items())


def build_recovery_script(baseline: Mapping[str, str]) -> str:
    config = cutover.render_nginx(baseline)
    unit = cutover.render_unit(baseline)
    values = {
        "config_target": cutover.CONFIG_TARGET,
        "unit_target": cutover.UNIT_TARGET,
        "config_sha256": hashlib.sha256(config.encode()).hexdigest(),
        "unit_sha256": hashlib.sha256(unit.encode()).hexdigest(),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
        "state_root": cutover.STATE_ROOT,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
service_enabled() { systemctl is-enabled "$1" 2>/dev/null || true; }
product_inactive_disabled() {
  for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
    [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || return 1
    [ "$(service_enabled "$unit")" = disabled ] || return 1
  done
}
control_boundary_absent() {
  [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]
  [ "$(service_enabled hardened-blikvm-control-broker.service)" = disabled ]
  [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]
}
ports_restored() {
  [ "$(ss -H -ltn | awk '$4 ~ /:80$/ {count++} END {print count+0}')" = 2 ]
  [ "$(ss -H -ltn | awk '$4 ~ /:443$/ {count++} END {print count+0}')" = 2 ]
}
orphan_absent() {
  [ -z "$(find /var/lib -maxdepth 1 -type d -name 'hardened-blikvm-web-cutover-*' -print -quit)" ]
  [ -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-web-cutover-*' -print -quit)" ]
}
[ "$(id -u)" = 0 ]
root_options=$(findmnt -n -T / -o OPTIONS)
case ",$root_options," in *,rw,*) ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ "$(service_enabled kvmd-nginx.service)" = enabled ]
product_inactive_disabled
control_boundary_absent
ports_restored
orphan_absent
[ ! -e "$state_root" ]
[ -f "$config_target" ] && [ ! -L "$config_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ -f "$unit_target" ] && [ ! -L "$unit_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$unit_target")" = 644:0:0:1 ]
[ "$(sha256sum "$unit_target" | awk '{print $1}')" = "$unit_sha256" ]
[ "$(systemctl is-active hardened-blikvm-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled hardened-blikvm-nginx.service)" = disabled ]
[ "$(sha256sum "$certificate_path" | awk '{print $1}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{print $1}')" = "$key_sha256" ]
rm -f -- "$config_target" "$unit_target"
systemctl daemon-reload
[ "$(systemctl show -p LoadState --value hardened-blikvm-nginx.service 2>/dev/null || true)" = not-found ]
product_inactive_disabled
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
ports_restored
orphan_absent
emit config_removed yes
emit unit_removed yes
emit account_state_absent yes
emit hardened_units_inactive yes
emit vendor_nginx_active yes
emit kvmd_active active
emit ports_restored yes
emit orphan_cutover_artifacts_absent yes
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{body}"


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > 8 * 1024:
        raise ValueError("fallback recovery output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts or value not in {"yes", "active"}:
            raise ValueError("fallback recovery returned an invalid fact")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("fallback recovery result was incomplete")
    return dict(sorted(facts.items()))


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    baseline = cutover.load_baseline(options.baseline)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(
            target, secret, build_recovery_script(baseline), timeout=60
        )
        if result.returncode != 0:
            raise ValueError("fallback recovery remote command failed")
        receipt: RecoveryReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-web-cutover-fallback-recovery",
            "result": "passed",
            "facts": parse_facts(result.stdout),
            "policy": {
                "vendorWebInterrupted": False,
                "accountStateRemoved": False,
                "packageRemoved": False,
                "credentialsChanged": False,
                "targetPowerControl": False,
                "deviceReboot": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    finally:
        _zero(secret)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"fallback recovery rejected: {type(error).__name__}", file=sys.stderr)
        raise SystemExit(1) from None
