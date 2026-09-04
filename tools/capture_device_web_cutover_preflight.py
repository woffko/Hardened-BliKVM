#!/usr/bin/env python3
"""Capture the read-only facts required for the persistent web cutover.

The SSH password is consumed by sshpass from stdin. The probe never returns
certificate/key contents, raw Nginx configuration, network addresses, or
credentials. Its mode-0600 receipt is intentionally private.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import shlex
import subprocess
import sys
from typing import TypedDict, cast
from urllib import parse

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic


MAX_OUTPUT_BYTES = 24 * 1024
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.,:+/@()= -]{1,320}$")
SAFE_KEYS = {
    "architecture",
    "rootfs_mode",
    "kvmd_active",
    "kvmd_enabled",
    "kvmd_nginx_active",
    "kvmd_nginx_enabled",
    "kvmd_nginx_fragment",
    "kvmd_nginx_fragment_sha256",
    "nginx_binary",
    "nginx_config_path",
    "nginx_config_sha256",
    "nginx_config_test",
    "certificate_directive_count",
    "key_directive_count",
    "certificate_path",
    "certificate_sha256",
    "certificate_mode_owner",
    "certificate_symlink",
    "certificate_not_after",
    "key_path",
    "key_sha256",
    "key_mode_owner",
    "key_symlink",
    "live_certificate_matches",
    "port80_listener_count",
    "port80_scope",
    "port443_listener_count",
    "port443_scope",
    "hardened_units",
    "hardened_nginx_unit",
    "app_state",
    "control_boundary",
    "staging_absent",
}
FAILURE_KEY = "failure_phase"


REMOTE_PROBE = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
phase=initial
trap 'rc=$?; if [ "$rc" -ne 0 ]; then emit failure_phase "$phase"; fi' EXIT
service_enabled() {
  state=$(systemctl is-enabled "$1" 2>/dev/null || true)
  case "$state" in enabled|disabled|static|indirect|generated|masked|not-found) printf '%s' "$state" ;; *) printf 'other' ;; esac
}
listener_count() {
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '$4 ~ (suffix "$" ) { count++ } END { print count + 0 }'
}
listener_scope() {
  port=$1
  ss -H -ltn | awk -v suffix=":$port" '
    $4 ~ (suffix "$" ) {
      seen=1
      if ($4 ~ /^0[.]0[.]0[.]0:/ || $4 ~ /^\[::\]:/ || $4 ~ /^\*:/) wildcard=1
      else if ($4 ~ /^127[.]0[.]0[.]1:/ || $4 ~ /^\[::1\]:/) loopback=1
      else other=1
    }
    END {
      if (!seen) print "absent"
      else if (wildcard && !other) print "wildcard"
      else if (loopback && !wildcard && !other) print "loopback"
      else print "mixed"
    }'
}
for tool in awk cut find findmnt openssl readlink sha256sum sort ss stat systemctl tr wc; do
  command -v "$tool" >/dev/null 2>&1
done
phase=baseline
[ "$(id -u)" = 0 ]
emit architecture "$(uname -m)"
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) emit rootfs_mode ro ;; *) emit rootfs_mode rw ;; esac
emit kvmd_active "$(systemctl is-active kvmd.service 2>/dev/null || true)"
emit kvmd_enabled "$(service_enabled kvmd.service)"
emit kvmd_nginx_active "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)"
emit kvmd_nginx_enabled "$(service_enabled kvmd-nginx.service)"
phase=nginx-unit
fragment=$(systemctl show -p FragmentPath --value kvmd-nginx.service)
[ -f "$fragment" ]
[ ! -L "$fragment" ]
emit kvmd_nginx_fragment "$fragment"
emit kvmd_nginx_fragment_sha256 "$(sha256sum "$fragment" | awk '{print $1}')"
main_pid=$(systemctl show -p MainPID --value kvmd-nginx.service)
case "$main_pid" in ''|0|*[!0-9]*) exit 31 ;; esac
nginx_binary=$(readlink -f "/proc/$main_pid/exe")
[ -x "$nginx_binary" ]
emit nginx_binary "$nginx_binary"
nginx_config_path=$(tr '\000' ' ' < "/proc/$main_pid/cmdline" | awk '{ for (i=1; i<NF; i++) if ($i == "-c") { print $(i+1); exit } }')
if [ -z "$nginx_config_path" ]; then
  nginx_config_path=$("$nginx_binary" -V 2>&1 | tr ' ' '\n' | awk -F= '$1 == "--conf-path" { print $2; exit }')
fi
case "$nginx_config_path" in /*) ;; *) exit 34 ;; esac
[ -f "$nginx_config_path" ]
[ ! -L "$nginx_config_path" ]
emit nginx_config_path "$nginx_config_path"
phase=nginx-config
nginx_dump=$("$nginx_binary" -T -c "$nginx_config_path" 2>&1)
printf '%s\n' "$nginx_dump" | awk '/configuration file .* test is successful/ { found=1 } END { exit !found }'
emit nginx_config_test passed
config_hash=$(printf '%s\n' "$nginx_dump" | sha256sum | awk '{print $1}')
emit nginx_config_sha256 "$config_hash"
phase=certificate-directives
cert_paths=$(printf '%s\n' "$nginx_dump" | awk '$1 == "ssl_certificate" { sub(/;$/, "", $2); print $2 }' | sort -u)
key_paths=$(printf '%s\n' "$nginx_dump" | awk '$1 == "ssl_certificate_key" { sub(/;$/, "", $2); print $2 }' | sort -u)
certificate_directive_count=$(printf '%s\n' "$cert_paths" | awk 'NF { count++ } END { print count + 0 }')
key_directive_count=$(printf '%s\n' "$key_paths" | awk 'NF { count++ } END { print count + 0 }')
emit certificate_directive_count "$certificate_directive_count"
emit key_directive_count "$key_directive_count"
[ -n "$cert_paths" ]
[ -n "$key_paths" ]
phase=live-certificate-connect
live_cert_pem=$(
  printf '\n' | openssl s_client -connect 127.0.0.1:443 -servername localhost -showcerts 2>/dev/null |
    awk '/-----BEGIN CERTIFICATE-----/ { capture=1 } capture { print } /-----END CERTIFICATE-----/ { exit }'
)
[ -n "$live_cert_pem" ]
phase=live-certificate-parse
live_cert_hash=$(printf '%s\n' "$live_cert_pem" | openssl x509 -outform DER 2>/dev/null | sha256sum | awk '{print $1}')
[ "${#live_cert_hash}" = 64 ]
[ "$live_cert_hash" != e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 ]
phase=certificate-match
certificate_path=
certificate_matches=0
for candidate in $cert_paths; do
  case "$candidate" in /*) ;; *) exit 32 ;; esac
  [ -f "$candidate" ] || continue
  candidate_hash=$(openssl x509 -in "$candidate" -outform DER 2>/dev/null | sha256sum | awk '{print $1}')
  if [ "$candidate_hash" = "$live_cert_hash" ]; then
    certificate_path=$candidate
    certificate_matches=$((certificate_matches + 1))
  fi
done
[ "$certificate_matches" = 1 ]
phase=key-match
certificate_pubkey_hash=$(
  openssl x509 -in "$certificate_path" -pubkey -noout 2>/dev/null |
    openssl pkey -pubin -outform DER 2>/dev/null | sha256sum | awk '{print $1}'
)
key_path=
key_matches=0
for candidate in $key_paths; do
  case "$candidate" in /*) ;; *) exit 33 ;; esac
  [ -f "$candidate" ] || continue
  candidate_pubkey_hash=$(
    openssl pkey -in "$candidate" -pubout -outform DER 2>/dev/null |
      sha256sum | awk '{print $1}'
  )
  if [ "$candidate_pubkey_hash" = "$certificate_pubkey_hash" ]; then
    key_path=$candidate
    key_matches=$((key_matches + 1))
  fi
done
[ "$key_matches" = 1 ]
emit certificate_path "$certificate_path"
emit certificate_sha256 "$(sha256sum "$certificate_path" | awk '{print $1}')"
emit certificate_mode_owner "$(stat -Lc '%a:%u:%g' "$certificate_path")"
if [ -L "$certificate_path" ]; then emit certificate_symlink yes; else emit certificate_symlink no; fi
emit certificate_not_after "$(openssl x509 -in "$certificate_path" -noout -enddate | cut -d= -f2-)"
emit key_path "$key_path"
emit key_sha256 "$(sha256sum "$key_path" | awk '{print $1}')"
emit key_mode_owner "$(stat -Lc '%a:%u:%g' "$key_path")"
if [ -L "$key_path" ]; then emit key_symlink yes; else emit key_symlink no; fi
emit live_certificate_matches yes
phase=listeners
emit port80_listener_count "$(listener_count 80)"
emit port80_scope "$(listener_scope 80)"
emit port443_listener_count "$(listener_count 443)"
emit port443_scope "$(listener_scope 443)"
phase=hardened-state
hardened_units=yes
for unit in hardened-blikvm-platform-broker.service hardened-blikvm-video-proxy.service hardened-blikvm-server.service; do
  [ "$(systemctl is-active "$unit" 2>/dev/null || true)" = inactive ] || hardened_units=no
  [ "$(service_enabled "$unit")" = disabled ] || hardened_units=no
done
emit hardened_units "$hardened_units"
if [ "$(systemctl show -p LoadState --value hardened-blikvm-nginx.service 2>/dev/null || true)" = not-found ]; then
  emit hardened_nginx_unit absent
else
  emit hardened_nginx_unit present
fi
if [ ! -e /var/lib/hardened-blikvm ]; then emit app_state absent; else emit app_state present; fi
if [ "$(systemctl is-active hardened-blikvm-control-broker.service 2>/dev/null || true)" = inactive ] &&
   [ "$(service_enabled hardened-blikvm-control-broker.service)" = disabled ] &&
   [ "$(systemctl show -p LoadState --value hardened-blikvm-control-broker.socket 2>/dev/null || true)" = not-found ]; then
  emit control_boundary absent
else
  emit control_boundary present
fi
if find /var/lib -maxdepth 1 -type d \( -name 'hardened-blikvm-*-rollback-*' -o -name 'hardened-blikvm-web-cutover-*' \) -print -quit | grep -q . ||
   find /etc/systemd/system -maxdepth 3 \( -name 'hardened-blikvm-web-cutover-*' -o -name 'hardened-blikvm-*-rollback-*' \) -print -quit | grep -q .; then
  emit staging_absent no
else
  emit staging_absent yes
fi
phase=complete
""".strip()


