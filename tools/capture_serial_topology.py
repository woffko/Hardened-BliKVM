#!/usr/bin/env python3
"""Capture bounded metadata-only H616 ttyUSB topology without opening a TTY."""

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
TTY_INDICES = tuple(range(4))
TTY_FIELDS = (
    "present",
    "class_dev",
    "device",
    "driver",
    "usb_vendor_id",
    "usb_product_id",
    "node_present",
    "node_character",
    "node_mode",
    "node_owner",
    "node_group",
)
FACT_KEYS = {
    "architecture",
    "rootfs_mode",
    "ttyusb_count",
    "by_id_directory_present",
} | {f"ttyusb{index}_{field}" for index in TTY_INDICES for field in TTY_FIELDS}
SAFE_TOKEN = re.compile(r"^[A-Za-z0-9_.:+@-]{1,120}$")
SAFE_ACCOUNT = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]{0,63}$")
SAFE_MODE = re.compile(r"^[0-7]{3,4}$")
SAFE_DEV = re.compile(r"^[0-9]{1,5}:[0-9]{1,5}$")
SAFE_USB_ID = re.compile(r"^[0-9A-Fa-f]{4}$")

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
emit_tty() {
  index=$1
  class="/sys/class/tty/ttyUSB$index"
  node="/dev/ttyUSB$index"
  if [ -e "$class" ]; then
    emit "ttyusb${index}_present" yes
    emit "ttyusb${index}_class_dev" "$(read_value "$class/dev")"
    device_path=$(readlink -f "$class/device" 2>/dev/null || true)
    if [ -n "$device_path" ]; then emit "ttyusb${index}_device" "$(basename "$device_path")"; else emit "ttyusb${index}_device" unavailable; fi
    driver_path=$(readlink -f "$class/device/driver" 2>/dev/null || true)
    if [ -n "$driver_path" ]; then emit "ttyusb${index}_driver" "$(basename "$driver_path")"; else emit "ttyusb${index}_driver" unavailable; fi
    usb_vendor=unavailable
    usb_product=unavailable
    current=$device_path
    depth=0
    while [ -n "$current" ] && [ "$depth" -lt 8 ]; do
      if [ -r "$current/idVendor" ] && [ -r "$current/idProduct" ]; then
        usb_vendor=$(read_value "$current/idVendor")
        usb_product=$(read_value "$current/idProduct")
        break
      fi
      parent=$(dirname "$current")
      [ "$parent" != "$current" ] || break
      current=$parent
      depth=$((depth + 1))
    done
    emit "ttyusb${index}_usb_vendor_id" "$usb_vendor"
    emit "ttyusb${index}_usb_product_id" "$usb_product"
  else
    emit "ttyusb${index}_present" no
    emit "ttyusb${index}_class_dev" unavailable
    emit "ttyusb${index}_device" unavailable
    emit "ttyusb${index}_driver" unavailable
    emit "ttyusb${index}_usb_vendor_id" unavailable
    emit "ttyusb${index}_usb_product_id" unavailable
  fi
  if [ -c "$node" ]; then
    emit "ttyusb${index}_node_present" yes
    emit "ttyusb${index}_node_character" yes
    emit "ttyusb${index}_node_mode" "$(stat -c '%a' "$node")"
    emit "ttyusb${index}_node_owner" "$(stat -c '%U' "$node")"
    emit "ttyusb${index}_node_group" "$(stat -c '%G' "$node")"
  else
    emit "ttyusb${index}_node_present" no
    emit "ttyusb${index}_node_character" no
    emit "ttyusb${index}_node_mode" unavailable
    emit "ttyusb${index}_node_owner" unavailable
    emit "ttyusb${index}_node_group" unavailable
  fi
}
for tool in basename dirname findmnt readlink stat tr uname; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
ttyusb_count=0
for class in /sys/class/tty/ttyUSB[0-9]*; do
  [ -e "$class" ] || continue
  ttyusb_count=$((ttyusb_count + 1))
