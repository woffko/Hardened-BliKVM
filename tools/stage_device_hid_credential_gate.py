#!/usr/bin/env python3
"""Run a reversible HID credential-delivery gate on a test H616.

The gate uploads a signed prototype bundle, starts one transient systemd unit
named exactly like the packaged HID credential unit, delivers two test-only
credentials through ``LoadCredential=``, runs the argument-free gate binary,
and removes every staging path afterwards. No host secret is transferred: the
test credentials are generated on the device and never leave it.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import secrets
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


UNIT = "hardened-blikvm-hid-credential.service"
GATE_PATH = "libexec/bli-kvm-hid-credential-gate"
PREFLIGHT_KEYS = {
    "architecture",
    "hid_account_absent",
    "systemd_run",
    "test_root_absent",
    "unit_free",
}
APPLY_KEYS = {
    "account_created",
    "credential_delivery",
    "delivery_token",
    "gate_present",
    "gate_sha256",
    "source_read_denied",
}
ROLLBACK_KEYS = {"account_removed", "credentials_removed", "stage_absent", "unit_absent"}
FINAL_KEYS = {"cleanup_confirmed", "stage_absent"}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str, *, cleanup_confirmed: bool | None = None) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type
        self.cleanup_confirmed = cleanup_confirmed


class Receipt(TypedDict):
    bundleSha256: str
    gateSha256: str
    unit: str
    failureStage: str
    failureType: str
    failureFacts: dict[str, str]
    deliveryToken: str
    sourceReadDenied: bool
    cleanupConfirmed: bool
    physicalActionsInvoked: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    bundle: str
    expected_sha256: str
    output: str


@dataclass(frozen=True)
class GatePaths:
    stage: prototype.StagePaths
    test_root: str
    credentials: str
    gate_output: str
    gate_binary: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        bundle=cast(str, namespace.bundle),
        expected_sha256=cast(str, namespace.expected_sha256),
        output=cast(str, namespace.output),
    )


def gate_paths(bundle_sha256: str, run_id: str) -> GatePaths:
    stage = prototype.stage_paths(bundle_sha256, run_id)
    test_root = f"{stage.root}/hid-credential-gate"
    return GatePaths(
        stage=stage,
        test_root=test_root,
        credentials=f"{test_root}/credentials",
        gate_output=f"{test_root}/gate.out",
        gate_binary=f"{stage.payload}/{GATE_PATH}",
    )


def _assignments(values: dict[str, str | int]) -> str:
    import shlex

    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def build_preflight_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "root": paths.test_root,
            "unit": UNIT,
        }
    )
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
case "$(uname -m)" in aarch64) arch=ok ;; *) arch=no ;; esac
emit architecture "$arch"
if id -u hardened-blikvm-hid >/dev/null 2>&1; then
  emit hid_account_absent no
else
  emit hid_account_absent yes
fi
if command -v systemd-run >/dev/null 2>&1; then emit systemd_run yes; else emit systemd_run no; fi
if [ ! -e "$root" ]; then emit test_root_absent yes; else emit test_root_absent no; fi
unit_state=$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)
case "$unit_state" in not-found|"") emit unit_free yes ;; *) emit unit_free no ;; esac
"""


def build_apply_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "root": paths.test_root,
            "cred": paths.credentials,
            "out": paths.gate_output,
            "gate": paths.gate_binary,
            "archive": paths.stage.archive,
            "payload": paths.stage.payload,
            "base": paths.stage.root,
            "unit": UNIT,
        }
    )
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
umask 077
install -d -m 0700 "$root"
install -d -m 0700 "$payload"
tar -xzf "$archive" -C "$payload" --strip-components=1
if [ ! -f "$gate" ]; then emit gate_present no; exit 1; fi
emit gate_present yes
chmod 0711 "$base" "$root"
install -d -m 0755 "$root/bin"
install -m 0755 "$gate" "$root/bin/bli-kvm-hid-credential-gate"
staged="$root/bin/bli-kvm-hid-credential-gate"
account_created=no
if ! id -u hardened-blikvm-hid >/dev/null 2>&1; then
  useradd --system --user-group --home-dir /nonexistent \
    --shell /usr/sbin/nologin hardened-blikvm-hid
  account_created=yes
fi
printf '%s\n' "$account_created" > "$root/account_created"
if [ "$account_created" = yes ]; then
  emit account_created yes
else
  emit account_created reused
fi
install -d -m 0700 "$cred"
printf '%s\n' hid-gate-user > "$cred/kvmd-user"
printf '%s\n' "hid-gate-$(
  od -An -N8 -tx1 /dev/urandom | tr -d ' \n'
)" > "$cred/kvmd-password"
chmod 400 "$cred/kvmd-user" "$cred/kvmd-password"
delivery=failed
if systemd-run --quiet --wait --pipe --collect \
  --unit=hardened-blikvm-hid-credential \
  --property=Type=oneshot \
  --property=User=hardened-blikvm-hid \
  --property=Group=hardened-blikvm-hid \
  --property=LoadCredential="kvmd-user:$cred/kvmd-user" \
  --property=LoadCredential="kvmd-password:$cred/kvmd-password" \
  "$staged" > "$out" 2>/dev/null; then
  delivery=ok
fi
case "$delivery" in ok) emit credential_delivery ok ;; *) emit credential_delivery failed ;; esac
if grep -qx 'hid-credential-delivery=ok' "$out" 2>/dev/null; then
  emit delivery_token ok
else
  emit delivery_token no
fi
emit gate_sha256 "$(sha256sum "$gate" | cut -d ' ' -f 1)"
uid=$(id -u hardened-blikvm-hid)
gid=$(id -g hardened-blikvm-hid)
if setpriv --reuid="$uid" --regid="$gid" --clear-groups --no-new-privs \
  cat "$cred/kvmd-password" >/dev/null 2>&1; then
  emit source_read_denied no
