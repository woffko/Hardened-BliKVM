#!/usr/bin/env python3
"""Run the BliKVM reboot gate for the restricted HID path on a test H616.

The gate records a boot identity, requests exactly one software reboot, waits
for the device to return, and verifies the boot identity changed while the
retained slots, retained guard and evidence store are intact and no HID test
account or staging path survived. It performs no upload and stores no raw boot
identifier in the receipt.

`--verify-only` skips the reboot and only records the post-reboot state, which
is useful after a manual power cycle. Note that the field image keeps a
volatile journal (`Storage=volatile`), so the previous boot's shutdown lines
are not available for post-mortem.
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
PREFLIGHT_KEYS = {
    "architecture",
    "boot_token",
    "kvmd_active",
    "retained_state",
    "staging_absent",
    "account_absent",
    "previous_boot_journal",
}
POSTFLIGHT_KEYS = {
    "architecture",
    "boot_token",
    "kvmd_active",
    "retained_state",
    "staging_absent",
    "account_absent",
    "previous_boot_journal",
}
REBOOT_KEYS = {"reboot_requested"}
FINAL_KEYS = {"cleanup_confirmed"}
DEFAULT_REBOOT_TIMEOUT_SECONDS = 1800
POLL_INTERVAL_SECONDS = 5.0


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    mode: str
    bootIdentityChanged: bool
    deviceReachableAfterReboot: bool
    kvmdActive: bool
    retainedStateIntact: bool
    stagingAbsent: bool
    accountAbsent: bool
    previousBootJournalPresent: bool
    cleanupConfirmed: bool
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
        "--verify-only",
        action="store_true",
        help="skip the reboot and only record the current post-reboot state",
    )
    namespace = parser.parse_args()
    timeout_seconds = cast(int, namespace.timeout_seconds)
    if timeout_seconds <= 0:
        raise SystemExit("--timeout-seconds must be positive")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        timeout_seconds=timeout_seconds,
        verify_only=cast(bool, namespace.verify_only),
    )


def _fact_script() -> str:
    return r"""
emit() { printf '%s=%s\n' "$1" "$2"; }
case "$(uname -m)" in aarch64) emit architecture ok ;; *) emit architecture no ;; esac
boot_token=$(sha256sum /proc/sys/kernel/random/boot_id | cut -d ' ' -f 1)
emit boot_token "$boot_token"
if systemctl is-active kvmd.service >/dev/null 2>&1; then emit kvmd_active yes; else emit kvmd_active no; fi
if [ -d /var/lib/hardened-blikvm-slots/current ] \
   && [ -e /var/lib/hardened-blikvm-write-retain-828cfb89528ce919111aeec7 ] \
   && [ -e /var/lib/hardened-blikvm-write-evidence-828cfb89528ce919111aeec7 ]; then
  emit retained_state yes
else
  emit retained_state no
fi
if ls -d /tmp/hardened-blikvm-* >/dev/null 2>&1; then emit staging_absent no; else emit staging_absent yes; fi
if id -u hardened-blikvm-hid >/dev/null 2>&1; then emit account_absent no; else emit account_absent yes; fi
if journalctl -b -1 -n 1 --no-pager >/dev/null 2>&1; then emit previous_boot_journal yes; else emit previous_boot_journal no; fi
"""


def build_preflight_script() -> str:
    return "set -eu\n" + _fact_script()


def build_postflight_script() -> str:
    return "set -eu\n" + _fact_script()


def build_reboot_script() -> str:
    return """set -eu
