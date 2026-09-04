#!/usr/bin/env python3
"""Run a bounded read-only resource/privacy soak of the persistent H616 UI."""

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


SAMPLES = 30
INTERVAL_SECONDS = 10
FACT_KEYS = {
    "architecture",
    "broker_rss_max_kib",
    "control_boundary_absent",
    "diagnostic_denials",
    "duration_seconds",
    "health_requests",
    "inventory_denials",
    "kvmd_active",
    "nginx_rss_max_kib",
    "pids_stable",
    "ports_exact",
    "redirect_requests",
    "restarts_stable",
    "rootfs_mode",
    "samples",
    "server_rss_max_kib",
    "setup_closed",
    "setup_requests",
    "thermal_max_millidegrees",
    "thermal_min_millidegrees",
    "tls12",
    "tls13",
    "units_persistent",
    "video_denials",
    "video_rss_max_kib",
}
TOKEN_VALUES = {"aarch64", "active", "passed", "rw", "yes"}
MAX_OUTPUT_BYTES = 16 * 1024


REMOTE_SOAK = rf"""
set -eu
export LC_ALL=C
samples={SAMPLES}
interval={INTERVAL_SECONDS}
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
phase=preflight
on_exit() {{
  code=$?
  trap - EXIT
  if [ "$code" -ne 0 ]; then emit failure_phase "$phase"; fi
  exit "$code"
}}
trap on_exit EXIT
service_enabled() {{ systemctl is-enabled "$1" 2>/dev/null || true; }}
control_boundary_absent() {{
  [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ]
  [ "$(service_enabled hardened-blikvm-control-broker.service)" = disabled ]
  [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]
}}
listener_count() {{
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '$4 ~ (suffix "$" ) {{ count++ }} END {{ print count + 0 }}'
}}
listener_scope() {{
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '
    $4 ~ (suffix "$" ) {{
      seen=1
      if ($4 ~ /^0[.]0[.]0[.]0:/ || $4 ~ /^\[::\]:/ || $4 ~ /^\*:/) wildcard=1
      else other=1
    }}
    END {{
      if (!seen) print "absent"
      else if (wildcard && !other) print "wildcard"
      else print "mixed"
    }}'
}}
ports_exact() {{
  [ "$(listener_count 80)" = 2 ] && [ "$(listener_scope 80)" = wildcard ] &&
  [ "$(listener_count 443)" = 2 ] && [ "$(listener_scope 443)" = wildcard ]
}}
natural() {{ case "$1" in ''|*[!0-9]*) return 1 ;; *) return 0 ;; esac; }}
unit_rss_kib() {{
  group=$(systemctl show -p ControlGroup --value "$1")
  [ -n "$group" ]
  file="/sys/fs/cgroup$group/cgroup.procs"
  [ -r "$file" ]
  total=0
  while IFS= read -r pid; do
    natural "$pid"
    value=$(awk '/^VmRSS:/ {{print $2}}' "/proc/$pid/status")
    natural "$value"
    total=$((total + value))
  done < "$file"
  printf '%s' "$total"
}}
capture_initial() {{
  unit=$1 key=$2
  pid=$(systemctl show -p MainPID --value "$unit")
  restarts=$(systemctl show -p NRestarts --value "$unit")
  natural "$pid" && [ "$pid" -gt 0 ] && natural "$restarts"
  eval "${{key}}_pid=$pid"
  eval "${{key}}_restarts=$restarts"
  eval "${{key}}_rss_max=0"
}}
sample_unit() {{
  unit=$1 key=$2 limit=$3
  [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = active ]
  [ "$(service_enabled "$unit")" = enabled ]
  pid=$(systemctl show -p MainPID --value "$unit")
  restarts=$(systemctl show -p NRestarts --value "$unit")
  natural "$pid" && natural "$restarts"
  eval "initial_pid=\${{${{key}}_pid}}"
  eval "initial_restarts=\${{${{key}}_restarts}}"
  [ "$pid" = "$initial_pid" ] && [ "$restarts" = "$initial_restarts" ]
  rss=$(unit_rss_kib "$unit")
  natural "$rss" && [ "$rss" -le "$limit" ]
  eval "maximum=\${{${{key}}_rss_max}}"
  if [ "$rss" -gt "$maximum" ]; then eval "${{key}}_rss_max=$rss"; fi
}}
for tool in awk curl findmnt grep openssl sed sha256sum sleep ss systemctl; do
  command -v "$tool" >/dev/null 2>&1
done
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) ;; *) exit 1 ;; esac
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = inactive ]
[ "$(service_enabled kvmd-nginx.service)" = disabled ]
control_boundary_absent
ports_exact
capture_initial hardened-blikvm-platform-broker.service broker
capture_initial hardened-blikvm-video-proxy.service video
capture_initial hardened-blikvm-server.service server
capture_initial hardened-blikvm-nginx.service nginx
thermal_min=150001000
thermal_max=-40001000
health_requests=0
setup_requests=0
redirect_requests=0
inventory_denials=0
diagnostic_denials=0
video_denials=0
phase=tls-start
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1/api/health
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1/api/health
index=1
while [ "$index" -le "$samples" ]; do
  phase="sample-$index"
  sample_unit hardened-blikvm-platform-broker.service broker 131072
  sample_unit hardened-blikvm-video-proxy.service video 131072
  sample_unit hardened-blikvm-server.service server 524288
  sample_unit hardened-blikvm-nginx.service nginx 131072
  ports_exact
  [ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
  control_boundary_absent
  health_status=$(curl --noproxy '*' --insecure --silent --max-time 5 --output /dev/null --write-out '%{{http_code}}' https://127.0.0.1/api/health)
  [ "$health_status" = 200 ]
  health_requests=$((health_requests + 1))
  setup_response=$(curl --noproxy '*' --insecure --silent --max-time 5 https://127.0.0.1/api/auth/setup)
  case "$setup_response" in *'"required":false'*) ;; *) exit 1 ;; esac
  setup_requests=$((setup_requests + 1))
  inventory_status=$(curl --noproxy '*' --insecure --silent --max-time 5 --output /dev/null --write-out '%{{http_code}}' https://127.0.0.1/api/inventory)
  diagnostics_status=$(curl --noproxy '*' --insecure --silent --max-time 5 --output /dev/null --write-out '%{{http_code}}' https://127.0.0.1/api/diagnostics)
  video_status=$(curl --noproxy '*' --insecure --silent --max-time 5 --output /dev/null --write-out '%{{http_code}}' https://127.0.0.1/api/stream/mjpeg)
  [ "$inventory_status" = 401 ] && [ "$diagnostics_status" = 401 ] && [ "$video_status" = 401 ]
  inventory_denials=$((inventory_denials + 1))
  diagnostic_denials=$((diagnostic_denials + 1))
  video_denials=$((video_denials + 1))
  if [ $((index % 5)) = 0 ]; then
    redirect_status=$(curl --noproxy '*' --silent --max-time 5 --output /dev/null --write-out '%{{http_code}}' http://127.0.0.1/api/health)
    [ "$redirect_status" = 308 ]
    redirect_requests=$((redirect_requests + 1))
  fi
  thermal=$(cat /sys/class/thermal/thermal_zone0/temp)
  case "$thermal" in -*) thermal_digits=${{thermal#-}} ;; *) thermal_digits=$thermal ;; esac
  natural "$thermal_digits"
  [ "$thermal" -ge -40000000 ] && [ "$thermal" -le 150000000 ]
  if [ "$thermal" -lt "$thermal_min" ]; then thermal_min=$thermal; fi
  if [ "$thermal" -gt "$thermal_max" ]; then thermal_max=$thermal; fi
  sleep "$interval"
  index=$((index + 1))
done
phase=tls-final
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.2 --tls-max 1.2 --output /dev/null https://127.0.0.1/api/health
curl --noproxy '*' --insecure --silent --fail --max-time 5 --tlsv1.3 --tls-max 1.3 --output /dev/null https://127.0.0.1/api/health
ports_exact
control_boundary_absent
phase=complete
emit architecture aarch64
emit samples "$samples"
emit duration_seconds "$((samples * interval))"
emit health_requests "$health_requests"
emit setup_requests "$setup_requests"
emit redirect_requests "$redirect_requests"
emit inventory_denials "$inventory_denials"
emit diagnostic_denials "$diagnostic_denials"
emit video_denials "$video_denials"
emit broker_rss_max_kib "$broker_rss_max"
emit video_rss_max_kib "$video_rss_max"
emit server_rss_max_kib "$server_rss_max"
emit nginx_rss_max_kib "$nginx_rss_max"
emit thermal_min_millidegrees "$thermal_min"
emit thermal_max_millidegrees "$thermal_max"
emit pids_stable yes
emit restarts_stable yes
emit units_persistent yes
emit ports_exact yes
emit setup_closed yes
emit tls12 passed
emit tls13 passed
emit kvmd_active active
emit control_boundary_absent yes
emit rootfs_mode rw
""".strip()


class SoakReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    facts: dict[str, str]
    policy: dict[str, object]


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
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("soak output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("soak returned an unexpected field")
        if value not in TOKEN_VALUES and not re.fullmatch(r"-?[0-9]{1,12}", value):
            raise ValueError("soak returned an unsafe value")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("soak result was incomplete")
    expected = {
        "architecture": "aarch64",
        "samples": str(SAMPLES),
        "duration_seconds": str(SAMPLES * INTERVAL_SECONDS),
        "health_requests": str(SAMPLES),
        "setup_requests": str(SAMPLES),
        "redirect_requests": str(SAMPLES // 5),
        "inventory_denials": str(SAMPLES),
        "diagnostic_denials": str(SAMPLES),
        "video_denials": str(SAMPLES),
        "pids_stable": "yes",
        "restarts_stable": "yes",
        "units_persistent": "yes",
        "ports_exact": "yes",
        "setup_closed": "yes",
        "tls12": "passed",
        "tls13": "passed",
        "kvmd_active": "active",
        "control_boundary_absent": "yes",
        "rootfs_mode": "rw",
    }
    for key, wanted in expected.items():
        if facts[key] != wanted:
            raise ValueError(f"soak rejected field {key}")
    return dict(sorted(facts.items()))


def parse_failure_phase(raw: bytes) -> str | None:
    if len(raw) > MAX_OUTPUT_BYTES:
        return None
    for line in raw.decode("ascii", errors="ignore").splitlines():
        key, separator, value = line.partition("=")
        if key == "failure_phase" and separator == "=" and re.fullmatch(
            r"[a-z0-9-]{1,64}", value
        ):
            return value
    return None


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
        result = prototype.run_ssh(
            target,
            secret,
            REMOTE_SOAK,
            timeout=SAMPLES * INTERVAL_SECONDS + 90,
        )
        if result.returncode != 0:
            receipt = {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-persistent-resource-soak",
                "result": "failed",
                "failure": {
                    "returnCode": result.returncode,
                    "phase": parse_failure_phase(result.stdout),
                },
                "policy": {
                    "persistentWrites": False,
                    "authenticatedRequests": False,
                    "targetPowerControl": False,
                    "deviceReboot": False,
                    "secretEmitted": False,
                },
            }
            write_json_0600_atomic(output, receipt)
            return 1
        facts = parse_facts(result.stdout)
        passed: SoakReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-persistent-resource-soak",
            "result": "passed",
            "facts": facts,
            "policy": {
                "persistentWrites": False,
                "authenticatedRequests": False,
                "targetPowerControl": False,
                "deviceReboot": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, passed)
        return 0
    finally:
        _zero(secret)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"persistent soak rejected: {type(error).__name__}", file=sys.stderr)
        raise SystemExit(2) from None
