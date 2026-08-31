#!/usr/bin/env python3
"""Capture bounded read-only H616 pinned GPIO-profile ownership metadata."""

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


MAX_OUTPUT_BYTES = 16 * 1024
SCHEMA_VERSION = 2
PROFILE_OFFSETS = (228, 272, 234, 233, 257, 261, 269, 259, 260, 262)
LINE_FIELDS = ("status", "info", "legacy_present", "direction", "active_low", "edge")
FACT_KEYS = {
    "architecture",
    "rootfs_mode",
    "chip_present",
    "chip_character",
    "chip_mode",
    "chip_owner",
    "chip_group",
    "sysfs_present",
    "sysfs_label",
    "sysfs_ngpio",
    "sysfs_base",
    "gpioinfo_available",
    "gpioinfo_version",
    "gpioinfo_probe",
} | {
    f"line{offset}_{field}" for offset in PROFILE_OFFSETS for field in LINE_FIELDS
}
SAFE_ACCOUNT = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]{0,63}$")
SAFE_MODE = re.compile(r"^[0-7]{3,4}$")
SAFE_LABEL = re.compile(r"^[A-Za-z0-9_.:+@/ -]{1,120}$")
SAFE_INFO = re.compile(r"^[A-Za-z0-9_.,:+@/()\[\]\" -]{1,240}$")

REMOTE_PROBE = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
read_value() {
  if [ -r "$1" ]; then
    value=$(tr -d '\r\n' < "$1")
    if [ -n "$value" ]; then printf '%s' "$value"; else printf unavailable; fi
  else
    printf unavailable
  fi
}
for tool in awk findmnt head sed stat tr uname wc; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"
if [ -c /dev/gpiochip0 ]; then
  emit chip_present yes
  emit chip_character yes
  emit chip_mode "$(stat -c '%a' /dev/gpiochip0)"
  emit chip_owner "$(stat -c '%U' /dev/gpiochip0)"
  emit chip_group "$(stat -c '%G' /dev/gpiochip0)"
else
  emit chip_present no
  emit chip_character no
  emit chip_mode unavailable
  emit chip_owner unavailable
  emit chip_group unavailable
fi
if [ -e /sys/class/gpio/gpiochip0 ]; then
  emit sysfs_present yes
  emit sysfs_label "$(read_value /sys/class/gpio/gpiochip0/label)"
  emit sysfs_ngpio "$(read_value /sys/class/gpio/gpiochip0/ngpio)"
  emit sysfs_base "$(read_value /sys/class/gpio/gpiochip0/base)"
else
  emit sysfs_present no
  emit sysfs_label unavailable
  emit sysfs_ngpio unavailable
  emit sysfs_base unavailable