else
  emit source_read_denied yes
fi
"""


def build_rollback_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "root": paths.test_root,
            "stage_root": paths.stage.root,
            "unit": UNIT,
        }
    )
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
systemctl stop "$unit" 2>/dev/null || true
systemctl reset-failed "$unit" 2>/dev/null || true
account_created=$(cat "$root/account_created" 2>/dev/null || echo no)
if [ "$account_created" = yes ]; then
  userdel hardened-blikvm-hid 2>/dev/null || true
  groupdel hardened-blikvm-hid 2>/dev/null || true
fi
if id -u hardened-blikvm-hid >/dev/null 2>&1; then
  emit account_removed no
else
  emit account_removed yes
fi
rm -rf -- "$root"
rm -rf -- "$stage_root"
if [ ! -e "$root" ]; then emit credentials_removed yes; else emit credentials_removed no; fi
if [ ! -e "$stage_root" ]; then emit stage_absent yes; else emit stage_absent no; fi
unit_state=$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)
case "$unit_state" in not-found|"") emit unit_absent yes ;; *) emit unit_absent no ;; esac
"""


def build_final_script(paths: GatePaths) -> str:
    values = _assignments(
        {"root": paths.test_root, "stage_root": paths.stage.root}
    )
    return f"""set -eu
{values}
if [ ! -e "$root" ] && [ ! -e "$stage_root" ]; then
  printf 'cleanup_confirmed=%s\\n' yes
else
  printf 'cleanup_confirmed=%s\\n' no
fi
printf 'stage_absent=%s\\n' "$([ ! -e "$stage_root" ] && echo yes || echo no)"
"""


def validate_preflight(facts: dict[str, str]) -> None:
    for key in ("architecture", "hid_account_absent", "systemd_run", "test_root_absent", "unit_free"):
        if key == "architecture":
            if facts.get(key) != "ok":
                raise GateError("preflight", "architecture_invalid")
        elif facts.get(key) != "yes":
            raise GateError("preflight", f"{key}_invalid")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    expected_sha256 = prototype.validate_sha256(options.expected_sha256)
    bundle = prototype.validate_bundle(options.bundle, expected_sha256)
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    paths = gate_paths(bundle.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    cleanup_confirmed = False
    cleanup_required = False
    checks: dict[str, str] = {}
    stage = "preflight"

    def run_rollback() -> bool:
        rolled_back = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_rollback_script(paths), timeout=40).stdout,
            ROLLBACK_KEYS,
        )
        return (
            rolled_back.get("credentials_removed") == "yes"
            and rolled_back.get("account_removed") == "yes"
            and rolled_back.get("stage_absent") == "yes"
            and rolled_back.get("unit_absent") == "yes"
        )

    try:
        preflight = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_preflight_script(paths), timeout=25).stdout,
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        stage = "upload"
        with bundle.path.open("rb") as bundle_stream:
            _ = prototype.run_ssh(
                target,
                secret,
                prototype.build_upload_script(paths.stage),
                stdin=bundle_stream,
                timeout=90,
            ).check_returncode()
        cleanup_required = True

        stage = "apply"
        apply_result = prototype.run_ssh(
            target, secret, build_apply_script(paths), timeout=60
        )
        apply_result.check_returncode()
        checks = prototype.parse_facts(apply_result.stdout, APPLY_KEYS)
        if checks.get("gate_present") != "yes":
            raise GateError("apply", "gate_binary_missing")
        if checks.get("account_created") not in ("yes", "reused"):
            raise GateError("apply", "account_not_prepared")
        if checks.get("credential_delivery") != "ok":
            raise GateError("apply", "credential_delivery_failed")
        if checks.get("delivery_token") != "ok":
            raise GateError("apply", "delivery_token_missing")
        if checks.get("source_read_denied") != "yes":
            raise GateError("apply", "credential_source_readable")

        stage = "rollback"
        if not run_rollback():
            raise GateError("rollback", "rollback_incomplete")

        stage = "final"
        final = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_final_script(paths), timeout=25).stdout,
            FINAL_KEYS,
        )
        if final.get("cleanup_confirmed") != "yes" or final.get("stage_absent") != "yes":
            raise GateError("final", "cleanup_unconfirmed")
        cleanup_confirmed = True
    except GateError as error:
        if cleanup_required and not cleanup_confirmed:
            try:
                cleanup_confirmed = run_rollback()
            except (OSError, UnicodeError, ValueError):
                cleanup_confirmed = False
        receipt: Receipt = {
            "bundleSha256": bundle.sha256,
            "gateSha256": checks.get("gate_sha256", ""),
            "unit": UNIT,
            "failureStage": stage,
            "failureType": error.failure_type,
            "failureFacts": checks,
            "deliveryToken": "",
            "sourceReadDenied": False,
            "cleanupConfirmed": cleanup_confirmed or error.cleanup_confirmed is True,
            "physicalActionsInvoked": False,
            "persistentChangeScope": "none",
        }
        write_json_0600_atomic(output, receipt)
        return 1

    success_receipt: Receipt = {
        "bundleSha256": bundle.sha256,
        "gateSha256": checks["gate_sha256"],
        "unit": UNIT,
        "failureStage": "",
        "failureType": "",
        "failureFacts": {},
        "deliveryToken": "ok",
        "sourceReadDenied": True,
        "cleanupConfirmed": True,
        "physicalActionsInvoked": False,
        "persistentChangeScope": "none",
    }
    write_json_0600_atomic(output, success_receipt)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
