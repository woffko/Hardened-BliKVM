#!/usr/bin/env python3
"""Exercise provisioning prediction and rollback without production mutation."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import secrets
import subprocess
import sys
import tarfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_debian_runtime_retain as retain
from tools import stage_device_prototype as prototype
from tools import stage_device_runtime_provision as provision
from tools.capture_pikvm_baseline import write_json_0600_atomic


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    package: str
    expected_sha256: str
    expected_source_commit: str
    expected_source_bundle_sha256: str
    expected_builder_commit: str
    output: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--known-hosts", default="evidence/private/ssh-known-hosts")
    parser.add_argument("--package", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--expected-source-commit", required=True)
    parser.add_argument("--expected-source-bundle-sha256", required=True)
    parser.add_argument("--expected-builder-commit", required=True)
    parser.add_argument("--output", required=True)
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        package=cast(str, namespace.package),
        expected_sha256=cast(str, namespace.expected_sha256),
        expected_source_commit=cast(str, namespace.expected_source_commit),
        expected_source_bundle_sha256=cast(
            str, namespace.expected_source_bundle_sha256
        ),
        expected_builder_commit=cast(str, namespace.expected_builder_commit),
        output=cast(str, namespace.output),
    )


def _provision_options(options: Options) -> provision.Options:
    return provision.Options(
        options.endpoint,
        options.username,
        options.known_hosts,
        options.package,
        options.expected_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
        options.output,
    )


def _zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = provision.validate_assets(_provision_options(options))
    host, port = prototype.parse_endpoint(options.endpoint)
    username = prototype.validate_username(options.username)
    known_hosts = prototype.validate_private_file(options.known_hosts)
    target = prototype.RemoteTarget(host, port, username, known_hosts)
    retained_paths = retain.remote_paths(assets.package.sha256, secrets.token_hex(8))
    paths = provision.remote_paths(assets.sysusers.sha256, secrets.token_hex(8))
    secret = prototype.read_secret_stdin()
    cleanup_required = False
    try:
        retained_result = prototype.run_ssh(
            target,
            secret,
            retain.build_final_script(retained_paths, assets.package),
            timeout=45,
        )
        if retained_result.returncode != 0:
            raise ValueError("retained package precondition failed")
        retained_before = retain.parse_facts(retained_result.stdout, retain.FINAL_KEYS)
        preflight_result = prototype.run_ssh(
            target, secret, provision.build_preflight_script(paths, assets), timeout=60
        )
        if preflight_result.returncode != 0:
            phase, _ = provision.parse_failure_phase(
                preflight_result.stdout, "preflight"
            )
            raise ValueError(f"runtime provisioning preflight failed at {phase}")
        preflight = provision.parse_facts(
            preflight_result.stdout, provision.PREFLIGHT_KEYS
        )
        cleanup_required = True
        prepare_result = prototype.run_ssh(
            target, secret, provision.build_prepare_script(paths, assets), timeout=180
        )
        if prepare_result.returncode != 0:
            phase, _ = provision.parse_failure_phase(prepare_result.stdout, "prepare")
            raise ValueError(f"runtime provisioning prepare failed at {phase}")
        prepared = provision.parse_facts(prepare_result.stdout, provision.PREPARE_KEYS)
        cleanup_result = prototype.run_ssh(
            target, secret, provision.build_cleanup_script(paths, assets), timeout=180
        )
        if cleanup_result.returncode != 0:
            raise ValueError("runtime provisioning prepare cleanup failed")
        cleanup = provision.parse_facts(cleanup_result.stdout, provision.CLEANUP_KEYS)
        cleanup_required = False
        final_result = prototype.run_ssh(
            target,
            secret,
            retain.build_final_script(retained_paths, assets.package),
            timeout=45,
        )
        if final_result.returncode != 0:
            raise ValueError("runtime provisioning prepare final state failed")
        final_state = retain.parse_facts(final_result.stdout, retain.FINAL_KEYS)
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-prepare-gate",
            "result": "passed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "policy": {
                "productionMutation": False,
                "temporaryRollbackArtifacts": True,
                "accountsProvisioned": False,
                "credentialsProvisioned": False,
                "servicesActive": False,
                "servicesEnabled": False,
                "hardwareMutation": False,
                "physicalActionsInvoked": False,
                "secretEmitted": False,
            },
            "retainedBefore": retained_before,
            "preflight": preflight,
            "prepared": prepared,
            "cleanup": cleanup,
            "finalState": final_state,
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError) as error:
        cleanup_confirmed = not cleanup_required
        if cleanup_required:
            try:
                result = prototype.run_ssh(
                    target,
                    secret,
                    provision.build_cleanup_script(paths, assets),
                    timeout=180,
                )
                facts = (
                    provision.parse_facts(result.stdout, provision.CLEANUP_KEYS)
                    if result.returncode == 0
                    else {}
                )
                cleanup_confirmed = facts == {"cleanup_restored": "yes"}
            except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
                cleanup_confirmed = False
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-prepare-gate",
            "result": "failed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "failure": {
                "stage": str(error)[:96],
                "failureType": type(error).__name__,
            },
            "cleanup": {
                "attempted": cleanup_required,
                "confirmed": cleanup_confirmed,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        _zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (
        OSError,
        UnicodeError,
        ValueError,
        subprocess.SubprocessError,
        tarfile.TarError,
    ):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
