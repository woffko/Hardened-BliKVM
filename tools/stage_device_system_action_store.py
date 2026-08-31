#!/usr/bin/env python3
"""Run the system-action store unit harness in a bounded H616 /tmp stage.

The SSH target is derived from the private known-hosts file and the password is
read once from stdin. The password reaches sshpass only through a dedicated
inherited file descriptor. Raw remote output, routing, and credentials never
enter the sanitized receipt.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import ipaddress
import os
from pathlib import Path
import re
import secrets
import shlex
import stat
import struct
import subprocess
import sys
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


APPROVED_BUILD_ROOT = Path("/mnt/d/debug/blikvm/hardened-blikvm")
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-system-action-store-harness-[0-9a-f]{64}$"
)
PRODUCTION_STORE_ROOT = "/var/lib/hardened-blikvm-system-actions"
MAX_HARNESS_BYTES = 16 * 1024 * 1024
MIN_TMP_HEADROOM_BYTES = 8 * 1024 * 1024
EXPECTED_TESTS = 15
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
SAFE_VALUE = re.compile(r"^[A-Za-z0-9_.:+/()=-]{1,240}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-system-action-store-[0-9a-f]{12}-[0-9a-f]{16}$"
)

PREFLIGHT_KEYS = {
    "architecture",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "production_root_absent_before",
    "rootfs_mode_before",
    "stage_absent_before",
    "tmp_exec",
    "tools_ready",
}
RUN_KEYS = {
    "architecture",
    "harness_sha256",
    "harness_size_bytes",
    "kvmd_active_after",
    "kvmd_active_before",
    "kvmd_nginx_active_after",
    "kvmd_nginx_active_before",
    "production_root_absent_after",
    "production_root_absent_before",
    "rootfs_mode_after",
    "rootfs_mode_before",
    "tests_failed",
    "tests_ignored",
    "tests_passed",
}
FINAL_KEYS = {
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "production_root_absent_final",
    "rootfs_mode_final",
    "stage_absent_final",
}
CLEANUP_KEYS = {"cleanup_removed"}


@dataclass(frozen=True)
class Options:
    harness: str
    source_commit: str
    known_hosts: str
    output: str
    endpoint: str | None


@dataclass(frozen=True)
class HarnessInfo:
    path: Path
    sha256: str
    size_bytes: int
    source_commit: str


@dataclass(frozen=True)
class StagePaths:
    root: str
    marker: str
    harness: str
    temporary_root: str
    log: str
    marker_value: str


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--harness", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument("--endpoint")
    parser.add_argument(
        "--output", default="evidence/private/device-system-action-store-native.json"
    )
    namespace = parser.parse_args()
    return Options(
        harness=cast(str, namespace.harness),
        source_commit=cast(str, namespace.source_commit),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        endpoint=cast(str | None, namespace.endpoint),
    )


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def validate_harness(path_value: str, source_commit: str) -> HarnessInfo:
    if not SOURCE_COMMIT.fullmatch(source_commit):
        raise ValueError("source commit is invalid")
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("harness must not be a symlink")
    path = source.resolve(strict=True)
    approved = APPROVED_BUILD_ROOT.resolve(strict=True)
    approved_snapshot = (
        path.parent == Path("/tmp")
        and LOCAL_SNAPSHOT.fullmatch(path.name) is not None
    )
    if not path.is_relative_to(approved) and not approved_snapshot:
        raise ValueError("harness is outside the approved build root")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_HARNESS_BYTES
        or metadata.st_mode & 0o022
        or metadata.st_nlink != 1
        or metadata.st_uid != os.geteuid()
    ):
        raise ValueError("harness metadata is unsafe")
    with path.open("rb") as handle:
        header = handle.read(64)
        if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01":
            raise ValueError("harness is not ELF64 little-endian")
        elf_type, machine = struct.unpack_from("<HH", header, 16)
        program_offset = struct.unpack_from("<Q", header, 32)[0]
        program_entry_size = struct.unpack_from("<H", header, 54)[0]
        program_count = struct.unpack_from("<H", header, 56)[0]
        if elf_type != 3 or machine != 183 or program_entry_size < 56 or program_count == 0:
            raise ValueError("harness is not an AArch64 PIE executable")
        handle.seek(program_offset)
        program_types: list[int] = []
        for _ in range(program_count):
            entry = handle.read(program_entry_size)
            if len(entry) != program_entry_size:
                raise ValueError("harness program headers are truncated")
            program_types.append(struct.unpack_from("<I", entry)[0])
        if 3 in program_types or 1 not in program_types:
            raise ValueError("harness is not a static PIE executable")
    sha256 = hash_file(path)
    if approved_snapshot and path.name != (
        f"hardened-blikvm-system-action-store-harness-{sha256}"
    ):
        raise ValueError("harness snapshot name does not match its SHA-256")
    return HarnessInfo(path, sha256, metadata.st_size, source_commit)


def target_from_known_hosts(
    path_value: str, endpoint: str | None = None
) -> prototype.RemoteTarget:
    known_hosts = prototype.validate_private_file(path_value)
    if endpoint is not None:
        host, port = prototype.parse_endpoint(endpoint)
        return prototype.RemoteTarget(host, port, "root", known_hosts)
    if known_hosts.stat().st_size > 1024 * 1024:
        raise ValueError("known-hosts exceeded the size limit")
    endpoints: set[tuple[str, int]] = set()
    for line in known_hosts.read_text(encoding="ascii").splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        fields = stripped.split()
        if len(fields) < 3 or fields[0].startswith("@"):
            raise ValueError("known-hosts contains an unsupported line")
        for host_field in fields[0].split(","):
            if host_field.startswith("|") or "*" in host_field or "?" in host_field:
                raise ValueError("known-hosts routing is not explicit")
            port = 22
            host = host_field
            if host_field.startswith("["):
                closing = host_field.rfind("]:")
                if closing <= 1:
                    raise ValueError("known-hosts endpoint is invalid")
                host = host_field[1:closing]
                try:
                    port = int(host_field[closing + 2 :])
                except ValueError as error:
                    raise ValueError("known-hosts port is invalid") from error
            try:
                parsed = ipaddress.ip_address(host)
            except ValueError as error:
                raise ValueError("known-hosts host must be an IP literal") from error
            if not 1 <= port <= 65_535:
                raise ValueError("known-hosts port is invalid")
            endpoints.add((str(parsed), port))
    if len(endpoints) != 1:
        raise ValueError("known-hosts must identify exactly one endpoint")
    host, port = endpoints.pop()
    return prototype.RemoteTarget(host, port, "root", known_hosts)


def stage_paths(harness_sha256: str, run_id: str) -> StagePaths:
    if not SHA256.fullmatch(harness_sha256) or not RUN_ID.fullmatch(run_id):
        raise ValueError("staging identity is invalid")
    root = (
        f"/tmp/hardened-blikvm-system-action-store-"
        f"{harness_sha256[:12]}-{run_id}"
    )
    if not STAGE_ROOT.fullmatch(root):
        raise ValueError("staging root is invalid")
    marker_value = f"system-action-store:{harness_sha256}:{run_id}"
    return StagePaths(
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        harness=f"{root}/store-tests",
        temporary_root=f"{root}/tmp",
        log=f"{root}/test.log",
        marker_value=marker_value,
    )


def assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def build_preflight_script(paths: StagePaths, required_bytes: int) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "required_bytes": required_bytes,
            "production_root": PRODUCTION_STORE_ROOT,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) tmp_exec=yes ;; esac
tmp_available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
case "$tmp_available" in ''|*[!0-9]*) exit 1 ;; esac
[ "$tmp_available" -ge "$required_bytes" ]
[ "$(uname -m)" = aarch64 ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ ! -e "$production_root" ]
[ ! -e "$stage_root" ]
for tool in cat chmod df findmnt grep mkdir mv rm sha256sum stat systemctl tail tr uname; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit architecture aarch64
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit production_root_absent_before yes
emit rootfs_mode_before "$rootfs_mode"
emit stage_absent_before yes
emit tmp_exec "$tmp_exec"
emit tools_ready yes
""".strip()
    return f"{values}\n{body}"


