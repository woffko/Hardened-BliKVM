#!/usr/bin/env python3
"""Run the isolated-backend HID transition gate on a test H616.

The gate uploads a signed prototype bundle, extracts the argument-free
`bli-kvm-hid-isolated-gate` binary, and runs it. That binary binds only local
mock KVMD Unix sockets under ``/tmp`` and proves startup-ledger replay, lease
apply/release, expiry, identity mismatch and backend-failure release-all. It
never contacts the real KVMD, never injects into a host, and removes its
staging directory.
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


GATE_PATH = "libexec/bli-kvm-hid-isolated-gate"
PREFLIGHT_KEYS = {"architecture", "test_root_absent"}
APPLY_KEYS = {
    "expiry_release",
    "failure_release_all",
    "gate_present",
    "gate_sha256",
    "identity_mismatch",
    "isolated_backend",
    "lease_apply",
    "physical_actions_invoked",
    "run_rc",
    "startup_replay",
}
ROLLBACK_KEYS = {"stage_absent"}
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
        test_root=f"{stage.root}/hid-isolated-gate",
        gate_binary=f"{stage.payload}/{GATE_PATH}",
    )


def _assignments(values: dict[str, str | int]) -> str:
    import shlex

    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def build_preflight_script(paths: GatePaths) -> str:
    values = _assignments({"root": paths.test_root})
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
case "$(uname -m)" in aarch64) emit architecture ok ;; *) emit architecture no ;; esac
if [ ! -e "$root" ]; then emit test_root_absent yes; else emit test_root_absent no; fi
"""


def build_apply_script(paths: GatePaths) -> str:
    values = _assignments(
        {
            "root": paths.test_root,
            "payload": paths.stage.payload,
            "archive": paths.stage.archive,
            "gate": paths.gate_binary,
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
emit gate_sha256 "$(sha256sum "$gate" | cut -d ' ' -f 1)"
if "$gate" > "$root/gate.out" 2>"$root/gate.err"; then run_rc=0; else run_rc=1; fi
emit run_rc "$run_rc"
cat "$root/gate.out"
"""


def build_rollback_script(paths: GatePaths) -> str:
    values = _assignments({"stage_root": paths.stage.root})
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
rm -rf -- "$stage_root"
if [ ! -e "$stage_root" ]; then emit stage_absent yes; else emit stage_absent no; fi
"""


def build_final_script(paths: GatePaths) -> str:
    values = _assignments({"stage_root": paths.stage.root})
    return f"""set -eu
{values}
if [ ! -e "$stage_root" ]; then printf 'cleanup_confirmed=%s\\n' yes; else printf 'cleanup_confirmed=%s\\n' no; fi
"""


def validate_preflight(facts: dict[str, str]) -> None:
    if facts.get("architecture") != "ok":
        raise GateError("preflight", "architecture_invalid")
    if facts.get("test_root_absent") != "yes":
        raise GateError("preflight", "test_root_present")


def validate_apply(facts: dict[str, str]) -> None:
    if facts.get("gate_present") != "yes":
        raise GateError("apply", "gate_binary_missing")
    if facts.get("run_rc") != "0":
        raise GateError("apply", "isolated_gate_exit")
    if facts.get("isolated_backend") != "yes":
        raise GateError("apply", "isolated_backend_missing")
    for phase in (
        "startup_replay",
        "lease_apply",
        "expiry_release",
        "identity_mismatch",
        "failure_release_all",
    ):
        if facts.get(phase) != "ok":
            raise GateError("apply", f"{phase}_failed")
    if facts.get("physical_actions_invoked") != "no":
        raise GateError("apply", "physical_actions_invoked")


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
        apply_result = prototype.run_ssh(target, secret, build_apply_script(paths), timeout=90)
        apply_result.check_returncode()
        facts = prototype.parse_facts(apply_result.stdout, APPLY_KEYS)
        validate_apply(facts)

        stage = "rollback"
        rollback = prototype.parse_facts(
            prototype.run_ssh(target, secret, build_rollback_script(paths), timeout=40).stdout,
            ROLLBACK_KEYS,
        )
        if rollback.get("stage_absent") != "yes":
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
            "physicalActionsInvoked": False,
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
        "physicalActionsInvoked": False,
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
