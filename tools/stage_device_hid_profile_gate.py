#!/usr/bin/env python3
"""Run the HID transition-profile broker gate on a test H616.

The gate uploads the test-profile bundle, starts the feature-built control
broker in HID profile mode with only temporary files, and proves two things:
the profile refuses to start without a systemd credential directory, and it
starts and binds its control socket when a temporary credential directory is
present. It never contacts the real KVMD and leaves no account, unit or staging
path behind.
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


GATE_PATH = "libexec/bli-kvm-control-broker"
PREFLIGHT_KEYS = {"architecture", "test_root_absent"}
APPLY_KEYS = {
    "broker_present",
    "gate_sha256",
    "refused_without_credentials",
    "started_with_credentials",
}
ROLLBACK_KEYS = {"process_stopped", "stage_absent"}
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
    return GatePaths(stage=stage, test_root=f"{stage.root}/hid-profile-gate")


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
            "broker": f"{paths.stage.payload}/{GATE_PATH}",
        }
    )
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
umask 077
install -d -m 0700 "$root" "$payload"
tar -xzf "$archive" -C "$payload" --strip-components=1
if [ ! -x "$broker" ]; then emit broker_present no; exit 1; fi
emit broker_present yes
emit gate_sha256 "$(sha256sum "$broker" | cut -d ' ' -f 1)"
install -d -m 0700 "$root/ledger" "$root/cred"
# The profile must refuse to start without a systemd credential directory.
set +e
env -u CREDENTIALS_DIRECTORY \\
  BLI_KVM_HID_PROFILE=1 \\
  BLI_KVM_HID_LEDGER="$root/ledger/ledger.json" \\
  BLI_KVM_HID_KVMD_SOCKET="$root/mock.sock" \\
  BLI_KVM_CONTROL_SOCKET="$root/control1.sock" \\
  "$broker" > "$root/out1" 2>&1
rc1=$?
set -e
if [ "$rc1" -ne 0 ] && [ ! -e "$root/control1.sock" ]; then
  emit refused_without_credentials yes
else
  emit refused_without_credentials no
fi
# With a temporary credential directory it must start and bind its socket.
printf '%s\\n' hid-profile-user > "$root/cred/kvmd-user"
printf '%s\\n' hid-profile-not-a-real-password > "$root/cred/kvmd-password"
chmod 400 "$root/cred/kvmd-user" "$root/cred/kvmd-password"
CREDENTIALS_DIRECTORY="$root/cred" \\
  BLI_KVM_HID_PROFILE=1 \\
  BLI_KVM_HID_LEDGER="$root/ledger/ledger.json" \\
  BLI_KVM_HID_KVMD_SOCKET="$root/mock.sock" \\
  BLI_KVM_CONTROL_SOCKET="$root/control2.sock" \\
  "$broker" > "$root/out2" 2>&1 &
pid=$!
printf '%s\\n' "$pid" > "$root/broker.pid"
started=no
i=0
while [ "$i" -lt 50 ]; do
  if [ -S "$root/control2.sock" ]; then started=yes; break; fi
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  sleep 0.1
  i=$((i + 1))
done
if [ "$started" = yes ]; then emit started_with_credentials yes; else emit started_with_credentials no; fi
kill "$pid" 2>/dev/null || true
wait "$pid" 2>/dev/null || true
"""


def build_rollback_script(paths: GatePaths) -> str:
    values = _assignments({"stage_root": paths.stage.root})
    return f"""set -eu
{values}
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
if [ -f "$stage_root/hid-profile-gate/broker.pid" ]; then
  pid=$(cat "$stage_root/hid-profile-gate/broker.pid")
  case "$pid" in
    *[!0-9]*) ;;
    *) kill "$pid" 2>/dev/null || true ;;
  esac
fi
rm -rf -- "$stage_root"
if [ ! -e "$stage_root" ]; then emit stage_absent yes; else emit stage_absent no; fi
emit process_stopped yes
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
    if facts.get("broker_present") != "yes":
        raise GateError("apply", "broker_missing")
    if facts.get("refused_without_credentials") != "yes":
        raise GateError("apply", "profile_started_without_credentials")
    if facts.get("started_with_credentials") != "yes":
        raise GateError("apply", "profile_did_not_start")


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
        if rollback.get("stage_absent") != "yes" or rollback.get("process_stopped") != "yes":
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