def build_upload_script(paths: StagePaths) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "harness": paths.harness,
            "temporary_root": paths.temporary_root,
        }
    )
    body = r"""
set -eu
umask 077
case "$stage_root" in /tmp/hardened-blikvm-system-action-store-*) ;; *) exit 1 ;; esac
[ ! -e "$stage_root" ]
mkdir -m 700 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
mkdir -m 700 "$temporary_root"
cat > "$harness.part"
[ -s "$harness.part" ]
mv "$harness.part" "$harness"
chmod 700 "$harness"
""".strip()
    return f"{values}\n{body}"


def build_run_script(paths: StagePaths, harness: HarnessInfo) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "harness": paths.harness,
            "temporary_root": paths.temporary_root,
            "test_log": paths.log,
            "expected_sha256": harness.sha256,
            "expected_size": harness.size_bytes,
            "production_root": PRODUCTION_STORE_ROOT,
            "expected_tests": EXPECTED_TESTS,
        }
    )
    body = r"""
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
cleanup() {
  case "$stage_root" in /tmp/hardened-blikvm-system-action-store-*) ;; *) return 1 ;; esac
  if [ -f "$marker" ] && [ "$(cat "$marker")" = "$marker_value" ]; then
    rm -rf -- "$stage_root"
  fi
}
trap cleanup EXIT HUP INT TERM
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$harness" ]
[ ! -L "$harness" ]
[ "$(stat -c %a "$harness")" = 700 ]
[ "$(stat -c %s "$harness")" = "$expected_size" ]
[ "$(sha256sum "$harness" | cut -d ' ' -f 1)" = "$expected_sha256" ]
[ ! -e "$production_root" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_before=rw ;; *) exit 1 ;; esac
set +e
TMPDIR="$temporary_root" "$harness" --test-threads=1 > "$test_log" 2>&1
test_status=$?
set -e
[ "$test_status" -eq 0 ]
result_line=$(grep '^test result: ok\. ' "$test_log" | tail -n 1)
case "$result_line" in
  "test result: ok. $expected_tests passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in "*) ;;
  *) exit 1 ;;
esac
[ ! -e "$production_root" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_after=rw ;; *) exit 1 ;; esac
emit architecture aarch64
emit harness_sha256 "$expected_sha256"
emit harness_size_bytes "$expected_size"
emit kvmd_active_after active
emit kvmd_active_before active
emit kvmd_nginx_active_after active
emit kvmd_nginx_active_before active
emit production_root_absent_after yes
emit production_root_absent_before yes
emit rootfs_mode_after "$rootfs_after"
emit rootfs_mode_before "$rootfs_before"
emit tests_failed 0
emit tests_ignored 0
emit tests_passed "$expected_tests"
""".strip()
    return f"{values}\n{body}"


