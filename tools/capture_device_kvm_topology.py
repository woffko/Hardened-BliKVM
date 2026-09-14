#!/usr/bin/env python3
"""Capture the sanitized KVM topology of a BliKVM H616 device, read-only.

The probe records only fixed facts and booleans about the current stack: the
USB gadget functions and a hash of its descriptor strings, the KVMD hid/msd
configuration, the BliKVM vendor daemon presence, the ATX socket, the MSD mount
mode, and active services. It never reads credentials, image contents, device
serial numbers, or any secret, and it performs no mutation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


EXPLICIT_FACTS = (
    "architecture",
    "gadget_present",
    "descriptor_sha256",
    "hid_functions",
    "mass_storage_functions",
    "udc_present",
    "kvmd_hid_otg",
    "kvmd_msd_otg",
    "kvmd_active",
    "kvmd_main_active",
    "ustreamer_active",
    "janus_active",
    "nginx_active",
    "blikvm_kvmd_main",
    "blikvm_enable_gadget",
    "blikvm_kvmd_msd",
    "atx_socket",
    "msd_mount_ro",
)
FACTS_KEYS = frozenset(EXPLICIT_FACTS)
BOOLEAN_FACTS = (
    "gadget_present",
    "udc_present",
    "kvmd_hid_otg",
    "kvmd_msd_otg",
    "kvmd_active",
    "kvmd_main_active",
    "ustreamer_active",
    "janus_active",
    "nginx_active",
    "blikvm_kvmd_main",
    "blikvm_enable_gadget",
    "blikvm_kvmd_msd",
    "atx_socket",
    "msd_mount_ro",
)
COUNT_FACTS = ("hid_functions", "mass_storage_functions")
ARCHITECTURE = re.compile(r"^[a-z][a-z0-9_]{1,15}$")
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")

REMOTE_PROBE = r"""set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
emit architecture "$(uname -m)"
gdir=/sys/kernel/config/usb_gadget
gname=$(ls "$gdir" 2>/dev/null | head -1 || true)
if [ -n "$gname" ]; then emit gadget_present yes; else emit gadget_present no; fi
g="$gdir/$gname"
desc_hash=$( { cat "$g/idVendor" "$g/idProduct" 2>/dev/null; cat "$g"/strings/0x409/* 2>/dev/null; } | sha256sum | cut -d ' ' -f 1 )
case "$desc_hash" in *[!0-9a-f]*|"") desc_hash=none ;; esac
emit descriptor_sha256 "$desc_hash"
hid=$(ls "$g/functions" 2>/dev/null | grep -c '^hid\.' || true)
msd=$(ls "$g/functions" 2>/dev/null | grep -c '^mass_storage' || true)
emit hid_functions "${hid:-0}"
emit mass_storage_functions "${msd:-0}"
if [ -n "$(cat "$g/UDC" 2>/dev/null | tr -d '\n')" ]; then emit udc_present yes; else emit udc_present no; fi
if grep -A1 -E '^[[:space:]]+hid:' /etc/kvmd/main.yaml 2>/dev/null | grep -q 'type: otg'; then emit kvmd_hid_otg yes; else emit kvmd_hid_otg no; fi
if grep -A1 -E '^[[:space:]]+msd:' /etc/kvmd/main.yaml 2>/dev/null | grep -q 'type: otg'; then emit kvmd_msd_otg yes; else emit kvmd_msd_otg no; fi
is_active() { if [ "$(systemctl is-active "$1" 2>/dev/null || true)" = active ]; then echo yes; else echo no; fi; }
emit kvmd_active "$(is_active kvmd.service)"
emit kvmd_main_active "$(is_active kvmd-main.service)"
emit ustreamer_active "$(is_active kvmd-video.service)"
emit janus_active "$(is_active kvmd-janus.service)"
emit nginx_active "$(is_active nginx.service)"
if [ -x /usr/bin/blikvm/kvmd-main ]; then emit blikvm_kvmd_main yes; else emit blikvm_kvmd_main no; fi
if [ -x /usr/bin/blikvm/enable-gadget.sh ]; then emit blikvm_enable_gadget yes; else emit blikvm_enable_gadget no; fi
if [ -x /usr/bin/blikvm/kvmd-msd.sh ]; then emit blikvm_kvmd_msd yes; else emit blikvm_kvmd_msd no; fi
if [ -S /var/blikvm/atx.sock ]; then emit atx_socket yes; else emit atx_socket no; fi
opts=$(findmnt -n -o OPTIONS --target /var/lib/kvmd/msd 2>/dev/null || true)
case ",$opts," in *,ro,*) emit msd_mount_ro yes ;; *) emit msd_mount_ro no ;; esac
"""


def parse_facts(raw: bytes) -> dict[str, str]:
    facts = prototype.parse_facts(raw, set(FACTS_KEYS))
    if not ARCHITECTURE.fullmatch(facts["architecture"]):
        raise ValueError("unexpected architecture value")
    for key in BOOLEAN_FACTS:
        if facts[key] not in {"yes", "no"}:
            raise ValueError("unexpected boolean value")
    for key in COUNT_FACTS:
        if not facts[key].isdigit() or not 0 <= int(facts[key]) <= 8:
            raise ValueError("unexpected function count")
    hash_value = facts["descriptor_sha256"]
    if hash_value != "none" and not HEX_SHA256.fullmatch(hash_value):
        raise ValueError("unexpected descriptor hash")
    gadget = facts["gadget_present"] == "yes"
    function_count = int(facts["hid_functions"]) + int(facts["mass_storage_functions"])
    if gadget and function_count == 0:
        raise ValueError("gadget reported without functions")
    if not gadget and (function_count > 0 or hash_value != "none" or facts["udc_present"] == "yes"):
        raise ValueError("gadget facts contradict gadget_present")
    return facts


def policy_receipt() -> dict[str, bool]:
    return {
        "sysfsGadgetMetadataRead": True,
        "serviceStateCommandsInvoked": True,
        "kvmdConfigRead": True,
        "mountOptionsRead": True,
        "descriptorBlockHashed": True,
        "descriptorStringValuesRecorded": False,
        "uniqueIdentifiersRecorded": False,
        "credentialsRead": False,
        "imageContentsRead": False,
        "mutationsPerformed": False,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=40)
    result.check_returncode()
    facts = parse_facts(result.stdout)
    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-kvm-topology",
        "facts": facts,
        "policy": policy_receipt(),
    }
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    print(f"topology={output} facts={len(facts)} receiptSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
