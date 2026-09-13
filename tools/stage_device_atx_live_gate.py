#!/usr/bin/env python3
"""Run the live ATX waiting-route gate on a test H616.

Power and Reset must be physically disconnected. The gate starts one transient
systemd unit named like the future ATX transition service, delivers the enrolled
KVMD credential through ``LoadCredential=`` and runs the argument-free
`bli-kvm-atx-live-gate` binary against `/run/kvmd/kvmd.sock`. It removes its
staging directory and the transient unit afterwards.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import secrets
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


GATE_PATH = "libexec/bli-kvm-atx-live-gate"
UNIT = "hardened-blikvm-atx-live-gate.service"
PREFLIGHT_KEYS = {
    "architecture",
    "credentials",
    "kvmd_socket",
    "systemd_run",
    "test_root_absent",
    "unit_free",
}
GATE_KEYS = {
    "cooldown_active",
    "conflict_rejected",
    "duplicate_replay",
    "power_long",
    "power_short",
    "power_short_elapsed_ms",
    "reconciliation",
    "reset",
    "restart_pending",
}
APPLY_KEYS = {"gate_present", "gate_sha256", "run_rc", "physical_actions_invoked"} | GATE_KEYS
ROLLBACK_KEYS = {"stage_absent", "unit_absent"}
FINAL_KEYS = {"cleanup_confirmed"}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    bundleSha256: str
    gateSha256: str
    facts: dict[str, str]
    cleanupConfirmed: bool
    failureStage: str
    failureType: str
    powerResetPhysicallyDisconnected: bool
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
    return GatePaths(
        stage=stage,
        test_root=f"{stage.root}/atx-live-gate",
        gate_binary=f"{stage.payload}/{GATE_PATH}",
    )


def _assignments(values: dict[str, str | int]) -> str:
    import shlex

    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def build_preflight_script(paths: GatePaths) -> str:
    values = _assignments({"root": paths.test_root, "unit": UNIT})
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
case "$(uname -m)" in aarch64) emit architecture ok ;; *) emit architecture no ;; esac
if [ -S /run/kvmd/kvmd.sock ]; then emit kvmd_socket yes; else emit kvmd_socket no; fi
if [ -f /etc/hardened-blikvm/kvmd-user ] && [ -f /etc/hardened-blikvm/kvmd-password ]; then
  emit credentials yes
else
  emit credentials no
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
            "base": paths.stage.root,
            "payload": paths.stage.payload,
            "archive": paths.stage.archive,
            "gate": paths.gate_binary,
            "unit": UNIT,
        }
    )
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
umask 077
install -d -m 0700 "$root" "$payload"
tar -xzf "$archive" -C "$payload" --strip-components=1
if [ ! -f "$gate" ]; then emit gate_present no; exit 1; fi
emit gate_present yes
chmod 0711 "$base" "$root"
install -d -m 0755 "$root/bin"
install -m 0755 "$gate" "$root/bin/bli-kvm-atx-live-gate"
staged="$root/bin/bli-kvm-atx-live-gate"
emit gate_sha256 "$(sha256sum "$gate" | cut -d ' ' -f 1)"
if systemd-run --quiet --wait --pipe --collect \
  --unit=hardened-blikvm-atx-live-gate \
  --property=Type=oneshot \
  --property=LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user \
  --property=LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password \
  "$staged" > "$root/gate.out" 2>/dev/null; then
  emit run_rc 0
else
  emit run_rc 1
fi
cat "$root/gate.out"
"""


def build_rollback_script(paths: GatePaths) -> str:
    values = _assignments({"stage_root": paths.stage.root, "unit": UNIT})
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
systemctl stop "$unit" 2>/dev/null || true
systemctl reset-failed "$unit" 2>/dev/null || true
rm -rf -- "$stage_root"
if [ ! -e "$stage_root" ]; then emit stage_absent yes; else emit stage_absent no; fi
unit_state=$(systemctl show -p LoadState --value "$unit" 2>/dev/null || true)
case "$unit_state" in not-found|"") emit unit_absent yes ;; *) emit unit_absent no ;; esac
"""


def build_final_script(paths: GatePaths) -> str:
    values = _assignments({"stage_root": paths.stage.root})
    return f"""set -eu
{values}
if [ ! -e "$stage_root" ]; then printf 'cleanup_confirmed=%s\\n' yes; else printf 'cleanup_confirmed=%s\\n' no; fi
"""


def validate_preflight(facts: dict[str, str]) -> None:
    for key in ("kvmd_socket", "credentials", "systemd_run", "test_root_absent", "unit_free"):
        if facts.get(key) != "yes":
            raise GateError("preflight", f"{key}_invalid")
    if facts.get("architecture") != "ok":
        raise GateError("preflight", "architecture_invalid")


def validate_apply(facts: dict[str, str]) -> None:
    if facts.get("gate_present") != "yes":
        raise GateError("apply", "gate_binary_missing")
    if facts.get("run_rc") != "0":
        raise GateError("apply", "atx_live_gate_exit")
    for phase in GATE_KEYS:
        if phase == "power_short_elapsed_ms":
            if not facts.get(phase, "").isdigit():
                raise GateError("apply", "power_short_elapsed_missing")
        elif facts.get(phase) != "ok":
            raise GateError("apply", f"{phase}_failed")
    if facts.get("physical_actions_invoked") != "yes":
        raise GateError("apply", "physical_actions_not_invoked")


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
    facts: dict[str, str] = {}
    stage = "preflight"
    cleanup_confirmed = False
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

        stage = "apply"
        apply_result = prototype.run_ssh(target, secret, build_apply_script(paths), timeout=180)
        apply_result.check_returncode()
        facts = prototype.parse_facts(apply_result.stdout, APPLY_KEYS)
        validate_apply(facts)

        stage = "rollback"
        rollback = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_rollback_script(paths), timeout=40).stdout,
            ROLLBACK_KEYS,
        )
        if (
            rollback.get("stage_absent") != "yes"
            or rollback.get("unit_absent") != "yes"
        ):
            raise GateError("rollback", "rollback_incomplete")

        stage = "final"
        final = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_final_script(paths), timeout=25).stdout,
            FINAL_KEYS,
        )
        if final.get("cleanup_confirmed") != "yes":
            raise GateError("final", "cleanup_unconfirmed")
        cleanup_confirmed = True
    except GateError as error:
        receipt: Receipt = {
            "bundleSha256": bundle.sha256,
            "gateSha256": facts.get("gate_sha256", ""),
            "facts": facts,
            "cleanupConfirmed": cleanup_confirmed,
            "failureStage": stage,
            "failureType": error.failure_type,
            "powerResetPhysicallyDisconnected": True,
            "persistentChangeScope": "none",
        }
        write_json_0600_atomic(output, receipt)
        return 1

    success: Receipt = {
        "bundleSha256": bundle.sha256,
        "gateSha256": facts["gate_sha256"],
        "facts": facts,
        "cleanupConfirmed": True,
        "failureStage": "",
        "failureType": "",
        "powerResetPhysicallyDisconnected": True,
        "persistentChangeScope": "none",
    }
    write_json_0600_atomic(output, success)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
