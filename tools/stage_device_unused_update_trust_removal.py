#!/usr/bin/env python3
"""Remove one exact unused application public-trust config from a test device."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
import sys
from typing import Mapping, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as runtime_builder
from tools import build_debian_update_runtime_package as update_builder
from tools import stage_device_prototype as prototype
from tools import stage_device_update_runtime_package as update_package
from tools import stage_device_update_trust_config as trust_gate
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


CONFIG_TARGET = "/etc/hardened-blikvm/update-supervisor.json"
CONFIG_PARENT = "/etc/hardened-blikvm"
STATE_ROOT = "/var/lib/hardened-blikvm"
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:+~-]{1,96}$")
PRE_KEYS = {
    "account_sha256",
    "architecture",
    "broker_pid",
    "broker_restarts",
    "config_exact",
    "control_boundary_absent",
    "guards_absent",
    "hardened_nginx_pid",
    "hardened_nginx_restarts",
    "persistent_runtime_healthy",
    "rootfs_mode",
    "server_pid",
    "server_restarts",
    "tls_unchanged",
    "update_runtime_absent",
    "update_units_inert",
    "video_pid",
    "video_restarts",
}
FINAL_KEYS = {
    "account_unchanged",
    "config_absent",
    "control_boundary_absent",
    "guards_absent",
    "persistent_runtime_unchanged",
    "tls_unchanged",
    "update_runtime_absent",
    "update_units_inert",
}
CLASSIFY_KEYS = {"config_state"}


@dataclass(frozen=True)
class Options:
    enrollment_receipt: str
    expected_config_sha256: str
    endpoint: str
    username: str
    known_hosts: str
    baseline: str
    output: str
    preflight_only: bool


@dataclass(frozen=True)
class AcceptedEnrollment:
    config_sha256: str
    request_sha256: str
    source_commit: str


class RemovalReceipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    removedConfigSha256: str
    priorRequestSha256: str
    sourceCommit: str
    preflight: dict[str, str]
    finalState: dict[str, str]
    policy: dict[str, object]


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--enrollment-receipt", required=True)
    parser.add_argument("--expected-config-sha256", required=True)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", required=True)
    parser.add_argument("--baseline", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--preflight-only", action="store_true")
    namespace = parser.parse_args()
    return Options(
        enrollment_receipt=cast(str, namespace.enrollment_receipt),
        expected_config_sha256=cast(str, namespace.expected_config_sha256),
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        baseline=cast(str, namespace.baseline),
        output=cast(str, namespace.output),
        preflight_only=cast(bool, namespace.preflight_only),
    )


def load_enrollment_receipt(
    path_value: str, expected_config_sha256: str
) -> AcceptedEnrollment:
    expected = prototype.validate_sha256(expected_config_sha256)
    path = Path(path_value)
    if path.is_symlink():
        raise ValueError("accepted enrollment receipt must not be a symlink")
    resolved = path.resolve(strict=True)
    private_root = (Path(__file__).resolve().parent.parent / "evidence/private").resolve()
    metadata = resolved.stat()
    if (
        not resolved.is_relative_to(private_root)
        or resolved == private_root
        or not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or metadata.st_mode & 0o077
        or not 0 < metadata.st_size <= 16 * 1024
    ):
        raise ValueError("accepted enrollment receipt metadata is unsafe")
    decoded = cast(object, json.loads(resolved.read_bytes()))
    if not isinstance(decoded, dict):
        raise ValueError("accepted enrollment receipt root is invalid")
    receipt = cast(dict[str, object], decoded)
    policy_value = receipt.get("policy")
    final_value = receipt.get("finalState")
    if not isinstance(policy_value, dict) or not isinstance(final_value, dict):
        raise ValueError("accepted enrollment receipt is incomplete")
    policy = cast(dict[str, object], policy_value)
    final_state = cast(dict[str, object], final_value)
    config_sha256 = receipt.get("configSha256")
    request_sha256 = receipt.get("requestSha256")
    source_commit = receipt.get("sourceCommit")
    if (
        receipt.get("schemaVersion") != 1
        or receipt.get("probe") != "hardened-blikvm-update-trust-enrollment"
        or receipt.get("result") != "passed"
        or config_sha256 != expected
        or not isinstance(request_sha256, str)
        or SHA256.fullmatch(request_sha256) is None
        or not isinstance(source_commit, str)
        or re.fullmatch(r"[0-9a-f]{40}", source_commit) is None
        or policy.get("configPublished") is not True
        or policy.get("publicKeyOnly") is not True
        or policy.get("privateKeyAccepted") is not False
        or policy.get("stateInitialized") is not False
        or policy.get("unitsEnabled") is not False
        or policy.get("deviceReboot") is not False
        or policy.get("targetPowerControl") is not False
        or final_state.get("config_persistent") != "yes"
        or final_state.get("update_runtime_absent") != "yes"
        or final_state.get("update_units_inert") != "yes"
        or final_state.get("guards_removed") != "yes"
    ):
        raise ValueError("accepted enrollment receipt does not authorize removal")
    assert isinstance(config_sha256, str)
    return AcceptedEnrollment(config_sha256, request_sha256, source_commit)


def _digest(value: str) -> str:
    return hashlib.sha256(value.encode()).hexdigest()


def _assignments(values: Mapping[str, str | int]) -> str:
    return "\n".join(f"{key}={shlex.quote(str(value))}" for key, value in values.items())


def _values(
    enrollment: AcceptedEnrollment,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str] | None = None,
) -> dict[str, str | int]:
    values: dict[str, str | int] = {
        "config_target": CONFIG_TARGET,
        "config_parent": CONFIG_PARENT,
        "config_sha256": enrollment.config_sha256,
        "runtime_package_name": runtime_builder.PACKAGE_NAME,
        "runtime_package_version": runtime_builder.PACKAGE_VERSION,
        "package_name": update_builder.PACKAGE_NAME,
        "package_version": update_builder.PACKAGE_VERSION,
        "state_root": STATE_ROOT,
        "update_config": CONFIG_TARGET,
        "update_state": update_package.UPDATE_STATE,
        "slot_root": update_package.SLOT_ROOT,
        "update_runtime": update_package.UPDATE_RUNTIME,
        "update_socket_runtime": update_package.UPDATE_SOCKET_RUNTIME,
        "update_units": " ".join(update_package.UNIT_NAMES),
        "update_unit_paths": " ".join(update_package.UNIT_PATHS),
        "nginx_config_sha256": _digest(cutover.render_nginx(baseline)),
        "nginx_unit_target": cutover.UNIT_TARGET,
        "nginx_unit_sha256": _digest(cutover.render_unit(baseline)),
        "certificate_path": baseline["certificate_path"],
        "certificate_sha256": baseline["certificate_sha256"],
        "key_path": baseline["key_path"],
        "key_sha256": baseline["key_sha256"],
    }
    if preflight:
        values.update(preflight)
    return values


def _config_exact() -> str:
    return r"""
