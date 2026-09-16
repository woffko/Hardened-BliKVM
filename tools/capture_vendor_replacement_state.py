#!/usr/bin/env python3
"""Capture the read-only vendor-replacement state of a hardened board.

This is the E5 summary capture: it proves from the device itself that our board
profile serves the main surface and that the retired vendor stack is gone -
our units active, every KVMD unit inactive and masked, the composite gadget
bound with its four functions, the MSD partition mounted at our images
directory with the reviewed image, both video sockets present, and the public
health endpoint answering with our platform identity.

Read-only: it opens no control surface, changes no unit and writes only the
private receipt. The result is `passed` only when every expectation holds.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
from typing import Any, Final

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


OUR_UNITS: Final[tuple[str, ...]] = (
    "hardened-blikvm-board-owner.service",
    "hardened-blikvm-board-server.service",
    "hardened-blikvm-nginx.service",
    "hardened-blikvm-ustreamer.service",
    "hardened-blikvm-board-video-proxy.service",
)
RETIRED_UNITS: Final[tuple[str, ...]] = (
    "kvmd.service",
    "kvmd-main.service",
    "kvmd-otg.service",
    "kvmd-webterm.service",
)
GADGET_FUNCTIONS: Final[tuple[str, ...]] = (
    "hid.usb0",
    "hid.usb1",
    "hid.usb2",
    "mass_storage.usb0",
)
IMAGES_DIR: Final = "/var/lib/hardened-blikvm-gadget/images"
USTREAMER_SOCKET: Final = "/run/hardened-blikvm-ustreamer/ustreamer.sock"
PROXY_SOCKET: Final = "/run/hardened-blikvm-video/mjpeg.sock"
MAX_RECEIPT_BYTES: Final = 64 * 1024


def build_script() -> str:
    units = " ".join(OUR_UNITS)
    retired = " ".join(RETIRED_UNITS)
    return f"""set +e
for u in {units}; do printf 'unit_%s=%s\\n' "$u" "$(systemctl is-active $u 2>/dev/null || true)"; done
for u in {retired}; do printf 'retired_%s_active=%s\\n' "$u" "$(systemctl is-active $u 2>/dev/null || true)"; printf 'retired_%s_enabled=%s\\n' "$u" "$(systemctl is-enabled $u 2>/dev/null || true)"; done
g=/sys/kernel/config/usb_gadget/hardened
if [ -d "$g" ]; then
  printf 'gadget_present=yes\\n'
  printf 'gadget_udc=%s\\n' "$(cat $g/UDC 2>/dev/null)"
  printf 'gadget_functions=%s\\n' "$(ls $g/functions 2>/dev/null | sort | tr '\\n' ',')"
else
  printf 'gadget_present=no\\n'
fi
printf 'msd_source=%s\\n' "$(findmnt -no SOURCE,TARGET {IMAGES_DIR} 2>/dev/null)"
printf 'image_bytes=%s\\n' "$(stat -c %s {IMAGES_DIR}/kubuntu.img 2>/dev/null || echo 0)"
printf 'ustreamer_socket=%s\\n' "$([ -S {USTREAMER_SOCKET} ] && echo yes || echo no)"
printf 'proxy_socket=%s\\n' "$([ -S {PROXY_SOCKET} ] && echo yes || echo no)"
printf 'health=%s\\n' "$(curl -sk --max-time 5 https://127.0.0.1/api/health 2>/dev/null)"
printf 'title=%s\\n' "$(curl -sk --max-time 5 https://127.0.0.1/ 2>/dev/null | grep -o '<title>[^<]*</title>' | head -1)"
"""


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line:
            key, value = line.split("=", 1)
            if re.fullmatch(r"[A-Za-z0-9_.-]+", key):
                facts[key] = value
    return facts


def evaluate(facts: dict[str, str]) -> tuple[bool, dict[str, bool]]:
    checks: dict[str, bool] = {}
    for unit in OUR_UNITS:
        checks[f"active:{unit}"] = facts.get(f"unit_{unit}") == "active"
    for unit in RETIRED_UNITS:
        checks[f"retired-inactive:{unit}"] = facts.get(f"retired_{unit}_active") == "inactive"
        checks[f"retired-masked:{unit}"] = facts.get(f"retired_{unit}_enabled") == "masked"
    functions = tuple(sorted((facts.get("gadget_functions") or "").strip(",").split(",")))
    checks["gadget"] = facts.get("gadget_present") == "yes" and functions == tuple(
        sorted(GADGET_FUNCTIONS)
    )
    checks["gadget-udc"] = bool(facts.get("gadget_udc"))
    checks["msd-mount"] = IMAGES_DIR in (facts.get("msd_source") or "")
    try:
        image_bytes = int(facts.get("image_bytes") or 0)
    except ValueError:
        image_bytes = 0
    checks["image"] = image_bytes > 0
    checks["ustreamer"] = facts.get("ustreamer_socket") == "yes"
    checks["proxy"] = facts.get("proxy_socket") == "yes"
    health = facts.get("health") or ""
    checks["health"] = '"status":"ok"' in health and "bli-h616-pikvm" in health
    checks["gui-title"] = "Hardened BliKVM" in (facts.get("title") or "")
    return all(checks.values()), checks


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--timeout", type=int, default=120)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    output = prototype.validate_output_path(args.output)
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, args.username, Path(args.known_hosts)
    )
    secret = prototype.read_secret_stdin()
    result = prototype.run_ssh(target, secret, build_script(), timeout=args.timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    facts = parse_facts(stdout)
    passed, checks = evaluate(facts)

    receipt: dict[str, Any] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-vendor-replacement",
        "result": "passed" if passed else "inspect",
        "checks": checks,
        "facts": facts,
        "policy": {
            "readOnly": True,
            "mutationsPerformed": False,
            "receiptContainsSecrets": False,
        },
    }
    if len(json.dumps(receipt).encode()) > MAX_RECEIPT_BYTES:
        raise SystemExit("receipt is too large")
    write_json_0600_atomic(output, receipt)
    print(f"vendor_replacement={output} result={receipt['result']}")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
