#!/usr/bin/env python3
"""Read-only log-health check for a test H616.

Verifies that log rotation is valid, journald persists to the dedicated
partition mount, and no unit is failed. It never mutates the device and stores
only safe booleans plus the mount source path in the receipt.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


FACT_KEYS = {
    "architecture",
    "failed_units",
    "journal_mount_source",
    "journal_on_dedicated",
    "journald_active",
    "journald_storage",
    "logrotate_service",
    "logrotate_valid",
    "previous_boot_journal",
}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    logrotateValid: bool
    logrotateServiceOk: bool
    journaldStorage: str
    journaldActive: bool
    journalMountOnDedicatedPartition: bool
    journalMountSource: str
    previousBootJournalPresent: bool
    failedUnits: int
    failureStage: str
    failureType: str
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


def build_script() -> str:
    return r"""set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
case "$(uname -m)" in aarch64) emit architecture ok ;; *) emit architecture no ;; esac
if logrotate -d /etc/logrotate.conf >/dev/null 2>&1; then emit logrotate_valid yes; else emit logrotate_valid no; fi
if systemctl is-failed --quiet logrotate.service; then emit logrotate_service failed; else emit logrotate_service ok; fi
emit journald_storage "$(grep '^Storage=' /etc/systemd/journald.conf 2>/dev/null | cut -d= -f2 | head -n1)"
if systemctl is-active --quiet systemd-journald; then emit journald_active yes; else emit journald_active no; fi
journal_source=$(findmnt -no SOURCE /var/log/journal 2>/dev/null | head -n1 | tr -d '[] ')
emit journal_mount_source "$journal_source"
case "$journal_source" in
  /dev/mmcblk0p3/*) emit journal_on_dedicated yes ;;
  *) emit journal_on_dedicated no ;;
esac
emit failed_units "$(systemctl --failed --no-legend --no-pager 2>/dev/null | wc -l)"
if journalctl -b -1 -n 1 --no-pager >/dev/null 2>&1; then emit previous_boot_journal yes; else emit previous_boot_journal no; fi
"""


def validate_facts(facts: dict[str, str]) -> None:
    if facts.get("architecture") != "ok":
        raise GateError("check", "architecture_invalid")
    if facts.get("logrotate_valid") != "yes":
        raise GateError("check", "logrotate_invalid")
    if facts.get("logrotate_service") != "ok":
        raise GateError("check", "logrotate_service_failed")
    if facts.get("journald_storage") != "persistent":
        raise GateError("check", "journald_not_persistent")
    if facts.get("journald_active") != "yes":
        raise GateError("check", "journald_inactive")
    if facts.get("journal_on_dedicated") != "yes":
        raise GateError("check", "journal_not_on_dedicated_partition")
    try:
        failed_units = int(facts.get("failed_units", "-1"))
    except ValueError:
        raise GateError("check", "failed_units_invalid") from None
    if failed_units != 0:
        raise GateError("check", "failed_units_present")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    facts: dict[str, str] = {}
    stage = "check"
    try:
        result = prototype.run_ssh(target, secret, build_script(), timeout=30)
        result.check_returncode()
        facts = prototype.parse_facts(result.stdout, FACT_KEYS)
        validate_facts(facts)
    except (GateError, ValueError, OSError) as error:
        failure_type = error.failure_type if isinstance(error, GateError) else type(error).__name__
        receipt: Receipt = {
            "logrotateValid": facts.get("logrotate_valid") == "yes",
            "logrotateServiceOk": facts.get("logrotate_service") == "ok",
            "journaldStorage": facts.get("journald_storage", ""),
            "journaldActive": facts.get("journald_active") == "yes",
            "journalMountOnDedicatedPartition": False,
            "journalMountSource": facts.get("journal_mount_source", ""),
            "previousBootJournalPresent": facts.get("previous_boot_journal") == "yes",
            "failedUnits": -1,
            "failureStage": stage,
            "failureType": failure_type,
            "persistentChangeScope": "none",
        }
        write_json_0600_atomic(output, receipt)
        return 1

    receipt = {
        "logrotateValid": True,
        "logrotateServiceOk": True,
        "journaldStorage": "persistent",
        "journaldActive": True,
        "journalMountOnDedicatedPartition": True,
        "journalMountSource": facts["journal_mount_source"],
        "previousBootJournalPresent": facts.get("previous_boot_journal") == "yes",
        "failedUnits": 0,
        "failureStage": "",
        "failureType": "",
        "persistentChangeScope": "none",
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
