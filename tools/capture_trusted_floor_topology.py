#!/usr/bin/env python3
"""Capture sanitized TPM, TEE, and RPMB presence metadata without device I/O."""

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
CONFIG_KEYS = (
    "config_tcg_tpm",
    "config_tcg_tis",
    "config_tcg_tis_spi",
    "config_tee",
    "config_optee",
    "config_mmc_block",
    "config_mmc_sunxi",
    "config_trusted_keys_tpm",
)
NODES = (
    "tpm0",
    "tpmrm0",
    "tee0",
    "teepriv0",
    "mmcblk0rpmb",
    "mmcblk1rpmb",
    "mmcblk2rpmb",
)
FACT_KEYS = {
    "architecture",
    "rootfs_mode",
    "kernel_config_available",
    "sys_class_tpm_count",
    "sys_class_tee_count",
    "mmc_block_count",
    "rpmb_sysfs_count",
    "rpmb_dev_count",
    "dt_optee_node_present",
    "tpm_core_module_present",
    "tpm_tis_core_module_present",
    "optee_module_present",
} | set(CONFIG_KEYS) | {
    f"{name}_{field}" for name in NODES for field in ("present", "type", "mode")
}
CONFIG_STATES = {"builtin", "module", "no", "unavailable"}
NODE_TYPES = {"char", "block", "other", "unavailable"}
YES_NO = {"yes", "no"}
MODE = re.compile(r"^[0-7]{3,4}$")


REMOTE_PROBE = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
config_state() {
  key=$1
  symbol=$2
  file=$3
  if [ ! -r "$file" ]; then emit "$key" unavailable
  elif grep -qx "$symbol=y" "$file"; then emit "$key" builtin
  elif grep -qx "$symbol=m" "$file"; then emit "$key" module
  elif grep -qx "# $symbol is not set" "$file"; then emit "$key" no
  else emit "$key" unavailable
  fi
}
node_metadata() {
  key=$1
  path=$2
  if [ -e "$path" ]; then
    emit "${key}_present" yes
    if [ -c "$path" ]; then type=char
    elif [ -b "$path" ]; then type=block
    else type=other
    fi
    emit "${key}_type" "$type"
    emit "${key}_mode" "$(stat -c %a "$path")"
  else
    emit "${key}_present" no
    emit "${key}_type" unavailable
    emit "${key}_mode" unavailable
  fi
}
count_matches() {
  count=0
  for path in "$@"; do [ -e "$path" ] || continue; count=$((count + 1)); done
  printf '%s' "$count"
}
for tool in findmnt grep stat uname; do command -v "$tool" >/dev/null 2>&1; done
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,ro,*) rootfs_mode=ro ;; *) rootfs_mode=rw ;; esac
kernel_config="/boot/config-$(uname -r)"
if [ -r "$kernel_config" ]; then kernel_config_available=yes; else kernel_config_available=no; fi
emit architecture "$(uname -m)"
emit rootfs_mode "$rootfs_mode"
emit kernel_config_available "$kernel_config_available"
config_state config_tcg_tpm CONFIG_TCG_TPM "$kernel_config"
config_state config_tcg_tis CONFIG_TCG_TIS "$kernel_config"
config_state config_tcg_tis_spi CONFIG_TCG_TIS_SPI "$kernel_config"
config_state config_tee CONFIG_TEE "$kernel_config"
config_state config_optee CONFIG_OPTEE "$kernel_config"
config_state config_mmc_block CONFIG_MMC_BLOCK "$kernel_config"
config_state config_mmc_sunxi CONFIG_MMC_SUNXI "$kernel_config"
config_state config_trusted_keys_tpm CONFIG_TRUSTED_KEYS_TPM "$kernel_config"
emit sys_class_tpm_count "$(count_matches /sys/class/tpm/tpm*)"
emit sys_class_tee_count "$(count_matches /sys/class/tee/tee*)"
emit mmc_block_count "$(count_matches /sys/class/block/mmcblk[0-9])"
emit rpmb_sysfs_count "$(count_matches /sys/class/block/mmcblk*rpmb)"
emit rpmb_dev_count "$(count_matches /dev/mmcblk*rpmb)"
if [ -e /sys/firmware/devicetree/base/firmware/optee ] || [ -e /proc/device-tree/firmware/optee ]; then
  emit dt_optee_node_present yes
else
  emit dt_optee_node_present no
