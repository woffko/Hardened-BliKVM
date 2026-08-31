#!/usr/bin/env python3
"""Capture sanitized H616 network ownership metadata without network config."""

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


MAX_OUTPUT_BYTES = 12 * 1024
INTERFACES = ("eth0", "end0", "wlan0", "usb0")
INTERFACE_FIELDS = ("present", "type", "operstate", "carrier", "wireless", "driver")
FACT_KEYS = {
    "architecture",
    "rootfs_mode",
    "interface_count",
    "nmcli_available",
    "networkmanager_state",
    "systemd_networkd_state",
    "connman_state",
    "dhcpcd_state",
} | {f"{name}_{field}" for name in INTERFACES for field in INTERFACE_FIELDS}
SAFE_TOKEN = re.compile(r"^[A-Za-z0-9_.:+@-]{1,120}$")
SERVICE_STATES = {
    "active",
    "inactive",
    "failed",
    "activating",
    "deactivating",
    "reloading",
    "maintenance",
    "unknown",
}
OPER_STATES = {
    "unknown",
    "notpresent",
    "down",
    "lowerlayerdown",
    "testing",
    "dormant",
    "up",
    "unavailable",
}

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
service_state() {
  key=$1
  unit=$2
  state=$(systemctl is-active "$unit" 2>/dev/null || true)
  case "$state" in
    active|inactive|failed|activating|deactivating|reloading|maintenance) ;;
    *) state=unknown ;;
  esac
  emit "$key" "$state"
}
emit_interface() {
  name=$1
  interface="/sys/class/net/$name"
  if [ -e "$interface" ]; then
    emit "${name}_present" yes
    emit "${name}_type" "$(read_value "$interface/type")"
    emit "${name}_operstate" "$(read_value "$interface/operstate")"
    emit "${name}_carrier" "$(read_value "$interface/carrier")"
    if [ -d "$interface/wireless" ]; then emit "${name}_wireless" yes; else emit "${name}_wireless" no; fi
    driver_path=$(readlink -f "$interface/device/driver" 2>/dev/null || true)
    if [ -n "$driver_path" ]; then emit "${name}_driver" "$(basename "$driver_path")"; else emit "${name}_driver" unavailable; fi
  else
    emit "${name}_present" no
    emit "${name}_type" unavailable
    emit "${name}_operstate" unavailable
    emit "${name}_carrier" unavailable
    emit "${name}_wireless" unavailable
    emit "${name}_driver" unavailable
  fi
}
for tool in basename findmnt readlink systemctl tr uname; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
interface_count=0
for interface in /sys/class/net/*; do
  [ -e "$interface" ] || continue
  interface_count=$((interface_count + 1))
done
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"
emit interface_count "$interface_count"
if command -v nmcli >/dev/null 2>&1; then emit nmcli_available yes; else emit nmcli_available no; fi
service_state networkmanager_state NetworkManager.service
service_state systemd_networkd_state systemd-networkd.service
service_state connman_state connman.service
service_state dhcpcd_state dhcpcd.service
for name in eth0 end0 wlan0 usb0; do emit_interface "$name"; done
""".strip()


class NetworkPolicy(TypedDict):
    filesystemWrites: bool
    sysfsInterfaceMetadataRead: bool
    serviceStateCommandsInvoked: bool
    nmcliInvoked: bool
    addressesRead: bool
    linkLayerAddressesRead: bool
    ssidOrBssidRead: bool
    routesRead: bool
    resolversRead: bool
    connectionProfilesRead: bool
    wirelessScanTriggered: bool
    managerMutationCommandsInvoked: bool
    interfaceStateChanged: bool
    servicesChanged: bool
    uniqueIdentifiersRead: bool
    physicalActionsInvoked: bool


class NetworkReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: NetworkPolicy
    facts: dict[str, str]


class FailureInfo(TypedDict):
    stage: str
    returnCode: int | None
    stdoutBytes: int


class FailedNetworkReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    policy: NetworkPolicy
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


def policy_receipt() -> NetworkPolicy:
    return {
        "filesystemWrites": False,
        "sysfsInterfaceMetadataRead": True,
        "serviceStateCommandsInvoked": True,
        "nmcliInvoked": False,
        "addressesRead": False,
        "linkLayerAddressesRead": False,
        "ssidOrBssidRead": False,
        "routesRead": False,
        "resolversRead": False,
        "connectionProfilesRead": False,
        "wirelessScanTriggered": False,
        "managerMutationCommandsInvoked": False,
        "interfaceStateChanged": False,
        "servicesChanged": False,
        "uniqueIdentifiersRead": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("network probe output exceeded the size limit")
    text = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("network probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("network probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("network platform facts are invalid")
    if not facts["interface_count"].isdecimal() or not 1 <= int(facts["interface_count"]) <= 64:
        raise ValueError("network interface count is invalid")
    if facts["nmcli_available"] not in {"yes", "no"}:
        raise ValueError("nmcli availability is invalid")
    for key in (
        "networkmanager_state",
        "systemd_networkd_state",
        "connman_state",
        "dhcpcd_state",
    ):
        if facts[key] not in SERVICE_STATES:
            raise ValueError("network manager state is invalid")

    present_count = 0
    for name in INTERFACES:
        prefix = f"{name}_"
        present = facts[prefix + "present"]
        if present not in {"yes", "no"}:
            raise ValueError("network interface presence is invalid")
        if present == "yes":
            present_count += 1
        interface_type = facts[prefix + "type"]
        if interface_type != "unavailable" and (
            not interface_type.isdecimal() or len(interface_type) > 5
        ):
            raise ValueError("network interface type is invalid")
        if facts[prefix + "operstate"] not in OPER_STATES:
            raise ValueError("network operstate is invalid")
        if facts[prefix + "carrier"] not in {"0", "1", "unavailable"}:
            raise ValueError("network carrier state is invalid")
        if facts[prefix + "wireless"] not in {"yes", "no", "unavailable"}:
            raise ValueError("network wireless metadata is invalid")
        driver = facts[prefix + "driver"]
        if driver != "unavailable" and not SAFE_TOKEN.fullmatch(driver):
            raise ValueError("network driver metadata is invalid")
        if present == "no" and any(
            facts[prefix + field] != "unavailable"
            for field in ("type", "operstate", "carrier", "wireless", "driver")
        ):
            raise ValueError("absent network interface exposed contradictory metadata")
    if present_count > int(facts["interface_count"]):
        raise ValueError("modeled interface count exceeds total interface count")
    return dict(sorted(facts.items()))


def _zero(value: bytearray) -> None:
    for index in range(len(value)):
        value[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("network receipt path already exists")
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
        failed: FailedNetworkReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-sanitized-network-topology",
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
            "probe": "hardened-blikvm-sanitized-network-topology",
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
    receipt: NetworkReceipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-sanitized-network-topology",
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
