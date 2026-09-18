#!/usr/bin/env python3
"""Run the boot-reliability gate on a test H616 board.

The gate records the boot identity, requests exactly one reboot through the
reviewed wrapper, waits for the device to return, and then proves that the
board comes back by itself: the boot identity changed, every product unit is
active, the HTTPS front answers and `/api/health` returns 200, no unit is in
the failed state, and the first journal records of the new boot survived.

`--verify-only` skips the reboot and records the current post-boot state, which
is useful after a manual power cycle. A reboot needs its own operator approval;
this tool never reboots without `--confirm-reboot`.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import time
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


BOOT_TOKEN_KEY = "boot_token"
PRODUCT_UNITS = (
    "hardened-blikvm-board-owner.service",
    "hardened-blikvm-board-server.service",
    "hardened-blikvm-boardd.service",
    "hardened-blikvm-ustreamer.service",
    "hardened-blikvm-board-video-proxy.service",
    "hardened-blikvm-nginx.service",
)
SERVICE_FACTS = {
    *(f"unit_{index}" for index in range(len(PRODUCT_UNITS))),
    "units_active",
    "failed_units",
    "https_status",
    "health_status",
    "journal_first_line",
    "kernel_records",
    "boot_token",
}
REBOOT_KEYS = {"reboot_requested", "reboot_command"}
FINAL_KEYS = {"cleanup_confirmed"}
DEFAULT_REBOOT_TIMEOUT_SECONDS = 1800
POLL_INTERVAL_SECONDS = 5.0


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    schemaVersion: int
    probe: str
    mode: str
    result: str
    bootIdentityChanged: bool
    deviceReachableAfterReboot: bool
    unitsActive: bool
    httpsHealthy: bool
    failedUnitsEmpty: bool
    earlyJournalPresent: bool
    pollAttempts: int
    failureStage: str
    failureType: str
    physicalActionsInvoked: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str
    timeout_seconds: int
    confirm_reboot: bool
    verify_only: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument(
        "--timeout-seconds", type=int, default=DEFAULT_REBOOT_TIMEOUT_SECONDS
    )
    _ = parser.add_argument(
        "--confirm-reboot",
        action="store_true",
        help="request the reviewed reboot; without it the gate stays read-only",
    )
    _ = parser.add_argument(
        "--verify-only",
        action="store_true",
        help="skip the reboot and only record the current post-boot state",
    )
    namespace = parser.parse_args()
    timeout_seconds = cast(int, namespace.timeout_seconds)
    if timeout_seconds <= 0:
        raise SystemExit("--timeout-seconds must be positive")
    if namespace.confirm_reboot and namespace.verify_only:
        raise SystemExit("--confirm-reboot and --verify-only are mutually exclusive")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        timeout_seconds=timeout_seconds,
        confirm_reboot=cast(bool, namespace.confirm_reboot),
        verify_only=cast(bool, namespace.verify_only),
    )


def _service_script() -> str:
    units = "\n".join(
        f"""if systemctl is-active {unit} >/dev/null 2>&1; then
  active=$((active + 1))
  printf 'unit_{index}=active\\n'
else
  printf 'unit_{index}=inactive\\n'
fi"""
        for index, unit in enumerate(PRODUCT_UNITS)
    )
    return (
        "active=0\n"
        + units
        + f"""
printf 'units_active=%s\\n' "$active"
failed=$(systemctl --failed --no-legend 2>/dev/null | wc -l | tr -d ' ')
printf 'failed_units=%s\\n' "$failed"
printf 'https_status=%s\\n' "$(curl -sk -o /dev/null -w '%{{http_code}}' --max-time 10 https://127.0.0.1/ 2>/dev/null || echo 000)"
printf 'health_status=%s\\n' "$(curl -sk -o /dev/null -w '%{{http_code}}' --max-time 10 https://127.0.0.1/api/health 2>/dev/null || echo 000)"
printf 'journal_first_line=%s\\n' "$(journalctl -b --no-pager -n +1 2>/dev/null | head -1 | tr -c 'A-Za-z0-9_.:,+/()=-' '_' | cut -c1-60)"
printf 'kernel_records=%s\\n' "$(journalctl -b -k --no-pager 2>/dev/null | wc -l | tr -d ' ')"
printf 'boot_token=%s\\n' "$(sha256sum /proc/sys/kernel/random/boot_id | cut -d ' ' -f 1)"
"""
    )


def build_service_script() -> str:
    return "set +e\n" + _service_script()


def build_reboot_script() -> str:
    return """set -eu
