#!/usr/bin/env python3
"""Read-only preflight for the board-owner handoff gate.

Confirms that the SSH target is the expected BliKVM H616 stack before any
mutation: KVMD and the vendor daemon are present, a USB gadget and its UDC are
bound, configfs is mounted. It performs no mutation and writes a sanitized
private receipt, including a single `handoffReady` verdict.
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
    "kvmd_present",
    "vendor_present",
    "gadget_present",
    "udc_present",
    "configfs_mounted",
    "kvmd_active",
    "kvmd_main_active",
    "gadget_name",
)
FACTS_KEYS = frozenset(EXPLICIT_FACTS)
BOOLEAN_FACTS = (
    "kvmd_present",
    "vendor_present",
    "gadget_present",
    "udc_present",
    "configfs_mounted",
    "kvmd_active",
    "kvmd_main_active",
)
GADGET_NAME = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")

REMOTE_SCRIPT = r"""set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ -d /etc/kvmd ]; then emit kvmd_present yes; else emit kvmd_present no; fi
if [ -d /usr/bin/blikvm ]; then emit vendor_present yes; else emit vendor_present no; fi
if mount | grep -q 'configfs on /sys/kernel/config type configfs'; then emit configfs_mounted yes; else emit configfs_mounted no; fi
gname=$(ls /sys/kernel/config/usb_gadget 2>/dev/null | head -1 || true)
if [ -n "$gname" ]; then emit gadget_present yes; emit gadget_name "$gname"; else emit gadget_present no; emit gadget_name none; fi
if [ -d /sys/class/udc ] && [ -n "$(ls /sys/class/udc 2>/dev/null)" ]; then emit udc_present yes; else emit udc_present no; fi
is_active() { if [ "$(systemctl is-active "$1" 2>/dev/null || true)" = active ]; then echo yes; else echo no; fi; }
emit kvmd_active "$(is_active kvmd.service)"
emit kvmd_main_active "$(is_active kvmd-main.service)"
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def parse_facts(raw: bytes) -> dict[str, str]:
    facts = prototype.parse_facts(raw, set(FACTS_KEYS))
    for key in BOOLEAN_FACTS:
        if facts[key] not in {"yes", "no"}:
            raise ValueError("unexpected boolean value")
    name = facts["gadget_name"]
    if name != "none" and not GADGET_NAME.fullmatch(name):
        raise ValueError("unexpected gadget name")
    if facts["gadget_present"] == "yes" and name == "none":
        raise ValueError("gadget facts contradict gadget_present")
    if facts["gadget_present"] == "no" and name != "none":
        raise ValueError("gadget facts contradict gadget_present")
    return facts


def handoff_ready(facts: dict[str, str]) -> bool:
    return all(
        facts[key] == "yes"
        for key in ("kvmd_present", "vendor_present", "gadget_present", "udc_present")
    )


def policy_receipt() -> dict[str, bool]:
    return {
        "sysfsAndMountMetadataRead": True,
        "serviceStateCommandsInvoked": True,
        "credentialsRead": False,
        "mutationsPerformed": False,
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    result = prototype.run_ssh(target, secret, REMOTE_SCRIPT, timeout=40)
    result.check_returncode()
    facts = parse_facts(result.stdout)
    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-handoff-preflight",
        "facts": facts,
        "handoffReady": handoff_ready(facts),
        "policy": policy_receipt(),
    }
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    print(f"handoff_preflight={output} ready={receipt['handoffReady']} receiptSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
