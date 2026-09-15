#!/usr/bin/env python3
"""Capture the HID function parameters and report descriptors, read-only.

The probe runs a fixed read-only remote script, parses the canonical lines and
writes a sanitized private receipt holding the exact per-function parameters
and report descriptors (hex). It performs no mutation and reads no credential. A
missing gadget is reported as `gadget_present=no` rather than an error so the
probe is safe to run before the reviewed handoff.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


MAX_HID_FUNCTIONS = 8
MAX_REPORT_DESCRIPTOR_BYTES = 4096
NAME = re.compile(r"^hid\.[a-z0-9]{1,16}$")
GADGET_NAME = re.compile(r"^[a-z][a-z0-9_-]{0,31}$")
HEX = re.compile(r"^[0-9a-f]*$")
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")

REMOTE_SCRIPT = r"""set -eu
gdir=/sys/kernel/config/usb_gadget
gname=$(ls "$gdir" 2>/dev/null | head -1 || true)
if [ -z "$gname" ]; then
  printf 'gadget_present=no\n'
  exit 0
fi
printf 'gadget_present=yes\n'
printf 'gadget=%s\n' "$gname"
g="$gdir/$gname"
for path in "$g"/functions/hid.*; do
  [ -d "$path" ] || continue
  name=$(basename "$path")
  protocol=$(cat "$path/protocol" 2>/dev/null | tr -d '\n' || true)
  subclass=$(cat "$path/subclass" 2>/dev/null | tr -d '\n' || true)
  length=$(cat "$path/report_length" 2>/dev/null | tr -d '\n' || true)
  desc=$(od -An -v -tx1 "$path/report_desc" 2>/dev/null | tr -d ' \n' || true)
  sha=$(sha256sum "$path/report_desc" 2>/dev/null | cut -d ' ' -f 1 || true)
  printf 'hid:%s:%s:%s:%s:%s:%s\n' "$name" "$protocol" "$subclass" "$length" "$desc" "$sha"
done
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def parse_probe(raw: bytes) -> dict[str, object]:
    """Parse the canonical probe output into a bounded, validated receipt."""
    text = raw.decode("ascii", errors="strict")
    if len(raw) > 1_048_576:
        raise ValueError("probe output exceeded the size limit")

    present: bool | None = None
    gadget = ""
    functions: list[dict[str, object]] = []

    for line in text.splitlines():
        if line == "gadget_present=yes":
            if present is not None:
                raise ValueError("duplicate gadget_present")
            present = True
            continue
        if line == "gadget_present=no":
            if present is not None:
                raise ValueError("duplicate gadget_present")
            present = False
            continue
        if line.startswith("gadget="):
            value = line.removeprefix("gadget=")
            if not GADGET_NAME.fullmatch(value):
                raise ValueError("invalid gadget name")
            gadget = value
            continue
        if line.startswith("hid:"):
            if len(functions) >= MAX_HID_FUNCTIONS:
                raise ValueError("too many HID functions")
            parts = line.split(":")
            if len(parts) != 7:
                raise ValueError("malformed HID line")
            _, name, protocol, subclass, length, desc, sha = parts
            if not NAME.fullmatch(name) or any(
                item["name"] == name for item in functions
            ):
                raise ValueError("invalid or duplicate HID function")
            for field, value in (("protocol", protocol), ("subclass", subclass)):
                if not value.isdigit() or not 0 <= int(value) <= 255:
                    raise ValueError(f"invalid {field}")
            if not length.isdigit() or not 1 <= int(length) <= 1024:
                raise ValueError("invalid report length")
            if (
                len(desc) % 2 != 0
                or not HEX.fullmatch(desc)
                or len(desc) // 2 > MAX_REPORT_DESCRIPTOR_BYTES
            ):
                raise ValueError("invalid report descriptor")
            if not HEX_SHA256.fullmatch(sha):
                raise ValueError("invalid report descriptor hash")
            computed = hashlib.sha256(bytes.fromhex(desc)).hexdigest()
            if computed != sha:
                raise ValueError("report descriptor hash mismatch")
            functions.append(
                {
                    "name": name,
                    "protocol": int(protocol),
                    "subclass": int(subclass),
                    "reportLength": int(length),
                    "reportDescriptorHex": desc,
                    "reportDescriptorSha256": sha,
                }
            )
            continue
        if line.strip() == "":
            continue
        raise ValueError("unexpected probe line")

    if present is None:
        raise ValueError("missing gadget_present")
    if present and not gadget:
        raise ValueError("missing gadget name")
    if not present and (gadget or functions):
        raise ValueError("gadget facts contradict gadget_present")

    return {"gadgetPresent": present, "gadget": gadget, "functions": functions}


def policy_receipt() -> dict[str, bool]:
    return {
        "sysfsGadgetMetadataRead": True,
        "reportDescriptorsRead": True,
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
    facts = parse_probe(result.stdout)
    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-hid-descriptors",
        "facts": facts,
        "policy": policy_receipt(),
    }
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    count = len(cast(list, facts["functions"]))
    print(f"hid_descriptors={output} functions={count} receiptSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
