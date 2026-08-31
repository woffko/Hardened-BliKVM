#!/usr/bin/env python3
"""Capture a bounded, sanitized, read-only H616 kernel RTC baseline."""

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
RTC_FIELDS = (
    "present",
    "name",
    "device",
    "driver",
    "class_dev",
    "hctosys",
    "date",
    "time",
    "since_epoch",
    "dev_present",
    "dev_mode",
    "dev_owner",
    "dev_group",
    "dev_character",
)
FACT_KEYS = {"architecture", "rootfs_mode", "rtc_count"} | {
    f"rtc{index}_{field}" for index in range(2) for field in RTC_FIELDS
}
SAFE_TOKEN = re.compile(r"^[A-Za-z0-9_.:+@ -]{1,120}$")
SAFE_ACCOUNT = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]{0,63}$")
SAFE_MODE = re.compile(r"^[0-7]{3,4}$")
SAFE_DEV = re.compile(r"^[0-9]{1,5}:[0-9]{1,5}$")
SAFE_DATE = re.compile(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$")
SAFE_TIME = re.compile(r"^[0-9]{2}:[0-9]{2}:[0-9]{2}$")

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
emit_rtc() {
  index=$1
  class="/sys/class/rtc/rtc$index"
  node="/dev/rtc$index"
  if [ -e "$class" ]; then
    emit "rtc${index}_present" yes
    emit "rtc${index}_name" "$(read_value "$class/name")"
    device_path=$(readlink -f "$class/device" 2>/dev/null || true)
    if [ -n "$device_path" ]; then emit "rtc${index}_device" "$(basename "$device_path")"; else emit "rtc${index}_device" unavailable; fi
    driver_path=$(readlink -f "$class/device/driver" 2>/dev/null || true)
    if [ -n "$driver_path" ]; then emit "rtc${index}_driver" "$(basename "$driver_path")"; else emit "rtc${index}_driver" unavailable; fi
    emit "rtc${index}_class_dev" "$(read_value "$class/dev")"
    emit "rtc${index}_hctosys" "$(read_value "$class/hctosys")"
    emit "rtc${index}_date" "$(read_value "$class/date")"
    emit "rtc${index}_time" "$(read_value "$class/time")"
    emit "rtc${index}_since_epoch" "$(read_value "$class/since_epoch")"
  else
    for field in present name device driver class_dev hctosys date time since_epoch; do
      if [ "$field" = present ]; then emit "rtc${index}_${field}" no; else emit "rtc${index}_${field}" unavailable; fi
    done
  fi
  if [ -c "$node" ]; then
    emit "rtc${index}_dev_present" yes
    emit "rtc${index}_dev_mode" "$(stat -c '%a' "$node")"
    emit "rtc${index}_dev_owner" "$(stat -c '%U' "$node")"
    emit "rtc${index}_dev_group" "$(stat -c '%G' "$node")"
    emit "rtc${index}_dev_character" yes
  else
    emit "rtc${index}_dev_present" no
    emit "rtc${index}_dev_mode" unavailable
    emit "rtc${index}_dev_owner" unavailable
    emit "rtc${index}_dev_group" unavailable
    emit "rtc${index}_dev_character" no
  fi
}
for tool in basename findmnt readlink stat tr uname; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
rtc_count=0
for class in /sys/class/rtc/rtc[0-9]*; do
  [ -e "$class" ] || continue
  rtc_count=$((rtc_count + 1))
done
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"
emit rtc_count "$rtc_count"
emit_rtc 0
emit_rtc 1
""".strip()


class RtcPolicy(TypedDict):
    filesystemWrites: bool
    rawI2cDeviceOpened: bool
    rtcDeviceOpened: bool
    clockSet: bool
    physicalActionsInvoked: bool


class RtcReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: RtcPolicy
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int | None
    stdoutBytes: int


class FailedRtcReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: RtcPolicy
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


def policy_receipt() -> RtcPolicy:
    return {
        "filesystemWrites": False,
        "rawI2cDeviceOpened": False,
        "rtcDeviceOpened": False,
        "clockSet": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("RTC probe output exceeded the size limit")
    text = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("RTC probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("RTC probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("RTC platform facts are invalid")
    if not facts["rtc_count"].isdecimal() or not 0 <= int(facts["rtc_count"]) <= 8:
        raise ValueError("RTC count is invalid")
    for index in range(2):
        prefix = f"rtc{index}_"
        present = facts[prefix + "present"]
        if present not in {"yes", "no"}:
            raise ValueError("RTC presence is invalid")
        for field in ("name", "device", "driver"):
            value = facts[prefix + field]
            if value != "unavailable" and not SAFE_TOKEN.fullmatch(value):
                raise ValueError("RTC identity is invalid")
        class_dev = facts[prefix + "class_dev"]
        if class_dev != "unavailable" and not SAFE_DEV.fullmatch(class_dev):
            raise ValueError("RTC class device is invalid")
        if facts[prefix + "hctosys"] not in {"0", "1", "unavailable"}:
            raise ValueError("RTC hctosys value is invalid")
        date = facts[prefix + "date"]
        time = facts[prefix + "time"]
        epoch = facts[prefix + "since_epoch"]
        if date != "unavailable" and not SAFE_DATE.fullmatch(date):
            raise ValueError("RTC date is invalid")
        if time != "unavailable" and not SAFE_TIME.fullmatch(time):
            raise ValueError("RTC time is invalid")
        if epoch != "unavailable" and (not epoch.isdecimal() or len(epoch) > 12):
            raise ValueError("RTC epoch is invalid")
        dev_present = facts[prefix + "dev_present"]
        dev_character = facts[prefix + "dev_character"]
        if dev_present not in {"yes", "no"} or dev_character not in {"yes", "no"}:
            raise ValueError("RTC device presence is invalid")
        mode = facts[prefix + "dev_mode"]
        owner = facts[prefix + "dev_owner"]
        group = facts[prefix + "dev_group"]
        if mode != "unavailable" and not SAFE_MODE.fullmatch(mode):
            raise ValueError("RTC device mode is invalid")
        for value in (owner, group):
            if value != "unavailable" and not SAFE_ACCOUNT.fullmatch(value):
                raise ValueError("RTC device ownership is invalid")
        if present == "no" and any(
            facts[prefix + field] != "unavailable"
            for field in ("name", "device", "driver", "class_dev", "hctosys", "date", "time", "since_epoch")
        ):
            raise ValueError("absent RTC class exposed contradictory facts")
        if dev_present == "yes" and dev_character != "yes":
            raise ValueError("RTC device is not a character node")
        if dev_present == "no" and any(value != "unavailable" for value in (mode, owner, group)):
            raise ValueError("absent RTC device exposed contradictory metadata")
    return dict(sorted(facts.items()))


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("RTC receipt path already exists")
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=25)
    finally:
        _zero(secret)
    if result.returncode != 0:
        failed: FailedRtcReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-read-only-rtc-baseline",
            "result": "failed",
            "policy": policy_receipt(),
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
            "schemaVersion": 1,
            "probe": "hardened-blikvm-read-only-rtc-baseline",
            "result": "failed",
            "policy": policy_receipt(),
            "failure": {
                "stage": "parse",
                "returnCode": None,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(output, failed)
        return 1
    receipt: RtcReceipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-read-only-rtc-baseline",
        "result": "passed",
        "policy": policy_receipt(),
        "facts": facts,
    }
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    return execute(parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