emit() { printf '%s=%s\\n' "$1" "$2"; }
if [ ! -x /usr/local/sbin/hardened-blikvm-reboot ]; then emit reboot_command missing; exit 1; fi
emit reboot_command present
nohup /usr/local/sbin/hardened-blikvm-reboot >/dev/null 2>&1 &
emit reboot_requested yes
"""


def build_final_script() -> str:
    return "set -eu\nprintf 'cleanup_confirmed=%s\\n' yes\n"


def validate_service_facts(facts: dict[str, str]) -> None:
    if facts.get("units_active") != str(len(PRODUCT_UNITS)):
        raise GateError("postflight", "product_unit_inactive")
    if facts.get("failed_units") != "0":
        raise GateError("postflight", "failed_unit_present")
    if facts.get("https_status") != "200":
        raise GateError("postflight", "https_unhealthy")
    if facts.get("health_status") != "200":
        raise GateError("postflight", "health_unhealthy")
    if not facts.get(BOOT_TOKEN_KEY):
        raise GateError("postflight", "boot_token_missing")
    if not facts.get("journal_first_line"):
        raise GateError("postflight", "journal_missing")
    try:
        kernel_records = int(facts.get("kernel_records", "0"))
    except ValueError as error:
        raise GateError("postflight", "kernel_records_invalid") from error
    if kernel_records <= 0:
        raise GateError("postflight", "kernel_records_missing")


def wait_for_return(
    target: prototype.RemoteTarget,
    secret: bytearray,
    previous_token: str,
    timeout_seconds: int,
) -> tuple[dict[str, str], int]:
    deadline = time.monotonic() + timeout_seconds
    attempts = 0
    reachable = False
    while time.monotonic() < deadline:
        time.sleep(POLL_INTERVAL_SECONDS)
        attempts += 1
        try:
            result = prototype.run_ssh(target, secret, build_service_script(), timeout=25)
        except (OSError, ValueError):
            continue
        if result.returncode != 0:
            continue
        reachable = True
        try:
            facts = prototype.parse_facts(result.stdout, SERVICE_FACTS)
        except ValueError:
            continue
        if facts.get(BOOT_TOKEN_KEY) and facts[BOOT_TOKEN_KEY] != previous_token:
            return facts, attempts
    if not reachable:
        raise GateError("reboot", "device_unreachable_after_reboot")
    raise GateError("reboot", "boot_identity_unchanged")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()

    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-boot-gate",
        "mode": "verify_only" if options.verify_only else "reboot",
        "result": "observed",
        "bootIdentityChanged": False,
        "deviceReachableAfterReboot": False,
        "unitsActive": False,
        "httpsHealthy": False,
        "failedUnitsEmpty": False,
        "earlyJournalPresent": False,
        "pollAttempts": 0,
        "failureStage": "",
        "failureType": "",
        "physicalActionsInvoked": False,
        "persistentChangeScope": "reboot" if options.confirm_reboot else "none",
    }
    stage = "preflight"
    postflight: dict[str, str] = {}
    attempts = 0
    try:
        preflight = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_service_script(), timeout=25).stdout,
            SERVICE_FACTS,
        )
        validate_service_facts(preflight)
        if not preflight.get(BOOT_TOKEN_KEY):
            raise GateError("preflight", "boot_token_missing")

        if options.verify_only:
            postflight = preflight
            receipt["deviceReachableAfterReboot"] = True
        else:
            stage = "reboot"
            if not options.confirm_reboot:
                raise GateError("reboot", "reboot_not_approved")
            reboot = prototype.parse_facts(
                prototype.run_ssh(target, secret, build_reboot_script(), timeout=25).stdout,
                REBOOT_KEYS,
            )
            if reboot.get("reboot_command") != "present":
                raise GateError("reboot", "reviewed_reboot_wrapper_missing")
            if reboot.get("reboot_requested") != "yes":
                raise GateError("reboot", "reboot_not_requested")
            stage = "postflight"
            postflight, attempts = wait_for_return(
                target, secret, preflight[BOOT_TOKEN_KEY], options.timeout_seconds
            )
            receipt["deviceReachableAfterReboot"] = True
            receipt["bootIdentityChanged"] = True
            validate_service_facts(postflight)

        cleanup = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_final_script(), timeout=25).stdout,
            FINAL_KEYS,
        )
        if cleanup.get("cleanup_confirmed") != "yes":
            raise GateError("receipt", "cleanup_unconfirmed")
    except GateError as error:
        receipt["result"] = "failed"
        receipt["failureStage"] = stage
        receipt["failureType"] = error.failure_type
        receipt["unitsActive"] = postflight.get("units_active") == str(len(PRODUCT_UNITS))
        receipt["httpsHealthy"] = postflight.get("https_status") == "200"
        receipt["failedUnitsEmpty"] = postflight.get("failed_units") == "0"
        receipt["earlyJournalPresent"] = bool(postflight.get("journal_first_line"))
        receipt["pollAttempts"] = attempts
        write_json_0600_atomic(output, receipt)
        return 1

    receipt["result"] = "passed"
    receipt["unitsActive"] = True
    receipt["httpsHealthy"] = True
    receipt["failedUnitsEmpty"] = True
    receipt["earlyJournalPresent"] = True
    receipt["pollAttempts"] = attempts
    if options.verify_only:
        receipt["result"] = "observed"
        receipt["bootIdentityChanged"] = False
        receipt["persistentChangeScope"] = "none"
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