done
if [ -d /dev/serial/by-id ]; then
  by_id_directory_present=yes
else
  by_id_directory_present=no
fi
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"
emit ttyusb_count "$ttyusb_count"
emit by_id_directory_present "$by_id_directory_present"
for index in 0 1 2 3; do emit_tty "$index"; done
""".strip()


class SerialPolicy(TypedDict):
    filesystemWrites: bool
    ttyOpened: bool
    termiosChanged: bool
    framesRead: bool
    framesWritten: bool
    byIdIdentifiersRead: bool
    usbSerialRead: bool
    physicalActionsInvoked: bool


class SerialReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: SerialPolicy
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int | None
    stdoutBytes: int


class FailedSerialReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: SerialPolicy
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


def policy_receipt() -> SerialPolicy:
    return {
        "filesystemWrites": False,
        "ttyOpened": False,
        "termiosChanged": False,
        "framesRead": False,
        "framesWritten": False,
        "byIdIdentifiersRead": False,
        "usbSerialRead": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("serial probe output exceeded the size limit")
    text = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("serial probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("serial probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("serial platform facts are invalid")
    if not facts["ttyusb_count"].isdecimal() or not 0 <= int(facts["ttyusb_count"]) <= 32:
        raise ValueError("serial device count is invalid")
    if facts["by_id_directory_present"] not in {"yes", "no"}:
        raise ValueError("serial by-id directory presence is invalid")

    for index in TTY_INDICES:
        prefix = f"ttyusb{index}_"
        present = facts[prefix + "present"]
        if present not in {"yes", "no"}:
            raise ValueError("ttyUSB presence is invalid")
        class_dev = facts[prefix + "class_dev"]
        if class_dev != "unavailable" and not SAFE_DEV.fullmatch(class_dev):
            raise ValueError("ttyUSB class device is invalid")
        for field in ("device", "driver"):
            value = facts[prefix + field]
            if value != "unavailable" and not SAFE_TOKEN.fullmatch(value):
                raise ValueError("ttyUSB identity is invalid")
        for field in ("usb_vendor_id", "usb_product_id"):
            value = facts[prefix + field]
            if value != "unavailable" and not SAFE_USB_ID.fullmatch(value):
                raise ValueError("ttyUSB non-unique USB ID is invalid")
        if present == "no" and any(
            facts[prefix + field] != "unavailable"
            for field in ("class_dev", "device", "driver", "usb_vendor_id", "usb_product_id")
        ):
            raise ValueError("absent ttyUSB class exposed contradictory metadata")

        node_present = facts[prefix + "node_present"]
        node_character = facts[prefix + "node_character"]
        if node_present not in {"yes", "no"} or node_character not in {"yes", "no"}:
            raise ValueError("ttyUSB node presence is invalid")
        mode = facts[prefix + "node_mode"]
        owner = facts[prefix + "node_owner"]
        group = facts[prefix + "node_group"]
        if mode != "unavailable" and not SAFE_MODE.fullmatch(mode):
            raise ValueError("ttyUSB node mode is invalid")
        for value in (owner, group):
            if value != "unavailable" and not SAFE_ACCOUNT.fullmatch(value):
                raise ValueError("ttyUSB node ownership is invalid")
        if node_present == "yes" and node_character != "yes":
            raise ValueError("ttyUSB node is not a character device")
        if node_present == "no" and any(
            value != "unavailable" for value in (mode, owner, group)
        ):
            raise ValueError("absent ttyUSB node exposed contradictory metadata")
    return dict(sorted(facts.items()))


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("serial receipt path already exists")
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
        failed: FailedSerialReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-metadata-only-serial-topology",
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
            "probe": "hardened-blikvm-metadata-only-serial-topology",
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
    receipt: SerialReceipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-metadata-only-serial-topology",
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