[ -f "$config_target" ] && [ ! -L "$config_target" ]
[ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ]
[ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]
[ -z "$(find "$config_parent" -maxdepth 1 -type f -name '.update-supervisor-*' -print -quit)" ]
""".strip()


def _common_preflight() -> str:
    return rf"""
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
for tool in awk curl dpkg-query find findmnt getent pgrep rm sha256sum ss stat sync systemctl; do command -v "$tool" >/dev/null 2>&1; done
for process in dpkg apt apt-get unattended-upgrade; do pgrep -x "$process" >/dev/null 2>&1 && exit 1; done
case ",$(findmnt -n -o OPTIONS /)," in *,rw,*) ;; *) exit 1 ;; esac
[ "$(dpkg-query -W -f='${{db:Status-Abbrev}}:${{Version}}:${{Architecture}}' "$package_name" 2>/dev/null || true)" = "ii :$package_version:arm64" ]
{_config_exact()}
update_runtime_absent
update_units_inert
persistent_runtime_healthy
config_parent_ready
trust_guards_absent
[ "$(sha256sum /etc/hardened-blikvm/nginx.conf | awk '{{print $1}}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{{print $1}}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{{print $1}}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{{print $1}}')" = "$key_sha256" ]
""".strip()


def build_preflight_script(
    enrollment: AcceptedEnrollment, baseline: Mapping[str, str],
    *, update_version: str = update_builder.PACKAGE_VERSION,
) -> str:
    if update_version not in update_builder.SUPPORTED_PACKAGE_VERSIONS:
        raise ValueError("unsupported update package preflight version")
    values = _values(enrollment, baseline)
    values["package_version"] = update_version
    body = rf"""