class PreflightOutput(TypedDict):
    schemaVersion: int
    probe: str
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int
    stdoutBytes: int
    failurePhase: str
    completedFacts: dict[str, str]


class FailedPreflightOutput(TypedDict):
    schemaVersion: int
    probe: str
    failure: FailureInfo


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    output: str
    known_hosts: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--known-hosts", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        output=cast(str, namespace.output),
        known_hosts=cast(str, namespace.known_hosts),
    )


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
    return parsed.hostname, parsed.port or 22


def validate_username(username: str) -> str:
    if not re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", username):
        raise ValueError("invalid SSH username")
    return username


def parse_safe_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("cutover preflight output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("utf-8", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in SAFE_KEYS or key in facts:
            raise ValueError("cutover preflight returned an unexpected field")
        if not SAFE_VALUE.fullmatch(value):
            raise ValueError("cutover preflight returned an unsafe value")
        facts[key] = value
    if set(facts) != SAFE_KEYS:
        raise ValueError("cutover preflight result was incomplete")
    return dict(sorted(facts.items()))


def parse_partial_failure(raw: bytes) -> tuple[str, dict[str, str]]:
    if len(raw) > MAX_OUTPUT_BYTES:
        return "invalid-output", {}
    facts: dict[str, str] = {}
    failure_phase = "missing"
    try:
        lines = raw.decode("utf-8", errors="strict").splitlines()
    except UnicodeError:
        return "invalid-output", {}
    for line in lines:
        key, separator, value = line.partition("=")
        if separator != "=" or not SAFE_VALUE.fullmatch(value):
            return "invalid-output", {}
        if key == FAILURE_KEY:
            if failure_phase != "missing":
                return "invalid-output", {}
            failure_phase = value
        elif key in SAFE_KEYS and key not in facts:
            facts[key] = value
        else:
            return "invalid-output", {}
    return failure_phase, dict(sorted(facts.items()))


def main() -> int:
    options = parse_args()
    host, port = parse_endpoint(options.endpoint)
    username = validate_username(options.username)
    known_hosts = Path(options.known_hosts).resolve()
    known_hosts.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    command = [
        "sshpass",
        "-d",
        "0",
        "ssh",
        "-p",
        str(port),
        "-o",
        "BatchMode=no",
        "-o",
        "ConnectTimeout=8",
        "-o",
        "LogLevel=ERROR",
        "-o",
        "NumberOfPasswordPrompts=1",
        "-o",
        "PreferredAuthentications=password",
        "-o",
        "PubkeyAuthentication=no",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        f"UserKnownHostsFile={known_hosts}",
        f"{username}@{host}",
        f"sh -c {shlex.quote(REMOTE_PROBE)}",
    ]
    try:
        result = subprocess.run(
            command,
            stdin=sys.stdin.buffer,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.SubprocessError):
        result = subprocess.CompletedProcess(command, -1, b"")
    if known_hosts.exists():
        known_hosts.chmod(0o600)
    if result.returncode != 0:
        failure_phase, completed_facts = parse_partial_failure(result.stdout)
        failed: FailedPreflightOutput = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-web-cutover-preflight",
            "failure": {
                "stage": "ssh-read-only",
                "returnCode": result.returncode,
                "stdoutBytes": len(result.stdout),
                "failurePhase": failure_phase,
                "completedFacts": completed_facts,
            },
        }
        write_json_0600_atomic(Path(options.output), failed)
        return 1
    output: PreflightOutput = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-web-cutover-preflight",
        "facts": parse_safe_facts(result.stdout),
    }
    write_json_0600_atomic(Path(options.output), output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
