#!/usr/bin/env python3
"""Capture sanitized recovery state and journal classes after a cutover failure."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import re
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import capture_device_web_cutover_preflight as preflight
from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


JOURNAL_KEYS = {
    "address_in_use",
    "ambient_capability_failure",
    "bind_permission_denied",
    "capset_failure",
    "chown_failure",
    "condition_failure",
    "config_credential_permission_denied",
    "config_permission_denied",
    "credential_failure",
    "credential_missing",
    "exec_failure",
    "fork_failure",
    "invalid_argument",
    "journal_lines",
    "namespace_failure",
    "nginx_pid_permission_denied",
    "nginx_config_failure",
    "nginx_emergency",
    "operation_not_permitted",
    "setgid_failure",
    "setgroups_failure",
    "setrlimit_failure",
    "setsockopt_failure",
    "setuid_failure",
    "socketpair_failure",
    "runtime_directory_failure",
    "runtime_root_permission_denied",
    "status_1_failure",
    "status_203_failure",
    "system_call_filter_failure",
    "tls_certificate_permission_denied",
    "tls_key_permission_denied",
    "unit_parse_failure",
}
SAFE_VALUE = re.compile(r"^(?:yes|no|[0-9]{1,3}|[0-9a-f]{64})$")


REMOTE_JOURNAL_PROBE = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
flag() {
  name=$1
  pattern=$2
  if printf '%s\n' "$journal" | grep -Eqi "$pattern"; then emit "$name" yes; else emit "$name" no; fi
}
journal=$(journalctl -u hardened-blikvm-nginx.service --since '-5 minutes' -n 120 --no-pager -o cat 2>/dev/null || true)
lines=$(printf '%s\n' "$journal" | awk 'NF { count++ } END { print count + 0 }')
[ "$lines" -le 120 ]
emit journal_lines "$lines"
flag credential_failure 'Failed to (set up|load|read).*credential|credential.*(failed|permission denied)'
flag credential_missing 'credential.*(No such file|not found|missing)'
flag config_permission_denied '(nginx-config|nginx[.]conf).*[Pp]ermission denied'
flag config_credential_permission_denied '/run/credentials/[^ ]*/nginx-config.*[Pp]ermission denied|nginx-config.*[Pp]ermission denied'
flag tls_certificate_permission_denied 'tls-certificate.*[Pp]ermission denied'
flag tls_key_permission_denied 'tls-key.*[Pp]ermission denied'
flag nginx_pid_permission_denied 'nginx[.]pid.*[Pp]ermission denied'
flag runtime_root_permission_denied '/run/hardened-blikvm-nginx.*[Pp]ermission denied'
flag bind_permission_denied 'bind[(].*[Pp]ermission denied|socket[(].*[Pp]ermission denied'
flag address_in_use 'address already in use'
flag namespace_failure '(namespace|namespacing).*(failed|[Pp]ermission denied|[Oo]peration not permitted)'
flag runtime_directory_failure 'RuntimeDirectory|hardened-blikvm-nginx.*(No such file|[Pp]ermission denied)'
flag ambient_capability_failure 'ambient capabilit.*(failed|[Pp]ermission denied|[Oo]peration not permitted)'
flag system_call_filter_failure '(seccomp|system call filter).*(failed|killed|denied)'
flag operation_not_permitted '[Oo]peration not permitted'
flag setrlimit_failure 'setrlimit.*[Oo]peration not permitted'
flag setuid_failure 'setuid.*[Oo]peration not permitted'
flag setgid_failure 'setgid.*[Oo]peration not permitted'
flag setgroups_failure '(setgroups|initgroups).*[Oo]peration not permitted'
flag chown_failure 'chown.*[Oo]peration not permitted'
flag socketpair_failure 'socketpair.*[Oo]peration not permitted'
flag setsockopt_failure 'setsockopt.*[Oo]peration not permitted'
flag fork_failure '(fork|clone).*[Oo]peration not permitted'
flag capset_failure '(capset|capabilit).*[Oo]peration not permitted'
flag invalid_argument '[Ii]nvalid argument'
flag unit_parse_failure '(unknown lvalue|failed to parse|bad unit setting|unknown key)'
flag condition_failure 'Condition.*failed|start condition failed'
flag exec_failure 'Failed at step EXEC|status=203/EXEC'
flag status_203_failure 'status=203/EXEC'
flag status_1_failure 'status=1/FAILURE'
flag nginx_config_failure 'nginx: configuration file .* test failed'
flag nginx_emergency 'nginx: \[emerg\]'
printf '%s\n' "$journal" | sha256sum | awk '{print "journal_sha256=" $1}'
""".strip()


class FailureOutput(TypedDict):
    schemaVersion: int
    probe: str
    recoveryFacts: dict[str, str]
    journalClasses: dict[str, str]
    journalSha256: str


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


def parse_journal_facts(raw: bytes) -> tuple[dict[str, str], str]:
    if len(raw) > 8 * 1024:
        raise ValueError("journal classification output exceeded the limit")
    facts: dict[str, str] = {}
    journal_sha256 = ""
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or not SAFE_VALUE.fullmatch(value):
            raise ValueError("journal classifier returned an invalid fact")
        if key == "journal_sha256":
            if journal_sha256 or not re.fullmatch(r"[0-9a-f]{64}", value):
                raise ValueError("journal classifier digest is invalid")
            journal_sha256 = value
        elif key in JOURNAL_KEYS and key not in facts:
            facts[key] = value
        else:
            raise ValueError("journal classifier returned an unexpected field")
    if set(facts) != JOURNAL_KEYS or not journal_sha256:
        raise ValueError("journal classifier result was incomplete")
    if int(facts["journal_lines"]) > 120:
        raise ValueError("journal classifier line count is invalid")
    return dict(sorted(facts.items())), journal_sha256


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
        recovery_result = prototype.run_ssh(
            target, secret, preflight.REMOTE_PROBE, timeout=60
        )
        if recovery_result.returncode != 0:
            raise ValueError("recovery-state probe failed")
        recovery = preflight.parse_safe_facts(recovery_result.stdout)
        journal_result = prototype.run_ssh(
            target, secret, REMOTE_JOURNAL_PROBE, timeout=30
        )
        if journal_result.returncode != 0:
            raise ValueError("journal classification probe failed")
        classes, journal_sha256 = parse_journal_facts(journal_result.stdout)
        receipt: FailureOutput = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-web-cutover-failure-classification",
            "recoveryFacts": recovery,
            "journalClasses": classes,
            "journalSha256": journal_sha256,
        }
        write_json_0600_atomic(output, receipt)
        return 0
    finally:
        _zero(secret)


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"cutover failure capture rejected: {type(error).__name__}", file=sys.stderr)
        raise SystemExit(1) from None