set -eu
export LC_ALL=C
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
{_common_preflight()}
account_hash=$(sha256sum "$state_root/account.json" | awk '{{print $1}}')
broker_pid=$(systemctl show -p MainPID --value hardened-blikvm-platform-broker.service)
video_pid=$(systemctl show -p MainPID --value hardened-blikvm-video-proxy.service)
server_pid=$(systemctl show -p MainPID --value hardened-blikvm-server.service)
nginx_pid=$(systemctl show -p MainPID --value hardened-blikvm-nginx.service)
for value in "$broker_pid" "$video_pid" "$server_pid" "$nginx_pid"; do case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac; done
emit architecture aarch64
emit rootfs_mode rw
emit config_exact yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_healthy yes
emit guards_absent yes
emit control_boundary_absent yes
emit tls_unchanged yes
emit account_sha256 "$account_hash"
emit broker_pid "$broker_pid"
emit broker_restarts "$(unit_number hardened-blikvm-platform-broker.service NRestarts)"
emit video_pid "$video_pid"
emit video_restarts "$(unit_number hardened-blikvm-video-proxy.service NRestarts)"
emit server_pid "$server_pid"
emit server_restarts "$(unit_number hardened-blikvm-server.service NRestarts)"
emit hardened_nginx_pid "$nginx_pid"
emit hardened_nginx_restarts "$(unit_number hardened-blikvm-nginx.service NRestarts)"
""".strip()
    return f"{_assignments(values)}\n{trust_gate.REMOTE_HELPERS}\n{body}"


def _unchanged_checks() -> str:
    return rf"""
{trust_gate.runtime_unchanged_checks()}
[ "$(sha256sum "$state_root/account.json" | awk '{{print $1}}')" = "$account_sha256" ]
[ "$(sha256sum /etc/hardened-blikvm/nginx.conf | awk '{{print $1}}')" = "$nginx_config_sha256" ]
[ "$(sha256sum "$nginx_unit_target" | awk '{{print $1}}')" = "$nginx_unit_sha256" ]
[ "$(sha256sum "$certificate_path" | awk '{{print $1}}')" = "$certificate_sha256" ]
[ "$(sha256sum "$key_path" | awk '{{print $1}}')" = "$key_sha256" ]
update_runtime_absent
update_units_inert
persistent_runtime_healthy
trust_guards_absent
""".strip()


def build_remove_script(
    enrollment: AcceptedEnrollment,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    values = _values(enrollment, baseline, preflight)
    body = rf"""
set -eu
export LC_ALL=C
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
{_common_preflight()}
{_unchanged_checks()}
rm -f -- "$config_target"
sync "$config_parent"
[ ! -e "$config_target" ] && [ ! -L "$config_target" ]
{_unchanged_checks()}
emit config_absent yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit tls_unchanged yes
emit guards_absent yes
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{trust_gate.REMOTE_HELPERS}\n{body}"


def build_final_script(
    enrollment: AcceptedEnrollment,
    baseline: Mapping[str, str],
    preflight: Mapping[str, str],
) -> str:
    values = _values(enrollment, baseline, preflight)
    body = rf"""
set -eu
export LC_ALL=C
emit() {{ printf '%s=%s\n' "$1" "$2"; }}
[ "$(id -u)" = 0 ] && [ "$(uname -m)" = aarch64 ]
[ ! -e "$config_target" ] && [ ! -L "$config_target" ]
config_parent_ready
{_unchanged_checks()}
emit config_absent yes
emit update_runtime_absent yes
emit update_units_inert yes
emit persistent_runtime_unchanged yes
emit account_unchanged yes
emit tls_unchanged yes
emit guards_absent yes
emit control_boundary_absent yes
""".strip()
    return f"{_assignments(values)}\n{trust_gate.REMOTE_HELPERS}\n{body}"


def build_classify_script(enrollment: AcceptedEnrollment) -> str:
    values = {
        "config_target": CONFIG_TARGET,
        "config_sha256": enrollment.config_sha256,
    }
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
if [ ! -e "$config_target" ] && [ ! -L "$config_target" ]; then
  emit config_state absent
