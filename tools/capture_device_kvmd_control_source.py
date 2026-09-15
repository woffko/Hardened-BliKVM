#!/usr/bin/env python3
"""Capture the board's KVMD control semantics and vendor ATX timings, read-only.

The probe runs a fixed read-only remote script that hashes the pinned KVMD
sources and the vendor ATX helper, prints the exact absolute-mouse remapping and
report packing, reads the configured ATX lines, and disassembles only the small
address range of the vendor helper that holds the three pulse sequences. It
performs no mutation and reads no credential, image content or serial number.

Every value is validated against pinned expectations: a source or binary that
does not match the recorded revision is reported as unverified instead of
guessed, so a device change can never silently alter the recorded semantics.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


KVMD_PACKAGE = "/usr/lib/python3/dist-packages/kvmd"
KVMD_OTG = f"{KVMD_PACKAGE}/plugins/hid/otg"

EXPECTED_SOURCE_HASHES = {
    f"{KVMD_PACKAGE}/tools.py": "9af6585948a4dd1c2c25f1e8426a59b632e37074d8b0fe508ca8c899c6a5983f",
    f"{KVMD_PACKAGE}/mouse.py": "d7c9a7a0385fe7f230fcf1834d49fd107ffaacace5e4b2b5d6e2bdd597fa80da",
    f"{KVMD_OTG}/events.py": "323bb171236eaf8d7f1fe1d2c55d4a4cd409c981038b65b37963c8fced0a1ab9",
    f"{KVMD_OTG}/mouse.py": "b04366f5905d125f9944f8211bf18b23ca46e8edde24ef3655c7a37b5c833f28",
    "/etc/kvmd/atx.yaml": "3a2c49f834898fce40a12194cfbb96ea703d83a490f7ab84dc9effcbc0ba9ed2",
    "/etc/kvmd/override.yaml": "771943938d93e7c59cc045dbf6fa4ad33e6befd9509f3917a5ab47d159fe4a83",
    "/usr/bin/atx": "a7f0b80fce3387b903537f62c3acdd7bd10bcb6a6159cd22eb90430abf168d21",
}

EXPECTED_REMAP = "returnint((value-in_min)*(out_max-out_min)//(in_max-in_min)+out_min)"
EXPECTED_PACK = '("<BHHbb"ifabsoluteelse"<Bbbbb")'
EXPECTED_MOUSE_RANGE = {"min": -32768, "max": 32767}
EXPECTED_ATX_PINS = {
    "power_switch_pin": 228,
    "reset_switch_pin": 272,
    "power_led_pin": 234,
    "hdd_led_pin": 233,
}
# usleep call site and the expected pulse length, in order.
EXPECTED_PULSES = (
    ("power_short", "0x17d4", 500),
    ("power_long", "0x1818", 5000),
    ("reset", "0x185c", 500),
)
DISASSEMBLY_START = "0x1760"
DISASSEMBLY_STOP = "0x18a0"
MAX_PROBE_BYTES = 262_144
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
DIS_LINE = re.compile(r"^\s*([0-9a-f]+):\s+(.*?)\s*$")
IMMEDIATE = re.compile(r"mov\s+w0,\s+#(0x[0-9a-f]+)")
MOVK = re.compile(r"movk\s+w0,\s+#(0x[0-9a-f]+),\s*lsl\s+#16")

REMOTE_PROBE = rf"""set -eu
pkg={KVMD_PACKAGE}
otg={KVMD_OTG}
sha() {{ sha256sum "$1" 2>/dev/null | cut -d' ' -f1 || echo missing; }}
printf 'file:%s:%s\n' "$(sha "$pkg/tools.py")" "$pkg/tools.py"
printf 'file:%s:%s\n' "$(sha "$pkg/mouse.py")" "$pkg/mouse.py"
printf 'file:%s:%s\n' "$(sha "$otg/events.py")" "$otg/events.py"
printf 'file:%s:%s\n' "$(sha "$otg/mouse.py")" "$otg/mouse.py"
printf 'file:%s:%s\n' "$(sha /etc/kvmd/atx.yaml)" "/etc/kvmd/atx.yaml"
printf 'file:%s:%s\n' "$(sha /etc/kvmd/override.yaml)" "/etc/kvmd/override.yaml"
printf 'file:%s:%s\n' "$(sha /usr/bin/atx)" "/usr/bin/atx"
printf 'remap:%s\n' "$(grep -m1 'return int((value - in_min)' "$pkg/tools.py" | tr -d ' \t')"
printf 'pack:%s\n' "$(grep -m1 -o '("<[^"]*" if absolute else "<[^"]*")' "$otg/events.py" | tr -d ' \t')"
printf 'mouserange:%s:%s\n' \
  "$(grep -m1 -oE 'MIN = -?[0-9]+' "$pkg/mouse.py" | grep -oE -- '-?[0-9]+$')" \
  "$(grep -m1 -oE 'MAX = -?[0-9]+' "$pkg/mouse.py" | grep -oE -- '-?[0-9]+$')"