def build_cleanup_script(paths: StagePaths) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
        }
    )
    body = r"""
set -eu
case "$stage_root" in /tmp/hardened-blikvm-system-action-store-*) ;; *) exit 1 ;; esac
if [ -e "$stage_root" ]; then
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  rm -rf -- "$stage_root"
fi
[ ! -e "$stage_root" ]
printf 'cleanup_removed=yes\n'
""".strip()
    return f"{values}\n{body}"


def build_final_script(paths: StagePaths) -> str:
    values = assignments(
        {"stage_root": paths.root, "production_root": PRODUCTION_STORE_ROOT}
    )
    body = r"""
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$production_root" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit production_root_absent_final yes
emit rootfs_mode_final "$rootfs_mode"
emit stage_absent_final yes
""".strip()
    return f"{values}\n{body}"


def require_result(
    stage: str, result: subprocess.CompletedProcess[bytes]
) -> subprocess.CompletedProcess[bytes]:
    if result.returncode != 0 or len(result.stdout) > 16 * 1024:
        raise RuntimeError(stage)
    return result


def parse_facts(raw: bytes, expected_keys: set[str]) -> dict[str, str]:
    if len(raw) > 16 * 1024:
        raise ValueError("remote output exceeded the size limit")
    decoded = raw.decode("ascii")
    facts: dict[str, str] = {}
    for line in decoded.splitlines():
        if not line or "=" not in line:
            raise ValueError("remote output is malformed")
        key, value = line.split("=", 1)
        if key not in expected_keys or key in facts or not SAFE_VALUE.fullmatch(value):
            raise ValueError("remote output is not allowlisted")
        facts[key] = value
    if set(facts) != expected_keys:
        raise ValueError("remote output is incomplete")
    return dict(sorted(facts.items()))


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "architecture": "aarch64",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "production_root_absent_before": "yes",
        "rootfs_mode_before": "rw",
        "stage_absent_before": "yes",
        "tmp_exec": "yes",
        "tools_ready": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("preflight invariant failed")


