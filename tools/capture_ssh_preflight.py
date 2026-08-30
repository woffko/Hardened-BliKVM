#!/usr/bin/env python3
"""Capture a bounded, sanitized, read-only SSH deployment preflight.

The SSH password is consumed by sshpass from stdin. Raw SSH output, stderr,
endpoint data, hostnames, and credentials are never written to the result.
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

if __package__:
    from .capture_pikvm_baseline import write_json_0600_atomic
else:
    from capture_pikvm_baseline import write_json_0600_atomic


MAX_OUTPUT_BYTES = 16 * 1024
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.,:+/()= -]{1,240}$")
SAFE_KEYS = {
    "architecture",
    "kernel",
    "os",
    "rootfs_mode",
    "tmp_mount_mode",
    "run_mount_mode",
    "tmp_available_bytes",
    "kvmd_active",
    "kvmd_nginx_active",
    "kvmd_socket",
    "kvmd_socket_mode",
    "kvmd_socket_owner",
    "kvmd_socket_group",
    "systemd_version",
    "systemd_credentials",
    "rw_helper",
    "ro_helper",
}

REMOTE_PROBE = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
emit architecture "$(uname -m)"
emit kernel "$(uname -r)"
emit os "$(. /etc/os-release && printf '%s' "$PRETTY_NAME")"
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) emit rootfs_mode ro ;; *) emit rootfs_mode rw ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) emit tmp_mount_mode noexec ;; *) emit tmp_mount_mode exec ;; esac
run_options=$(findmnt -n -o OPTIONS /run 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$run_options," in *,noexec,*) emit run_mount_mode noexec ;; *) emit run_mount_mode exec ;; esac
emit tmp_available_bytes "$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')"
emit kvmd_active "$(systemctl is-active kvmd.service 2>/dev/null || true)"
emit kvmd_nginx_active "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)"
if [ -S /run/kvmd/kvmd.sock ]; then
  emit kvmd_socket present
  emit kvmd_socket_mode "$(stat -c '%a' /run/kvmd/kvmd.sock)"
  emit kvmd_socket_owner "$(stat -c '%U' /run/kvmd/kvmd.sock)"
  emit kvmd_socket_group "$(stat -c '%G' /run/kvmd/kvmd.sock)"
else
  emit kvmd_socket missing
  emit kvmd_socket_mode unavailable
  emit kvmd_socket_owner unavailable
  emit kvmd_socket_group unavailable
fi
emit systemd_version "$(systemctl --version | head -n 1)"
if command -v systemd-creds >/dev/null 2>&1; then emit systemd_credentials available; else emit systemd_credentials unavailable; fi
if command -v rw >/dev/null 2>&1; then emit rw_helper available; else emit rw_helper unavailable; fi
if command -v ro >/dev/null 2>&1; then emit ro_helper available; else emit ro_helper unavailable; fi
""".strip()


class PreflightOutput(TypedDict):
    schemaVersion: int
    probe: str
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int
    stdoutBytes: int


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
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
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
        raise ValueError("SSH probe output exceeded the size limit")
    text = raw.decode("utf-8", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in SAFE_KEYS or key in facts:
            raise ValueError("SSH probe returned an unexpected field")
        if not SAFE_VALUE.fullmatch(value):
            raise ValueError("SSH probe returned an unsafe value")
        facts[key] = value
    if set(facts) != SAFE_KEYS:
        raise ValueError("SSH probe result was incomplete")
    return dict(sorted(facts.items()))


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
        "StrictHostKeyChecking=accept-new",
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
            timeout=20,
        )
    except subprocess.TimeoutExpired:
        timed_out: FailedPreflightOutput = {
            "schemaVersion": 1,
            "probe": "blikvm-ssh-read-only-preflight",
            "failure": {"stage": "ssh-timeout", "returnCode": -1, "stdoutBytes": 0},
        }
        write_json_0600_atomic(Path(options.output), timed_out)
        return 1
    except OSError as error:
        start_failed: FailedPreflightOutput = {
            "schemaVersion": 1,
            "probe": "blikvm-ssh-read-only-preflight",
            "failure": {
                "stage": "ssh-start",
                "returnCode": -(error.errno or 1),
                "stdoutBytes": 0,
            },
        }
        write_json_0600_atomic(Path(options.output), start_failed)
        return 1
    except subprocess.SubprocessError:
        subprocess_failed: FailedPreflightOutput = {
            "schemaVersion": 1,
            "probe": "blikvm-ssh-read-only-preflight",
            "failure": {"stage": "ssh-start", "returnCode": -1, "stdoutBytes": 0},
        }
        write_json_0600_atomic(Path(options.output), subprocess_failed)
        return 1
    if known_hosts.exists():
        known_hosts.chmod(0o600)
    if result.returncode != 0:
        failed: FailedPreflightOutput = {
            "schemaVersion": 1,
            "probe": "blikvm-ssh-read-only-preflight",
            "failure": {
                "stage": "ssh",
                "returnCode": result.returncode,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(Path(options.output), failed)
        return 1

    output: PreflightOutput = {
        "schemaVersion": 1,
        "probe": "blikvm-ssh-read-only-preflight",
        "facts": parse_safe_facts(result.stdout),
    }
    write_json_0600_atomic(Path(options.output), output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