elif [ -f "$config_target" ] && [ ! -L "$config_target" ] &&
     [ "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1 ] &&
     [ "$(sha256sum "$config_target" | awk '{print $1}')" = "$config_sha256" ]; then
  emit config_state old
else
  emit config_state unknown
fi
""".strip()
    return f"{_assignments(values)}\n{body}"


def parse_facts(raw: bytes, expected: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("unused trust removal output exceeded the limit")
    facts: dict[str, str] = {}
    for line in raw.decode("ascii", errors="strict").splitlines():
        key, separator, value = line.partition("=")
        if (
            separator != "="
            or key not in expected
            or key in facts
            or SAFE_VALUE.fullmatch(value) is None
        ):
            raise ValueError("unused trust removal returned an invalid fact")
        facts[key] = value
    if set(facts) != expected:
        raise ValueError("unused trust removal result was incomplete")
    return dict(sorted(facts.items()))


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def main() -> int:
    options = parse_args()
    output = prototype.validate_output_path(options.output)
    enrollment = load_enrollment_receipt(
        options.enrollment_receipt, options.expected_config_sha256
    )
    baseline = cutover.load_baseline(options.baseline)
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    preflight: dict[str, str] = {}
    try:
        result = prototype.run_ssh(
            target,
            secret,
            build_preflight_script(enrollment, baseline),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("unused trust removal preflight failed")
        preflight = parse_facts(result.stdout, PRE_KEYS)
        if options.preflight_only:
            write_json_0600_atomic(
                output,
                {
                    "schemaVersion": 1,
                    "probe": "hardened-blikvm-unused-update-trust-removal-preflight",
                    "result": "passed",
                    "configSha256": enrollment.config_sha256,
                    "facts": preflight,
                    "policy": {
                        "deviceChanged": False,
                        "configRemoved": False,
                        "stateInitialized": False,
                        "unitsEnabled": False,
                        "privateKeyAccepted": False,
                        "deviceReboot": False,
                        "targetPowerControl": False,
                        "secretEmitted": False,
                    },
                },
            )
            return 0
        stage = "atomic-unlink"
        result = prototype.run_ssh(
            target,
            secret,
            build_remove_script(enrollment, baseline, preflight),
            timeout=120,
        )
        if result.returncode != 0:
            raise ValueError("unused trust config removal failed")
        _ = parse_facts(result.stdout, FINAL_KEYS)
        stage = "independent-final"
        result = prototype.run_ssh(
            target,
            secret,
            build_final_script(enrollment, baseline, preflight),
            timeout=90,
        )
        if result.returncode != 0:
            raise ValueError("unused trust removal final verification failed")
        final_state = parse_facts(result.stdout, FINAL_KEYS)
        receipt: RemovalReceipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-unused-update-trust-removal",
            "result": "passed",
            "removedConfigSha256": enrollment.config_sha256,
            "priorRequestSha256": enrollment.request_sha256,
            "sourceCommit": enrollment.source_commit,
            "preflight": preflight,
            "finalState": final_state,
            "policy": {
                "atomicExactConfigUnlink": True,
                "oldPrivateKeyTouched": False,
                "stateInitialized": False,
                "unitsEnabled": False,
                "runtimeRestarted": False,
                "deviceReboot": False,
                "targetPowerControl": False,
                "secretEmitted": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        config_state = "unknown"
        try:
            result = prototype.run_ssh(
                target, secret, build_classify_script(enrollment), timeout=30
            )
            if result.returncode == 0:
                config_state = parse_facts(result.stdout, CLASSIFY_KEYS)["config_state"]
        except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
            pass
        write_json_0600_atomic(
            output,
            {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-unused-update-trust-removal",
                "result": "failed",
                "failure": {
                    "stage": stage,
                    "failureType": type(error).__name__,
                    "configState": config_state,
                },
                "policy": {
                    "stateInitialized": False,
                    "unitsEnabled": False,
                    "deviceReboot": False,
                    "targetPowerControl": False,
                    "secretEmitted": False,
                },
            },
        )
        return 1
    finally:
        _zero(secret)


if __name__ == "__main__":
    raise SystemExit(main())
