#!/usr/bin/env python3
"""Stage and run the E1 storage reboot gate.

Plan-only by default: it records the read-only state the reboot must preserve
(the reviewed MSD partition mounted at our images directory through the fstab
entry, the tmpfiles-created directory modes and owners, and the catalogued
image). `--confirm-reboot` asks the board to reboot once and, after it returns,
records the same facts plus the new boot identity; the gate passes only when
every preserved fact still holds.

The reboot step is never implicit and needs the operator's explicit approval in
the current session. The SSH secret is read once from stdin.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
import time
from typing import Any, Final

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


IMAGES_DIR: Final = "/var/lib/hardened-blikvm-gadget/images"
GADGET_DIR: Final = "/var/lib/hardened-blikvm-gadget"
PARTITION_UUID: Final = "f44e42ea-ba9e-4cbc-8783-f85df176642e"
IMAGE_NAME: Final = "kubuntu.img"
IMAGE_BYTES: Final = 6841958400
MAX_RECEIPT_BYTES: Final = 64 * 1024


def probe_script() -> str:
    return f"""set -u
printf 'boot_id=%s\\n' "$(tr -d '\\n' < /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
printf 'fstab_entries=%s\\n' "$(grep -c '{PARTITION_UUID}' /etc/fstab 2>/dev/null || true)"
printf 'mount_source=%s\\n' "$(findmnt -n -o SOURCE {IMAGES_DIR} 2>/dev/null || true)"
printf 'mount_target=%s\\n' "$(findmnt -n -o TARGET {IMAGES_DIR} 2>/dev/null || true)"
printf 'mount_fstype=%s\\n' "$(findmnt -n -o FSTYPE {IMAGES_DIR} 2>/dev/null || true)"
printf 'mount_options=%s\\n' "$(findmnt -n -o OPTIONS {IMAGES_DIR} 2>/dev/null || true)"
printf 'tmpfiles_entries=%s\\n' "$(systemd-tmpfiles --cat-config 2>/dev/null | grep -c 'hardened-blikvm-gadget' || true)"
printf 'gadget_dir=%s\\n' "$(stat -c '%a %U %G' {GADGET_DIR} 2>/dev/null || true)"
printf 'images_dir=%s\\n' "$(stat -c '%a %U %G' {IMAGES_DIR} 2>/dev/null || true)"
printf 'image_present=%s\\n' "$([ -f {IMAGES_DIR}/{IMAGE_NAME} ] && echo yes || echo no)"
printf 'image_bytes=%s\\n' "$(stat -c %s {IMAGES_DIR}/{IMAGE_NAME} 2>/dev/null || echo 0)"
printf 'failed_units=%s\\n' "$(systemctl list-units --state=failed --plain --no-legend 2>/dev/null | wc -l)"
printf 'owner_unit=%s\\n' "$(systemctl is-active hardened-blikvm-board-owner.service 2>/dev/null || true)"
printf 'server_unit=%s\\n' "$(systemctl is-active hardened-blikvm-board-server.service 2>/dev/null || true)"
"""


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line and not line.startswith(" "):
            key, value = line.split("=", 1)
            if re.fullmatch(r"[A-Za-z0-9_]+", key):
                facts[key] = value
    return facts


def preserved_facts(facts: dict[str, str]) -> dict[str, bool]:
    """The facts the reboot must preserve, evaluated from one capture."""
    options = facts.get("mount_options", "")
    return {
        "fstabEntry": facts.get("fstab_entries", "0") == "1",
        "mountedAtImages": facts.get("mount_target") == IMAGES_DIR,
        "partitionSource": facts.get("mount_source") == "/dev/mmcblk0p4",
        "ext4": facts.get("mount_fstype") == "ext4",
        "readWrite": "rw" in options.split(","),
        "noExec": "noexec" in options.split(","),
        "tmpfiles": facts.get("tmpfiles_entries", "0") != "0",
        "gadgetDir": facts.get("gadget_dir") == "750 root hardened-blikvm",
        "imagesDir": facts.get("images_dir") == "770 root hardened-blikvm",
        "image": facts.get("image_present") == "yes"
        and facts.get("image_bytes") == str(IMAGE_BYTES),
        "noFailedUnits": facts.get("failed_units") == "0",
    }


def decide_outcome(
    *, confirm_reboot: bool, before: dict[str, str], after: dict[str, str]
) -> str:
    if not confirm_reboot:
        return "plan"
    if not after:
        return "inspect"
    if after.get("boot_id") == before.get("boot_id"):
        return "inspect"
    preserved = preserved_facts(after)
    if all(preserved.values()) and after.get("owner_unit") == "active":
        return "passed"
    return "inspect"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--confirm-reboot", action="store_true")
    _ = parser.add_argument(
        "--reboot-command", default="/usr/bin/systemctl reboot"
    )
    _ = parser.add_argument("--timeout", type=int, default=120)
    _ = parser.add_argument("--boot-wait", type=int, default=180)
    return parser.parse_args()


def wait_for_return(target: prototype.RemoteTarget, secret: bytearray, seconds: int) -> bool:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        time.sleep(10)
        result = prototype.run_ssh(target, secret, "true", timeout=20)
        if result.returncode == 0:
            return True
    return False


def main() -> int:
    args = parse_args()
    output: Path = prototype.validate_output_path(args.output)
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    target = prototype.RemoteTarget(host, ssh_port, args.username, Path(args.known_hosts))
    secret = prototype.read_secret_stdin()

    before_result = prototype.run_ssh(target, secret, probe_script(), timeout=args.timeout)
    before = parse_facts((before_result.stdout or b"").decode("utf-8", "replace"))
    print("\n".join(f"{key}={value}" for key, value in sorted(before.items())))
    after: dict[str, str] = {}
    if args.confirm_reboot:
        if not all(preserved_facts(before).values()):
            raise SystemExit("the reviewed pre-reboot state is not intact")
        boot_before = before.get("boot_id", "")
        reboot = prototype.run_ssh(
            target, secret, f"{args.reboot_command} || true", timeout=30
        )
        if reboot.returncode != 0:
            raise SystemExit("the reboot command did not run")
        if not wait_for_return(target, secret, args.boot_wait):
            raise SystemExit("the board did not return inside the wait window")
        after_result = prototype.run_ssh(target, secret, probe_script(), timeout=args.timeout)
        after = parse_facts((after_result.stdout or b"").decode("utf-8", "replace"))
        print("\n".join(f"after_{key}={value}" for key, value in sorted(after.items())))
        if after.get("boot_id") == boot_before:
            raise SystemExit("the boot identity did not change")

    receipt: dict[str, Any] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-storage-reboot-gate",
        "mode": "reboot" if args.confirm_reboot else "plan",
        "result": decide_outcome(
            confirm_reboot=args.confirm_reboot, before=before, after=after
        ),
        "before": before,
        "after": after,
        "preservedBefore": preserved_facts(before),
        "preservedAfter": preserved_facts(after) if after else {},
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": bool(args.confirm_reboot),
            "receiptContainsSecrets": False,
        },
    }
    encoded = json.dumps(receipt).encode()
    if len(encoded) > MAX_RECEIPT_BYTES:
        raise SystemExit("receipt is too large")
    write_json_0600_atomic(output, receipt)
    print(f"storage_reboot_gate={output} mode={receipt['mode']} result={receipt['result']}")
    return 0 if receipt["result"] in ("plan", "passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