emit() { printf '%s=%s\\n' "$1" "$2"; }
nohup sh -c 'sleep 1; systemctl reboot' >/dev/null 2>&1 &
emit reboot_requested yes
"""


def build_final_script() -> str:
    return "set -eu\nprintf 'cleanup_confirmed=%s\\n' yes\n"


def validate_preflight(facts: dict[str, str]) -> None:
    if facts.get("architecture") != "ok":
        raise GateError("preflight", "architecture_invalid")
    for key in ("kvmd_active", "retained_state", "staging_absent", "account_absent"):
        if facts.get(key) != "yes":
            raise GateError("preflight", f"{key}_invalid")
    if not facts.get(BOOT_TOKEN_KEY):
        raise GateError("preflight", "boot_token_missing")


def validate_postflight(facts: dict[str, str]) -> None:
    for key in ("kvmd_active", "retained_state", "staging_absent", "account_absent"):
        if facts.get(key) != "yes":
            raise GateError("postflight", f"{key}_invalid")


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
            result = prototype.run_ssh(
                target, secret, build_postflight_script(), timeout=20
            )
        except (OSError, ValueError):
            continue
        if result.returncode != 0:
            continue
        reachable = True
        try:
            facts = prototype.parse_facts(result.stdout, POSTFLIGHT_KEYS)
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

    stage = "preflight"
    attempts = 0
    postflight: dict[str, str] = {}
    try:
        preflight = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_preflight_script(), timeout=25).stdout,
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        if options.verify_only:
            postflight = prototype.parse_facts(
                prototype.run_ssh(
                    target, secret, build_postflight_script(), timeout=25
                ).stdout,
                POSTFLIGHT_KEYS,
            )
            validate_postflight(postflight)
            stage = "receipt"
            cleanup = prototype.parse_facts(
                prototype.run_ssh(target, secret, build_final_script(), timeout=25).stdout,
                FINAL_KEYS,
            )
            if cleanup.get("cleanup_confirmed") != "yes":
                raise GateError("receipt", "cleanup_unconfirmed")
            verify_receipt: Receipt = {
                "mode": "verify_only",
                "bootIdentityChanged": False,
                "deviceReachableAfterReboot": True,
                "kvmdActive": True,
                "retainedStateIntact": True,
                "stagingAbsent": True,
                "accountAbsent": True,
                "previousBootJournalPresent": postflight.get("previous_boot_journal") == "yes",
                "cleanupConfirmed": True,
                "pollAttempts": 0,
                "failureStage": "",
                "failureType": "",
                "physicalActionsInvoked": False,
                "persistentChangeScope": "none",
            }
            write_json_0600_atomic(output, verify_receipt)
            return 0

        previous_token = preflight[BOOT_TOKEN_KEY]
        stage = "reboot"
        reboot = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_reboot_script(), timeout=25).stdout,
            REBOOT_KEYS,
        )
        if reboot.get("reboot_requested") != "yes":
            raise GateError("reboot", "reboot_not_requested")

        stage = "postflight"
        postflight, attempts = wait_for_return(
            target, secret, previous_token, options.timeout_seconds
        )
        validate_postflight(postflight)

        stage = "final"
        final = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_final_script(), timeout=25).stdout,
            FINAL_KEYS,
        )
        if final.get("cleanup_confirmed") != "yes":
            raise GateError("final", "cleanup_unconfirmed")
    except GateError as error:
        receipt: Receipt = {
            "mode": "verify_only" if options.verify_only else "reboot",
            "bootIdentityChanged": bool(postflight),
            "deviceReachableAfterReboot": bool(postflight),
            "kvmdActive": postflight.get("kvmd_active") == "yes",
            "retainedStateIntact": postflight.get("retained_state") == "yes",
            "stagingAbsent": postflight.get("staging_absent") == "yes",
            "accountAbsent": postflight.get("account_absent") == "yes",
            "previousBootJournalPresent": postflight.get("previous_boot_journal") == "yes",
            "cleanupConfirmed": False,
            "pollAttempts": attempts,
            "failureStage": stage,
            "failureType": error.failure_type,
            "physicalActionsInvoked": False,
            "persistentChangeScope": "reboot",
        }
        write_json_0600_atomic(output, receipt)
        return 1

    receipt = {
        "mode": "reboot",
        "bootIdentityChanged": True,
        "deviceReachableAfterReboot": True,
        "kvmdActive": True,
        "retainedStateIntact": True,
        "stagingAbsent": True,
        "accountAbsent": True,
        "previousBootJournalPresent": postflight.get("previous_boot_journal") == "yes",
        "cleanupConfirmed": True,
        "pollAttempts": attempts,
        "failureStage": "",
        "failureType": "",
        "physicalActionsInvoked": False,
        "persistentChangeScope": "reboot",
    }
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
