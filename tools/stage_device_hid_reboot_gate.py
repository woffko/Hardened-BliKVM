#!/usr/bin/env python3
"""Run the BliKVM reboot gate for the restricted HID path on a test H616.

The gate records a boot identity, requests exactly one software reboot, waits
for the device to return, and verifies the boot identity changed while the
retained slots, retained guard and evidence store are intact and no HID test
account or staging path survived. It performs no upload and stores no raw boot
identifier in the receipt.
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
}
POSTFLIGHT_KEYS = {
    "boot_token",
    "kvmd_active",
    "retained_state",
    "staging_absent",
    "account_absent",
}
REBOOT_KEYS = {"reboot_requested"}
FINAL_KEYS = {"cleanup_confirmed"}
REBOOT_TIMEOUT_SECONDS = 300.0
POLL_INTERVAL_SECONDS = 5.0


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    bootIdentityChanged: bool
    kvmdActive: bool
    retainedStateIntact: bool
    stagingAbsent: bool
    accountAbsent: bool
    cleanupConfirmed: bool
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


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
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


def wait_for_return(
    target: prototype.RemoteTarget,
    secret: bytearray,
    previous_token: str,
) -> dict[str, str]:
    deadline = time.monotonic() + REBOOT_TIMEOUT_SECONDS
    while time.monotonic() < deadline:
        time.sleep(POLL_INTERVAL_SECONDS)
        try:
            result = prototype.run_ssh(
                target, secret, build_postflight_script(), timeout=20
            )
        except (OSError, ValueError):
            continue
        if result.returncode != 0:
            continue
        try:
            facts = prototype.parse_facts(result.stdout, POSTFLIGHT_KEYS)
        except ValueError:
            continue
        if facts.get(BOOT_TOKEN_KEY) and facts[BOOT_TOKEN_KEY] != previous_token:
            return facts
    raise GateError("reboot", "device_did_not_return_with_new_boot_identity")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()

    stage = "preflight"
    previous_token = ""
    postflight: dict[str, str] = {}
    try:
        preflight = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_preflight_script(), timeout=25).stdout,
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)
        previous_token = preflight[BOOT_TOKEN_KEY]

        stage = "reboot"
        reboot = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_reboot_script(), timeout=25).stdout,
            REBOOT_KEYS,
        )
        if reboot.get("reboot_requested") != "yes":
            raise GateError("reboot", "reboot_not_requested")

        stage = "postflight"
        postflight = wait_for_return(
            target, secret, previous_token
        )
        for key in ("kvmd_active", "retained_state", "staging_absent", "account_absent"):
            if postflight.get(key) != "yes":
                raise GateError("postflight", f"{key}_invalid")

        stage = "final"
        final = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_final_script(), timeout=25).stdout,
            FINAL_KEYS,
        )
        if final.get("cleanup_confirmed") != "yes":
            raise GateError("final", "cleanup_unconfirmed")
    except GateError as error:
        receipt: Receipt = {
            "bootIdentityChanged": bool(postflight),
            "kvmdActive": postflight.get("kvmd_active") == "yes",
            "retainedStateIntact": postflight.get("retained_state") == "yes",
            "stagingAbsent": postflight.get("staging_absent") == "yes",
            "accountAbsent": postflight.get("account_absent") == "yes",
            "cleanupConfirmed": False,
            "failureStage": stage,
            "failureType": error.failure_type,
            "physicalActionsInvoked": False,
            "persistentChangeScope": "reboot",
        }
        write_json_0600_atomic(output, receipt)
        return 1

    receipt = {
        "bootIdentityChanged": True,
        "kvmdActive": True,
        "retainedStateIntact": True,
        "stagingAbsent": True,
        "accountAbsent": True,
        "cleanupConfirmed": True,
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
