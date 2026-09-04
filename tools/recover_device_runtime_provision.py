#!/usr/bin/env python3
"""Replace and execute one marker-bound provisioning rollback after a fixed bug."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
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
    secret = prototype.read_secret_stdin()
    discovery: dict[str, str] | None = None
    try:
        discovery_result = prototype.run_ssh(
            target,
            secret,
            provision.build_recovery_discovery_script(assets),
            timeout=60,
        )
        if discovery_result.returncode != 0:
            raise ValueError("runtime provisioning recovery discovery failed")
        discovery = provision.parse_recovery_facts(discovery_result.stdout)
        if (
            discovery["rollback_state"] != "ready"
            or discovery["boot_state"] != "armed"
            or discovery["database_state"] not in {"before", "after", "mixed"}
            or discovery["htpasswd_state"] not in {"before", "after"}
            or discovery["config_state"] not in {"absent", "staged", "present"}
            or discovery["package_state"] != "installed"
            or discovery["unit_state"] != "inactive"
            or discovery["kvmd_state"] != "active"
            or discovery["rootfs_mode"] != "rw"
        ):
            raise ValueError("runtime provisioning recovery state is not safe")
        paths = provision.remote_paths(assets.sysusers.sha256, discovery["run_id"])
        rollback = provision.build_rollback_script(paths, assets).encode()
        rollback_sha256 = hashlib.sha256(rollback).hexdigest()
        with tempfile.TemporaryFile(
            prefix="hardened-blikvm-rollback.", dir="/tmp"
        ) as rollback_stream:
            _ = rollback_stream.write(rollback)
            _ = rollback_stream.seek(0)
            upload_result = prototype.run_ssh(
                target,
                secret,
                provision.build_rollback_replacement_script(
                    paths, assets, rollback_sha256, len(rollback)
                ),
                stdin=rollback_stream,
                timeout=60,
            )
        if upload_result.returncode != 0:
            raise ValueError("runtime provisioning rollback replacement failed")
        replaced = provision.parse_facts(
            upload_result.stdout, {"rollback_replaced"}
        )
        recovery_result = prototype.run_ssh(
            target,
            secret,
            provision.build_execute_recovery_script(
                paths, assets, rollback_sha256
            ),
            timeout=180,
        )
        if recovery_result.returncode != 0:
            raise ValueError("runtime provisioning fixed rollback failed")
        executed = provision.parse_facts(
            recovery_result.stdout, provision.RECOVERY_EXECUTE_KEYS
        )
        retained_paths = retain.remote_paths(assets.package.sha256, "0" * 16)
        final_result = prototype.run_ssh(
            target,
            secret,
            retain.build_final_script(retained_paths, assets.package),
            timeout=60,
        )
        if final_result.returncode != 0:
            raise ValueError("runtime provisioning recovery final state failed")
        final_state = retain.parse_facts(final_result.stdout, retain.FINAL_KEYS)
        receipt: dict[str, object] = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-recovery",
            "result": "recovered",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "discovery": discovery,
            "replacement": replaced,
            "execution": executed,
            "finalState": final_state,
            "policy": {
                "markerBound": True,
                "rollbackOnly": True,
                "newProvisioningMutation": False,
                "secretEmitted": False,
                "physicalActionsInvoked": False,
            },
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError) as error:
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-runtime-provisioning-recovery",
            "result": "failed",
            "packageSha256": assets.package.sha256,
            "sysusersSha256": assets.sysusers.sha256,
            "discovery": discovery,
            "failure": {
                "stage": str(error)[:96],
                "failureType": type(error).__name__,
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