fi
if [ -d /sys/module/tpm ]; then emit tpm_core_module_present yes; else emit tpm_core_module_present no; fi
if [ -d /sys/module/tpm_tis_core ]; then emit tpm_tis_core_module_present yes; else emit tpm_tis_core_module_present no; fi
if [ -d /sys/module/optee ]; then emit optee_module_present yes; else emit optee_module_present no; fi
node_metadata tpm0 /dev/tpm0
node_metadata tpmrm0 /dev/tpmrm0
node_metadata tee0 /dev/tee0
node_metadata teepriv0 /dev/teepriv0
node_metadata mmcblk0rpmb /dev/mmcblk0rpmb
node_metadata mmcblk1rpmb /dev/mmcblk1rpmb
node_metadata mmcblk2rpmb /dev/mmcblk2rpmb
""".strip()


class FloorPolicy(TypedDict):
    filesystemWrites: bool
    fixedPathMetadataRead: bool
    kernelConfigRead: bool
    deviceNodesOpened: bool
    deviceContentsRead: bool
    mmcIdentityRead: bool
    tpmIdentityRead: bool
    teeIdentityRead: bool
    uniqueIdentifiersRead: bool
    keysProvisioned: bool
    secureObjectsCreated: bool
    servicesChanged: bool
    rootfsHelpersInvoked: bool
    physicalActionsInvoked: bool


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


def policy_receipt() -> FloorPolicy:
    return {
        "filesystemWrites": False,
        "fixedPathMetadataRead": True,
        "kernelConfigRead": True,
        "deviceNodesOpened": False,
        "deviceContentsRead": False,
        "mmcIdentityRead": False,
        "tpmIdentityRead": False,
        "teeIdentityRead": False,
        "uniqueIdentifiersRead": False,
        "keysProvisioned": False,
        "secureObjectsCreated": False,
        "servicesChanged": False,
        "rootfsHelpersInvoked": False,
        "physicalActionsInvoked": False,
    }


def parse_facts(raw: bytes) -> dict[str, str]:
    if len(raw) > MAX_OUTPUT_BYTES:
        raise ValueError("trusted-floor probe output exceeded the size limit")
    decoded = raw.decode("ascii", errors="strict")
    facts: dict[str, str] = {}
    for line in decoded.splitlines():
        key, separator, value = line.partition("=")
        if separator != "=" or key not in FACT_KEYS or key in facts:
            raise ValueError("trusted-floor probe returned an unexpected field")
        facts[key] = value
    if set(facts) != FACT_KEYS:
        raise ValueError("trusted-floor probe result was incomplete")
    if facts["architecture"] != "aarch64" or facts["rootfs_mode"] not in {"ro", "rw"}:
        raise ValueError("trusted-floor platform facts are invalid")
    if facts["kernel_config_available"] not in YES_NO:
        raise ValueError("kernel config availability is invalid")
    for key in CONFIG_KEYS:
        if facts[key] not in CONFIG_STATES:
            raise ValueError("kernel configuration state is invalid")
    for key in (
        "sys_class_tpm_count",
        "sys_class_tee_count",
        "mmc_block_count",
        "rpmb_sysfs_count",
        "rpmb_dev_count",
    ):
        if not facts[key].isdecimal() or not 0 <= int(facts[key]) <= 32:
            raise ValueError("trusted-floor topology count is invalid")
    for key in (
        "dt_optee_node_present",
        "tpm_core_module_present",
        "tpm_tis_core_module_present",
        "optee_module_present",
    ):
        if facts[key] not in YES_NO:
            raise ValueError("trusted-floor presence fact is invalid")
    present_by_family = {"tpm": 0, "tee": 0, "rpmb": 0}
    for name in NODES:
        present = facts[f"{name}_present"]
        node_type = facts[f"{name}_type"]
        mode = facts[f"{name}_mode"]
        if present not in YES_NO or node_type not in NODE_TYPES:
            raise ValueError("trusted-floor node metadata is invalid")
        if present == "no":
            if node_type != "unavailable" or mode != "unavailable":
                raise ValueError("absent trusted-floor node is contradictory")
        else:
            if node_type in {"unavailable", "other"} or MODE.fullmatch(mode) is None:
                raise ValueError("present trusted-floor node metadata is unsafe")
            family = "tpm" if name.startswith("tpm") else "tee" if name.startswith("tee") else "rpmb"
            present_by_family[family] += 1
    if present_by_family["tpm"] > int(facts["sys_class_tpm_count"]) + 1:
        raise ValueError("TPM node/class metadata is contradictory")
    if present_by_family["tee"] > int(facts["sys_class_tee_count"]):
        raise ValueError("TEE node/class metadata is contradictory")
    if present_by_family["rpmb"] > int(facts["rpmb_dev_count"]):
        raise ValueError("RPMB node/count metadata is contradictory")
    return dict(sorted(facts.items()))


def zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    if output.exists():
        raise ValueError("trusted-floor receipt path already exists")
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    try:
        result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=25)
    finally:
        zero(secret)
    if result.returncode != 0:
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-trusted-floor-topology",
            "result": "failed",
            "policy": policy_receipt(),
            "failure": {
                "stage": "remote-probe",
                "returnCode": result.returncode,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(output, receipt)
        return 1
    try:
        facts = parse_facts(result.stdout)
    except (UnicodeError, ValueError):
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-trusted-floor-topology",
            "result": "failed",
            "policy": policy_receipt(),
            "failure": {
                "stage": "parse",
                "returnCode": None,
                "stdoutBytes": len(result.stdout),
            },
        }
        write_json_0600_atomic(output, receipt)
        return 1
    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-trusted-floor-topology",
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