fi
emit_legacy_line() {
  offset=$1
  legacy="/sys/class/gpio/gpio$offset"
  if [ -e "$legacy" ]; then
    emit "line${offset}_legacy_present" yes
    emit "line${offset}_direction" "$(read_value "$legacy/direction")"
    emit "line${offset}_active_low" "$(read_value "$legacy/active_low")"
    emit "line${offset}_edge" "$(read_value "$legacy/edge")"
  else
    emit "line${offset}_legacy_present" no
    emit "line${offset}_direction" unavailable
    emit "line${offset}_active_low" unavailable
    emit "line${offset}_edge" unavailable
  fi
}
for offset in 228 272 234 233 257 261 269 259 260 262; do emit_legacy_line "$offset"; done
emit_line_info() {
  offset=$1
  line_info=$(printf '%s\n' "$gpioinfo_output" | awk -v target="$offset" '
    {
      line = $0
      sub(/^[[:space:]]*/, "", line)
      if (line ~ ("^line[[:space:]]+" target ":") || line ~ ("^" target ":")) { print line; exit }
    }
  ' | tr '\t' ' ' | tr -s ' ' | sed 's/^ //;s/ $//')
  if [ -z "$line_info" ]; then
    emit "line${offset}_status" missing
    emit "line${offset}_info" unavailable
  elif [ "$(printf '%s' "$line_info" | wc -c | tr -d ' ')" -le 240 ]; then
    emit "line${offset}_status" captured
    emit "line${offset}_info" "$line_info"
  else
    emit "line${offset}_status" oversized
    emit "line${offset}_info" unavailable
  fi
}
emit_unavailable_lines() {
  for offset in 228 272 234 233 257 261 269 259 260 262; do
    emit "line${offset}_status" unavailable
    emit "line${offset}_info" unavailable
  done
}
if command -v gpioinfo >/dev/null 2>&1; then
  emit gpioinfo_available yes
  gpioinfo_version=$(gpioinfo --version 2>&1 | head -n 1 | tr -s ' ')
  if [ -n "$gpioinfo_version" ]; then emit gpioinfo_version "$gpioinfo_version"; else emit gpioinfo_version unavailable; fi
  set +e
  gpioinfo_output=$(gpioinfo /dev/gpiochip0 2>/dev/null)
  gpioinfo_rc=$?
  if [ "$gpioinfo_rc" -ne 0 ]; then
    gpioinfo_output=$(gpioinfo gpiochip0 2>/dev/null)
    gpioinfo_rc=$?
  fi
  if [ "$gpioinfo_rc" -ne 0 ]; then
    gpioinfo_output=$(gpioinfo -c gpiochip0 228 272 234 233 257 261 269 259 260 262 2>/dev/null)
    gpioinfo_rc=$?
  fi
  set -e
  if [ "$gpioinfo_rc" -eq 0 ]; then
    emit gpioinfo_probe success
    for offset in 228 272 234 233 257 261 269 259 260 262; do emit_line_info "$offset"; done
  else
    emit gpioinfo_probe error
    emit_unavailable_lines
  fi
else
  emit gpioinfo_available no
  emit gpioinfo_version unavailable
  emit gpioinfo_probe not-run
  emit_unavailable_lines
fi
""".strip()


class GpioPolicy(TypedDict):
    filesystemWrites: bool
    gpioInfoCommandInvoked: bool
    lineRequestPerformed: bool
    lineValueRead: bool
    lineConfigured: bool
    physicalActionsInvoked: bool


class GpioReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: GpioPolicy
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int | None
    stdoutBytes: int


class FailedGpioReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: GpioPolicy
    failure: FailureInfo


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
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
    )


def policy_receipt(gpioinfo_invoked: bool) -> GpioPolicy:
    return {
        "filesystemWrites": False,
        "gpioInfoCommandInvoked": gpioinfo_invoked,
        "lineRequestPerformed": False,
        "lineValueRead": False,
        "lineConfigured": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("GPIO probe output exceeded the size limit")
    text = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("GPIO probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("GPIO probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("GPIO platform facts are invalid")
    if facts["chip_present"] not in {"yes", "no"} or facts["chip_character"] not in {
        "yes",
        "no",
    }:
        raise ValueError("GPIO chip presence is invalid")
    mode = facts["chip_mode"]
    owner = facts["chip_owner"]
    group = facts["chip_group"]
    if mode != "unavailable" and not SAFE_MODE.fullmatch(mode):
        raise ValueError("GPIO chip mode is invalid")
    for value in (owner, group):
        if value != "unavailable" and not SAFE_ACCOUNT.fullmatch(value):
            raise ValueError("GPIO chip ownership is invalid")
    if facts["chip_present"] == "yes" and facts["chip_character"] != "yes":
        raise ValueError("GPIO chip is not a character device")
    if facts["chip_present"] == "no" and any(
        value != "unavailable" for value in (mode, owner, group)
    ):
        raise ValueError("absent GPIO chip exposed contradictory metadata")

    if facts["sysfs_present"] not in {"yes", "no"}:
        raise ValueError("GPIO sysfs presence is invalid")
    label = facts["sysfs_label"]
    if label != "unavailable" and not SAFE_LABEL.fullmatch(label):
        raise ValueError("GPIO label is invalid")
    version = facts["gpioinfo_version"]
    if version != "unavailable" and not SAFE_INFO.fullmatch(version):
        raise ValueError("gpioinfo version is invalid")
    for key in ("sysfs_ngpio", "sysfs_base"):
        value = facts[key]
        if value != "unavailable" and (
            not value.lstrip("-").isdecimal() or len(value) > 6
        ):
            raise ValueError("GPIO numeric metadata is invalid")
    if facts["sysfs_present"] == "no" and any(
        facts[key] != "unavailable" for key in ("sysfs_label", "sysfs_ngpio", "sysfs_base")
    ):
        raise ValueError("absent GPIO sysfs exposed contradictory metadata")

    if facts["gpioinfo_available"] not in {"yes", "no"}:
        raise ValueError("gpioinfo availability is invalid")
    if facts["gpioinfo_probe"] not in {"success", "error", "not-run"}:
        raise ValueError("gpioinfo probe status is invalid")
    for offset in PROFILE_OFFSETS:
        prefix = f"line{offset}_"
        if facts[prefix + "legacy_present"] not in {"yes", "no"}:
            raise ValueError("legacy GPIO line presence is invalid")
        if facts[prefix + "direction"] not in {"in", "out", "high", "low", "unavailable"}:
            raise ValueError("legacy GPIO direction is invalid")
        if facts[prefix + "active_low"] not in {"0", "1", "unavailable"}:
            raise ValueError("legacy GPIO active-low flag is invalid")
        if facts[prefix + "edge"] not in {
            "none",
            "rising",
            "falling",
            "both",
            "unavailable",
        }:
            raise ValueError("legacy GPIO edge is invalid")
        if facts[prefix + "legacy_present"] == "no" and any(
            facts[prefix + field] != "unavailable"
            for field in ("direction", "active_low", "edge")
        ):
            raise ValueError("absent legacy GPIO line exposed contradictory metadata")
        if facts[prefix + "status"] not in {
            "captured",
            "missing",
            "oversized",
            "unavailable",
        }:
            raise ValueError("GPIO line status is invalid")
        line = facts[prefix + "info"]
        if line != "unavailable" and not SAFE_INFO.fullmatch(line):
            raise ValueError("GPIO line metadata is invalid")
        if (facts[prefix + "status"] == "captured") != (line != "unavailable"):
            raise ValueError("GPIO line metadata is contradictory")
    if facts["gpioinfo_available"] == "no" and (
        facts["gpioinfo_version"] != "unavailable"
        or facts["gpioinfo_probe"] != "not-run"
        or any(facts[f"line{offset}_status"] != "unavailable" for offset in PROFILE_OFFSETS)
    ):
        raise ValueError("unavailable gpioinfo exposed contradictory facts")
    return dict(sorted(facts.items()))


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("GPIO receipt path already exists")
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=30)
    finally:
        _zero(secret)
    invoked = b"gpioinfo_available=yes\n" in result.stdout
    if result.returncode != 0:
        failed: FailedGpioReceipt = {
            "schemaVersion": SCHEMA_VERSION,
            "probe": "hardened-blikvm-read-only-gpio-profile-baseline",
            "result": "failed",
            "policy": policy_receipt(invoked),
            "failure": {
                "stage": "remote-probe",
                "returnCode": result.returncode,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(output, failed)
        return 1
    try:
        facts = parse_facts(result.stdout)
    except (UnicodeError, ValueError):
        failed = {
            "schemaVersion": SCHEMA_VERSION,
            "probe": "hardened-blikvm-read-only-gpio-profile-baseline",
            "result": "failed",
            "policy": policy_receipt(invoked),
            "failure": {
                "stage": "parse",
                "returnCode": None,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(output, failed)
        return 1
    receipt: GpioReceipt = {
        "schemaVersion": SCHEMA_VERSION,
        "probe": "hardened-blikvm-read-only-gpio-profile-baseline",
        "result": "passed",
        "policy": policy_receipt(facts["gpioinfo_available"] == "yes"),
        "facts": facts,
    }
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    return execute(parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
