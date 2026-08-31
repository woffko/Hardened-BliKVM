#!/usr/bin/env python3
"""Capture bounded metadata-only H616 ST7789 SPI topology without opening it."""

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


MAX_OUTPUT_BYTES = 8 * 1024
FACT_KEYS = {
    "architecture",
    "rootfs_mode",
    "spidev_class_present",
    "spidev_class_dev",
    "spidev_device",
    "spidev_driver",
    "spidev_modalias",
    "spidev_of_compatible",
    "spi_bus_device_present",
    "spi_master_present",
    "spi_master_device",
    "spi_master_driver",
    "node_present",
    "node_character",
    "node_mode",
    "node_owner",
    "node_group",
}
SAFE_ACCOUNT = re.compile(r"^[A-Za-z_][A-Za-z0-9_-]{0,63}$")
SAFE_MODE = re.compile(r"^[0-7]{3,4}$")
SAFE_DEV = re.compile(r"^[0-9]{1,5}:[0-9]{1,5}$")
SAFE_TOKEN = re.compile(r"^[A-Za-z0-9_.:+@-]{1,120}$")
SAFE_METADATA = re.compile(r"^[A-Za-z0-9_.,:+@/-]{1,240}$")

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
read_compatible() {
  if [ -r "$1" ]; then
    value=$(tr '\000' ',' < "$1" | sed 's/,$//')
    if [ -n "$value" ]; then printf '%s' "$value"; else printf unavailable; fi
  else
    printf unavailable
  fi
}
for tool in basename findmnt readlink sed stat tr uname; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"

class=/sys/class/spidev/spidev1.0
if [ -e "$class" ]; then
  emit spidev_class_present yes
  emit spidev_class_dev "$(read_value "$class/dev")"
  device_path=$(readlink -f "$class/device" 2>/dev/null || true)
  if [ -n "$device_path" ]; then emit spidev_device "$(basename "$device_path")"; else emit spidev_device unavailable; fi
  driver_path=$(readlink -f "$class/device/driver" 2>/dev/null || true)
  if [ -n "$driver_path" ]; then emit spidev_driver "$(basename "$driver_path")"; else emit spidev_driver unavailable; fi
  emit spidev_modalias "$(read_value "$class/device/modalias")"
  emit spidev_of_compatible "$(read_compatible "$class/device/of_node/compatible")"
else
  emit spidev_class_present no
  emit spidev_class_dev unavailable
  emit spidev_device unavailable
  emit spidev_driver unavailable
  emit spidev_modalias unavailable
  emit spidev_of_compatible unavailable
fi

if [ -e /sys/bus/spi/devices/spi1.0 ]; then emit spi_bus_device_present yes; else emit spi_bus_device_present no; fi

master=/sys/class/spi_master/spi1
if [ -e "$master" ]; then
  emit spi_master_present yes
  master_device_path=$(readlink -f "$master/device" 2>/dev/null || true)
  if [ -n "$master_device_path" ]; then emit spi_master_device "$(basename "$master_device_path")"; else emit spi_master_device unavailable; fi
  master_driver_path=$(readlink -f "$master/device/driver" 2>/dev/null || true)
  if [ -n "$master_driver_path" ]; then emit spi_master_driver "$(basename "$master_driver_path")"; else emit spi_master_driver unavailable; fi
else
  emit spi_master_present no
  emit spi_master_device unavailable
  emit spi_master_driver unavailable
fi

node=/dev/spidev1.0
if [ -e "$node" ]; then
  emit node_present yes
  if [ -c "$node" ]; then emit node_character yes; else emit node_character no; fi
  emit node_mode "$(stat -c '%a' "$node")"
  emit node_owner "$(stat -c '%U' "$node")"
  emit node_group "$(stat -c '%G' "$node")"
else
  emit node_present no
  emit node_character no
  emit node_mode unavailable
  emit node_owner unavailable
  emit node_group unavailable
fi
""".strip()


class OledSpiPolicy(TypedDict):
    filesystemWrites: bool
    rawSpiOpened: bool
    spiIoctlsInvoked: bool
    spiTransfersPerformed: bool
    driverBindingChanged: bool
    gpioLinesRequested: bool
    gpioValuesRead: bool
    screenStateChanged: bool
    uniqueIdentifiersRead: bool
    physicalActionsInvoked: bool


class OledSpiReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: OledSpiPolicy
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int | None
    stdoutBytes: int


class FailedOledSpiReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: OledSpiPolicy
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


def policy_receipt() -> OledSpiPolicy:
    return {
        "filesystemWrites": False,
        "rawSpiOpened": False,
        "spiIoctlsInvoked": False,
        "spiTransfersPerformed": False,
        "driverBindingChanged": False,
        "gpioLinesRequested": False,
        "gpioValuesRead": False,
        "screenStateChanged": False,
        "uniqueIdentifiersRead": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("OLED SPI probe output exceeded the size limit")
    text = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("OLED SPI probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("OLED SPI probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("OLED SPI platform facts are invalid")

    for key in (
        "spidev_class_present",
        "spi_bus_device_present",
        "spi_master_present",
        "node_present",
        "node_character",
    ):
        if facts[key] not in {"yes", "no"}:
            raise ValueError("OLED SPI presence metadata is invalid")

    class_dev = facts["spidev_class_dev"]
    if class_dev != "unavailable" and not SAFE_DEV.fullmatch(class_dev):
        raise ValueError("OLED SPI class device number is invalid")
    for key in (
        "spidev_device",
        "spidev_driver",
        "spi_master_device",
        "spi_master_driver",
    ):
        value = facts[key]
        if value != "unavailable" and not SAFE_TOKEN.fullmatch(value):
            raise ValueError("OLED SPI identity metadata is invalid")
    for key in ("spidev_modalias", "spidev_of_compatible"):
        value = facts[key]
        if value != "unavailable" and not SAFE_METADATA.fullmatch(value):
            raise ValueError("OLED SPI firmware metadata is invalid")
    if facts["spidev_class_present"] == "no" and any(
        facts[key] != "unavailable"
        for key in (
            "spidev_class_dev",
            "spidev_device",
            "spidev_driver",
            "spidev_modalias",
            "spidev_of_compatible",
        )
    ):
        raise ValueError("absent OLED SPI class exposed contradictory metadata")
    if facts["spi_master_present"] == "no" and any(
        facts[key] != "unavailable" for key in ("spi_master_device", "spi_master_driver")
    ):
        raise ValueError("absent SPI master exposed contradictory metadata")

    mode = facts["node_mode"]
    owner = facts["node_owner"]
    group = facts["node_group"]
    if mode != "unavailable" and not SAFE_MODE.fullmatch(mode):
        raise ValueError("OLED SPI node mode is invalid")
    for value in (owner, group):
        if value != "unavailable" and not SAFE_ACCOUNT.fullmatch(value):
            raise ValueError("OLED SPI node ownership is invalid")
    if facts["node_present"] == "yes" and facts["node_character"] != "yes":
        raise ValueError("OLED SPI node is not a character device")
    if facts["node_present"] == "no" and (
        facts["node_character"] != "no"
        or any(value != "unavailable" for value in (mode, owner, group))
    ):
        raise ValueError("absent OLED SPI node exposed contradictory metadata")
    return dict(sorted(facts.items()))


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("OLED SPI receipt path already exists")
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
        failed: FailedOledSpiReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-metadata-only-st7789-spi-topology",
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
            "probe": "hardened-blikvm-metadata-only-st7789-spi-topology",
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
    receipt: OledSpiReceipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-metadata-only-st7789-spi-topology",
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