printf 'kvmd:%s\n' "$(grep -m1 -oE '^ *type: [a-z]+' /etc/kvmd/override.yaml | tr -d ' ' | cut -d: -f2)"
printf 'chip:%s\n' "$(grep -m1 -oE 'device: /dev/gpiochip[0-9]+' /etc/kvmd/override.yaml | tr -d ' ' | cut -d: -f2)"
for pin in power_switch_pin reset_switch_pin power_led_pin hdd_led_pin; do
  printf 'pin:%s:%s\n' "$pin" "$(grep -m1 -oE "^ *$pin: [0-9]+" /etc/kvmd/override.yaml | tr -d ' ' | cut -d: -f2)"
done
od=$(command -v aarch64-linux-gnu-objdump || command -v objdump)
printf 'objdump:%s\n' "${{od:-missing}}"
if [ -n "$od" ]; then
  "$od" -d --no-show-raw-insn \
    --start-address={DISASSEMBLY_START} --stop-address={DISASSEMBLY_STOP} \
    /usr/bin/atx 2>/dev/null | grep -E '^ +[0-9a-f]+:' | while IFS= read -r line; do
      printf 'dis:%s\n' "$line"
    done
fi
"""


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def _pulse_durations(
    dis: dict[str, str], expected_binary: bool
) -> dict[str, Any]:
    """Derive the three pulse lengths from the pinned usleep call sites."""
    durations: dict[str, Any] = {}
    if not expected_binary:
        return {"verified": False, "reason": "vendor binary hash differs"}
    immediate = 0
    upper = 0
    for address, instruction in sorted(dis.items(), key=lambda item: int(item[0], 16)):
        found = IMMEDIATE.match(instruction)
        if found:
            immediate = int(found.group(1), 16)
            upper = 0
            continue
        found = MOVK.match(instruction)
        if found:
            upper = int(found.group(1), 16)
            continue
        if "usleep@plt" not in instruction:
            continue
        microseconds = immediate + (upper << 16)
        for action, call_site, expected_ms in EXPECTED_PULSES:
            if int(address, 16) == int(call_site, 16):
                if microseconds // 1000 != expected_ms:
                    return {
                        "verified": False,
                        "reason": f"{action} pulse {microseconds} us differs from the pinned value",
                    }
                durations[action] = {"callSite": f"0x{address}", "microseconds": microseconds,
                                     "durationMs": expected_ms}
    if len(durations) != len(EXPECTED_PULSES):
        return {"verified": False, "reason": "a pinned usleep call site is missing"}
    return {"verified": True, "pulses": [durations[action] for action, _, _ in EXPECTED_PULSES]}


def parse_probe(raw: bytes) -> dict[str, Any]:
    """Parse the canonical probe output into a bounded, validated fact set."""
    text = raw.decode("ascii", errors="strict")
    if len(raw) > MAX_PROBE_BYTES:
        raise ValueError("probe output exceeded the size limit")

    hashes: dict[str, str] = {}
    dis: dict[str, str] = {}
    remap = pack = ""
    mouse_range: dict[str, int] = {}
    atx_type = chip = ""
    pins: dict[str, int] = {}

    for line in text.splitlines():
        if line.startswith("file:"):
            _, digest, path = line.split(":", 2)
            if not HEX_SHA256.fullmatch(digest) or path in hashes:
                raise ValueError("invalid or duplicate source hash")
            hashes[path] = digest
        elif line.startswith("remap:"):
            remap = line.removeprefix("remap:")
        elif line.startswith("pack:"):
            pack = line.removeprefix("pack:")
        elif line.startswith("mouserange:"):
            _, minimum, maximum = line.split(":")
            mouse_range = {"min": int(minimum), "max": int(maximum)}
        elif line.startswith("kvmd:"):
            atx_type = line.removeprefix("kvmd:")
        elif line.startswith("chip:"):
            chip = line.removeprefix("chip:")
        elif line.startswith("pin:"):
            _, name, value = line.split(":")
            pins[name] = int(value)
        elif line.startswith("dis:"):
            found = DIS_LINE.match(line.removeprefix("dis:"))
            if not found:
                raise ValueError("malformed disassembly line")
            address, instruction = found.group(1), found.group(2)
            if address in dis:
                raise ValueError("duplicate disassembly address")
            dis[address] = instruction
        elif line.startswith("objdump:"):
            continue
        else:
            raise ValueError("unexpected probe line")

    expected_binary = hashes.get("/usr/bin/atx") == EXPECTED_SOURCE_HASHES["/usr/bin/atx"]
    durations = _pulse_durations(dis, expected_binary)
    if not durations["verified"]:
        raise ValueError(f"ATX pulse evidence is not reproducible: {durations['reason']}")

    facts: dict[str, Any] = {
        "capturedAt": "unverified-date",
        "sourceHashes": [
            {"path": path, "sha256": hashes.get(path, "missing")} for path in EXPECTED_SOURCE_HASHES
        ],
        "sourceRevisionMatches": all(
            hashes.get(path) == digest for path, digest in EXPECTED_SOURCE_HASHES.items()
        ),
        "absoluteRemap": {
            "formula": remap,
            "matchesPinned": remap == EXPECTED_REMAP,
            "inMin": mouse_range.get("min"),
            "inMax": mouse_range.get("max"),
            "outMin": 0,
            "outMax": 32767,
        },
        "reportFormats": {
            "packExpression": pack,
            "matchesPinned": pack == EXPECTED_PACK,
            "absolute": {"function": "hid.usb1", "reportLength": 7, "format": "<BHHbb"},
            "relative": {"function": "hid.usb2", "reportLength": 5, "format": "<Bbbbb"},
        },
        "mouseRange": mouse_range,
        "atx": {
            "configuredType": atx_type,
            "device": chip,
            "pins": pins,
            "pinsMatchPinned": pins == EXPECTED_ATX_PINS,
            "pulses": durations["pulses"],
        },
    }
    return facts


def policy_receipt() -> dict[str, bool]:
    return {
        "kvmdSourcesRead": True,
        "atxConfigurationRead": True,
        "vendorBinaryDisassembled": True,
        "devicePinsToggled": False,
        "credentialsRead": False,
        "identifiersRecorded": False,
        "mutationsPerformed": False,
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    result = prototype.run_ssh(target, secret, REMOTE_PROBE, timeout=60)
    result.check_returncode()
    facts = parse_probe(result.stdout)
    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-kvmd-control-source",
        "facts": facts,
        "policy": policy_receipt(),
    }
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    print(f"control-source={output} receiptSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