def validate_run(facts: dict[str, str], harness: HarnessInfo) -> None:
    expected = {
        "architecture": "aarch64",
        "harness_sha256": harness.sha256,
        "harness_size_bytes": str(harness.size_bytes),
        "kvmd_active_after": "active",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_after": "active",
        "kvmd_nginx_active_before": "active",
        "production_root_absent_after": "yes",
        "production_root_absent_before": "yes",
        "rootfs_mode_after": "rw",
        "rootfs_mode_before": "rw",
        "tests_failed": "0",
        "tests_ignored": "0",
        "tests_passed": str(EXPECTED_TESTS),
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("native test invariant failed")


def validate_final(facts: dict[str, str]) -> None:
    expected = {
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
        "production_root_absent_final": "yes",
        "rootfs_mode_final": "rw",
        "stage_absent_final": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def zero(secret: bytearray) -> None:
    for index in range(len(secret)):
        secret[index] = 0


def attempt_cleanup(
    target: prototype.RemoteTarget, secret: bytearray, paths: StagePaths
) -> CleanupReceipt:
    try:
        result = prototype.run_ssh(
            target, secret, build_cleanup_script(paths), timeout=20
        )
        if result.returncode != 0:
            return {"attempted": True, "confirmed": False}
        facts = parse_facts(result.stdout, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_removed": "yes"}}
    except (OSError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    harness = validate_harness(options.harness, options.source_commit)
    target = target_from_known_hosts(options.known_hosts, options.endpoint)
    paths = stage_paths(harness.sha256, secrets.token_hex(8))
    required_bytes = harness.size_bytes * 4 + MIN_TMP_HEADROOM_BYTES
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    cleanup_required = False
    try:
        preflight_result = require_result(
            stage,
            prototype.run_ssh(
                target,
                secret,
                build_preflight_script(paths, required_bytes),
                timeout=20,
            ),
        )
        preflight = parse_facts(preflight_result.stdout, PREFLIGHT_KEYS)
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        with harness.path.open("rb") as harness_stream:
            require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_upload_script(paths),
                    stdin=harness_stream,
                    timeout=60,
                ),
            )

        stage = "native-test"
        run_result = require_result(
            stage,
            prototype.run_ssh(
                target, secret, build_run_script(paths, harness), timeout=120
            ),
        )
        checks = parse_facts(run_result.stdout, RUN_KEYS)
        validate_run(checks, harness)

        stage = "final-state"
        final_result = require_result(
            stage,
            prototype.run_ssh(target, secret, build_final_script(paths), timeout=20),
        )
        final_state = parse_facts(final_result.stdout, FINAL_KEYS)
        validate_final(final_state)
        cleanup_required = False

        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-system-action-store-native",
            "result": "passed",
            "harness": {
                "sha256": harness.sha256,
                "sizeBytes": harness.size_bytes,
                "sourceCommit": harness.source_commit,
                "target": "aarch64-unknown-linux-musl",
                "staticPie": True,
            },
            "policy": {
                "deviceActionsExecuted": False,
                "persistentPathsChanged": False,
                "productionRootTouched": False,
                "rootfsHelpersInvoked": False,
                "stagingFilesystem": "/tmp",
            },
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            attempt_cleanup(target, secret, paths)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-system-action-store-native",
            "result": "failed",
            "harness": {
                "sha256": harness.sha256,
                "sizeBytes": harness.size_bytes,
                "sourceCommit": harness.source_commit,
            },
            "policy": {
                "deviceActionsExecuted": False,
                "persistentPathsChanged": False,
                "productionRootTouched": False,
                "rootfsHelpersInvoked": False,
                "stagingFilesystem": "/tmp",
            },
            "failure": {"stage": stage, "failureType": type(error).__name__},
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
