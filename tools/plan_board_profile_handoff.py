#!/usr/bin/env python3
"""Produce the board-owner handoff plan without touching the device.

Validates the verified bundle, the operator ATX timings and every device path
against an allowlist, then writes the exact owner/server environment and the
ordered gate steps into a sanitized private receipt. It performs no mutation; the
operator (or a later, device-tested runner) executes the plan.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from typing import Any
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import verify_board_profile_bundle as bundle_verifier


MAX_TIMING_MS = 10_000
# ATX pulse durations sourced read-only from the vendor helper on this board:
# /usr/bin/atx (sha256
# a7f0b80fce3387b903537f62c3acdd7bd10bcb6a6159cd22eb90430abf168d21) sleeps
# 0x7a120 us (500 ms) for power_on, 0x4c4b40 us (5000 ms) for power_off and
# 0x7a120 us (500 ms) for power_reset. Receipt:
# evidence/private/device-vendor-atx-timings-20260915-a.json. Override on the
# command line after a reviewed bench measurement.
VENDOR_ATX_POWER_SHORT_MS = 500
VENDOR_ATX_POWER_LONG_MS = 5_000
VENDOR_ATX_RESET_MS = 500

UDC = re.compile(r"^[A-Za-z0-9._:-]{1,64}$")
GPIOCHIP = re.compile(r"^/dev/gpiochip[0-9]{1,3}$")
CONFIGFS_ROOT = "/sys/kernel/config/usb_gadget"
HIDG_DIR = "/dev"
GADGET_STATE = "/var/lib/hardened-blikvm-gadget"
GADGET_ETC = "/etc/hardened-blikvm-gadget"
GADGET_RUN = "/run/hardened-blikvm-gadget"
BUNDLE_ROOT = "hardened-blikvm-board-profile-aarch64"
GATE_ROOT = "/var/lib/hardened-blikvm-gadget/gate"

STEPS = [
    "run tools/capture_device_handoff_preflight.py and require handoffReady=true",
    "run tools/capture_device_hid_descriptors.py and pin the report descriptors",
    "copy the verified bundle to /var/lib/hardened-blikvm-gadget/gate/bundle.tar.gz and write the operator ATX environment",
    "start the board owner with the environment below and wait for the control socket",
    "verify media select/attach/eject and a keyboard report (the gate can wait for the composite gadget with --accept-media-hid)",
    "run the bundled native ATX bench with --accept-atx-bench --confirm-physical-disconnect while Power and Reset are unplugged",
    "tear down the runtime directory and restore the KVMD route on any failure",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--udc", required=True)
    _ = parser.add_argument("--power-short-ms", default=VENDOR_ATX_POWER_SHORT_MS, type=int)
    _ = parser.add_argument("--power-long-ms", default=VENDOR_ATX_POWER_LONG_MS, type=int)
    _ = parser.add_argument("--reset-ms", default=VENDOR_ATX_RESET_MS, type=int)
    _ = parser.add_argument("--configfs-root", default=CONFIGFS_ROOT)
    _ = parser.add_argument("--images-dir", default=f"{GADGET_STATE}/images")
    _ = parser.add_argument("--catalog", default=f"{GADGET_ETC}/catalog.json")
    _ = parser.add_argument("--serial-file", default=f"{GADGET_ETC}/serial")
    _ = parser.add_argument("--atx-journal", default=f"{GADGET_STATE}/atx-journal.json")
    _ = parser.add_argument("--hid-ledger", default=f"{GADGET_STATE}/hid-ledger.json")
    _ = parser.add_argument("--control-socket", default=f"{GADGET_RUN}/control.sock")
    _ = parser.add_argument("--atx-chip", default="/dev/gpiochip0")
    _ = parser.add_argument("--hidg-dir", default=HIDG_DIR)
    _ = parser.add_argument("--reconciliations", default="")
    return parser.parse_args()


def _require_under(value: str, prefixes: tuple[str, ...], label: str) -> str:
    path = Path(value)
    if not path.is_absolute() or not any(str(path).startswith(prefix) for prefix in prefixes):
        raise ValueError(f"{label} is outside the reviewed paths")
    return value


def _timing(value: int, label: str) -> int:
    if not 1 <= value <= MAX_TIMING_MS:
        raise ValueError(f"{label} is out of range")
    return value


def build_plan(options: argparse.Namespace, bundle_sha256: str) -> dict[str, Any]:
    if options.configfs_root != CONFIGFS_ROOT:
        raise ValueError("unexpected configfs root")
    if options.hidg_dir != HIDG_DIR:
        raise ValueError("unexpected HID character-device directory")
    if not UDC.fullmatch(options.udc):
        raise ValueError("invalid UDC name")
    if not GPIOCHIP.fullmatch(options.atx_chip):
        raise ValueError("invalid ATX GPIO chip path")

    images = _require_under(options.images_dir, (GADGET_STATE,), "images directory")
    catalog = _require_under(options.catalog, (GADGET_ETC,), "catalog")
    serial = _require_under(options.serial_file, (GADGET_ETC,), "serial file")
    atx_journal = _require_under(options.atx_journal, (GADGET_STATE,), "ATX journal")
    hid_ledger = _require_under(options.hid_ledger, (GADGET_STATE,), "HID ledger")
    control_socket = _require_under(options.control_socket, (GADGET_RUN,), "control socket")
    if options.reconciliations:
        _ = _require_under(options.reconciliations, (GADGET_ETC,), "reconciliations")

    power_short = _timing(options.power_short_ms, "power_short_ms")
    power_long = _timing(options.power_long_ms, "power_long_ms")
    reset = _timing(options.reset_ms, "reset_ms")

    owner_environment = {
        "BLI_KVM_CONTROL_SOCKET": control_socket,
        "BLI_KVM_BOARD_PROFILE": "1",
        "BLI_KVM_GADGET_MEDIA_JOURNAL": atx_journal.replace("atx-journal", "media-journal"),
        "BLI_KVM_GADGET_MEDIA_CATALOG": catalog,
        "BLI_KVM_GADGET_CONFIGFS": options.configfs_root,
        "BLI_KVM_GADGET_IMAGES": images,
        "BLI_KVM_GADGET_UDC": options.udc,
        "BLI_KVM_GADGET_SERIAL_FILE": serial,
        "BLI_KVM_GADGET_HID_LEDGER": hid_ledger,
        "BLI_KVM_GADGET_HIDG_DIR": options.hidg_dir,
        "BLI_KVM_ATX_JOURNAL": atx_journal,
        "BLI_KVM_ATX_CHIP": options.atx_chip,
        "BLI_KVM_ATX_POWER_SHORT_MS": str(power_short),
        "BLI_KVM_ATX_POWER_LONG_MS": str(power_long),
        "BLI_KVM_ATX_RESET_MS": str(reset),
    }
    atx_environment_file = {
        "BLI_KVM_ATX_POWER_SHORT_MS": str(power_short),
        "BLI_KVM_ATX_POWER_LONG_MS": str(power_long),
        "BLI_KVM_ATX_RESET_MS": str(reset),
    }
    if options.reconciliations:
        atx_environment_file["BLI_KVM_ATX_RECONCILIATIONS"] = options.reconciliations

    return {
        "schemaVersion": 1,
        "plan": "hardened-blikvm-board-profile-handoff",
        "bundle": {"sha256": bundle_sha256, "profile": bundle_verifier.EXPECTED_PROFILE},
        "device": {
            "configfsRoot": options.configfs_root,
            "udc": options.udc,
            "atxChip": options.atx_chip,
            "hidgDir": options.hidg_dir,
        },
        "steps": STEPS,
        "ownerEnvironment": owner_environment,
        "atxEnvironmentFile": atx_environment_file,
        "policy": {"mutationsPerformed": False, "secretsRead": False},
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    bundle = Path(args.bundle).resolve(strict=True)
    verified = bundle_verifier.verify_bundle(bundle, "")
    plan = build_plan(args, str(verified["sha256"]))
    serialized = json.dumps(plan, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, plan)
    bundle_info = plan["bundle"]
    print(f"board_handoff_plan={output} bundleSha256={bundle_info['sha256']} planSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError) as error:
        _ = error
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
