#!/usr/bin/env python3
"""Run a reversible transient diagnostic-collector gate on the test H616."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
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
from tools import stage_device_system_action_store as native


UNIT_NAME = "hardened-blikvm-diagnostic-collector.service"
COLLECTOR_ACCOUNT = "hardened-blikvm-diagnostics"
SHARED_GROUP = "hardened-blikvm"
RUNTIME_DIRECTORY = "/run/hardened-blikvm-diagnostics"
SOCKET_PATH = f"{RUNTIME_DIRECTORY}/collector.sock"
EXPORT_SOCKET_PATH = f"{RUNTIME_DIRECTORY}/export.sock"
BROKER_RUNTIME_DIRECTORY = "/run/hardened-blikvm"
BROKER_SOCKET_PATH = f"{BROKER_RUNTIME_DIRECTORY}/platform-broker.sock"
BINARY_PARENT = "/usr/libexec/hardened-blikvm"
BINARY_TARGET = f"{BINARY_PARENT}/bli-kvm-diagnostic-collector"
PRODUCER_ACCOUNTS = (
    "hardened-blikvm-server",
    "hardened-blikvm-broker",
    "hardened-blikvm-video",
    "hardened-blikvm-control",
    "hardened-blikvm-update",
)
ALL_ACCOUNTS = (*PRODUCER_ACCOUNTS, COLLECTOR_ACCOUNT)
ACCOUNT_DATABASE_FILES = ("passwd", "group", "shadow", "gshadow")
MAX_BINARY_BYTES = 16 * 1024 * 1024
MAX_CONFIG_BYTES = 64 * 1024
MIN_TMP_HEADROOM_BYTES = 16 * 1024 * 1024
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")
RUN_ID = re.compile(r"^[0-9a-f]{16}$")
STAGE_ROOT = re.compile(
    r"^/tmp/hardened-blikvm-diagnostic-collector-[0-9a-f]{12}-[0-9a-f]{16}$"
)
LOCAL_SNAPSHOT = re.compile(
    r"^hardened-blikvm-diagnostic-collector-[0-9a-f]{64}$"
)
LOCAL_BROKER_SNAPSHOT = re.compile(
    r"^hardened-blikvm-platform-broker-diagnostic-[0-9a-f]{64}$"
)
PREFLIGHT_KEYS = {
    "account_absent_before",
    "architecture",
    "binary_absent_before",
    "binary_parent_before",
    "broker_runtime_absent_before",
    "broker_socket_absent_before",
    "broker_unit_absent_before",
    "account_database_ready",
    "identities_absent_before",
    "kvmd_active_before",
    "kvmd_nginx_active_before",
    "rootfs_mode_before",
    "runtime_absent_before",
    "export_socket_absent_before",
    "stage_absent_before",
    "systemd_ready",
    "tools_ready",
    "unit_absent_before",
    "watchdog_absent_before",
}
RUN_KEYS = {
    "binary_hash_verified",
    "broker_binary_hash_verified",
    "broker_caps",
    "broker_exact_reader",
    "broker_identity",
    "broker_matches_export",
    "broker_network_namespace",
    "broker_no_ack",
    "broker_no_new_privs",
    "broker_repeatable",
    "broker_runtime_removed",
    "broker_socket_mode",
    "broker_socket_owner",
    "broker_started_without_collector",
    "broker_unauthorized_rejected",
    "broker_unavailable_without_collector",
    "broker_unit",
    "collector_caps",
    "collector_identity",
    "collector_network_namespace",
    "collector_no_new_privs",
    "collector_unit",
    "control_rejection_accepted",
    "duplicate_rejected",
    "export_acknowledged",
    "export_exact_consumer",
    "export_malformed_rejected",
    "export_mismatch_preserved",
    "export_no_sealed_after_ack",
    "export_over_capacity_rejected",
    "export_partial_timeout",
    "export_repeatable",
    "export_socket_mode",
    "export_socket_owner",
    "export_unauthorized_rejected",
    "five_sources_accepted",
    "graceful_export_shutdown",
    "graceful_shutdown",
    "kvmd_active_after_run",
    "kvmd_nginx_active_after_run",
    "malformed_rejected",
    "partial_timeout",
    "producer_during_export_stall",
    "root_rejected",
    "rollback_confirmed",
    "runtime_removed",
    "socket_mode",
    "socket_owner",
    "sysusers_synthetic",
    "static_identities",
    "account_database_restored",
    "unit_hash_verified",
    "unknown_rejected",
    "watchdog_armed",
}
FINAL_KEYS = {
    "account_absent_final",
    "binary_absent_final",
    "binary_parent_final",
    "broker_runtime_absent_final",
    "broker_socket_absent_final",
    "broker_unit_absent_final",
    "kvmd_active_final",
    "kvmd_nginx_active_final",
    "rootfs_mode_final",
    "runtime_absent_final",
    "export_socket_absent_final",
    "stage_absent_final",
    "unit_absent_final",
    "watchdog_inactive_final",
}
FAILURE_KEYS = {"failure_phase"}
CLEANUP_KEYS = {"cleanup_restored"}
SAFE_PHASE = re.compile(r"^[a-z0-9-]{1,80}$")


class CleanupReceipt(TypedDict):
    attempted: bool
    confirmed: bool


class PolicyReceipt(TypedDict):
    accountMode: str
    deviceActionsExecuted: bool
    networkAccess: str
    persistentChangeScope: str
    persistentStateRestored: bool
    physicalPowerOrReset: bool
    rawLogsRead: bool
    rollbackWatchdog: str


@dataclass(frozen=True)
class Options:
    binary: str
    broker_binary: str
    source_commit: str
    unit: str
    base_sysusers: str
    sysusers: str
    endpoint: str
    known_hosts: str
    output: str


@dataclass(frozen=True)
class Asset:
    path: Path
    sha256: str
    size_bytes: int


@dataclass(frozen=True)
class Assets:
    binary: Asset
    broker_binary: Asset
    unit: Asset
    base_sysusers: Asset
    sysusers: Asset
    source_commit: str


@dataclass(frozen=True)
class Paths:
    run_id: str
    root: str
    marker: str
    marker_value: str
    binary: str
    broker_binary: str
    unit: str
    base_sysusers: str
    sysusers: str
    client: str
    rollback: str
    state: str
    unit_target: str
    binary_parent: str
    binary_target: str
    binary_part: str
    runtime_directory: str
    socket: str
    export_socket: str
    broker_runtime_directory: str
    broker_socket: str
    broker_credentials: str
    broker_unit: str
    client_state: str
    account_backup: str
    watchdog: str


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--broker-binary", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument(
        "--unit",
        default="packaging/systemd/hardened-blikvm-diagnostic-collector.service",
    )
    parser.add_argument(
        "--base-sysusers",
        default="packaging/sysusers.d/hardened-blikvm.conf",
    )
    parser.add_argument(
        "--sysusers",
        default="packaging/sysusers.d/hardened-blikvm-diagnostics.conf",
    )
    parser.add_argument("--endpoint", required=True)
    parser.add_argument(
        "--known-hosts", default="evidence/private/ssh-known-hosts"
    )
    parser.add_argument(
        "--output",
        default="evidence/private/device-diagnostic-collector.json",
    )
    namespace = parser.parse_args()
    return Options(
        binary=cast(str, namespace.binary),
        broker_binary=cast(str, namespace.broker_binary),
        source_commit=cast(str, namespace.source_commit),
        unit=cast(str, namespace.unit),
        base_sysusers=cast(str, namespace.base_sysusers),
        sysusers=cast(str, namespace.sysusers),
        endpoint=cast(str, namespace.endpoint),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
    )


def hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def hash_text(value: str) -> str:
    return hashlib.sha256(value.encode("utf-8")).hexdigest()


def validate_binary(path_value: str) -> Asset:
    return validate_elf_binary(
        path_value,
        LOCAL_SNAPSHOT,
        "hardened-blikvm-diagnostic-collector",
    )


def validate_broker_binary(path_value: str) -> Asset:
    return validate_elf_binary(
        path_value,
        LOCAL_BROKER_SNAPSHOT,
        "hardened-blikvm-platform-broker-diagnostic",
    )


def validate_elf_binary(
    path_value: str,
    snapshot_pattern: re.Pattern[str],
    expected_prefix: str,
) -> Asset:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("binary must not be a symlink")
    path = source.resolve(strict=True)
    if path.parent != Path("/tmp") or snapshot_pattern.fullmatch(path.name) is None:
        raise ValueError("binary must be an exact hash-named local snapshot")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_BINARY_BYTES
        or metadata.st_mode & 0o022
        or metadata.st_nlink != 1
        or metadata.st_uid != os.geteuid()
    ):
        raise ValueError("binary metadata is unsafe")
    with path.open("rb") as handle:
        header = handle.read(64)
        if len(header) != 64 or header[:6] != b"\x7fELF\x02\x01":
            raise ValueError("binary is not ELF64 little-endian")
        elf_type, machine = struct.unpack_from("<HH", header, 16)
        program_offset = struct.unpack_from("<Q", header, 32)[0]
        program_entry_size = struct.unpack_from("<H", header, 54)[0]
        program_count = struct.unpack_from("<H", header, 56)[0]
        if elf_type != 3 or machine != 183 or program_entry_size < 56 or program_count == 0:
            raise ValueError("binary is not an AArch64 PIE")
        handle.seek(program_offset)
        program_types: list[int] = []
        for _ in range(program_count):
            entry = handle.read(program_entry_size)
            if len(entry) != program_entry_size:
                raise ValueError("binary program headers are truncated")
            program_types.append(struct.unpack_from("<I", entry)[0])
        if 3 in program_types or 1 not in program_types:
            raise ValueError("binary is not static PIE")
    sha256 = hash_file(path)
    if path.name != f"{expected_prefix}-{sha256}":
        raise ValueError("binary snapshot name does not match SHA-256")
    return Asset(path, sha256, metadata.st_size)


def validate_config(path_value: str, expected: Path) -> Asset:
    source = Path(path_value)
    if source.is_symlink():
        raise ValueError("configuration source must not be a symlink")
    path = source.resolve(strict=True)
    if path != expected.resolve(strict=True):
        raise ValueError("configuration source path is not exact")
    metadata = path.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or not 0 < metadata.st_size <= MAX_CONFIG_BYTES
        or metadata.st_mode & 0o022
    ):
        raise ValueError("configuration source metadata is unsafe")
    return Asset(path, hash_file(path), metadata.st_size)


def validate_assets(options: Options) -> Assets:
    if SOURCE_COMMIT.fullmatch(options.source_commit) is None:
        raise ValueError("source commit is invalid")
    return Assets(
        binary=validate_binary(options.binary),
        broker_binary=validate_broker_binary(options.broker_binary),
        unit=validate_config(
            options.unit,
            Path("packaging/systemd/hardened-blikvm-diagnostic-collector.service"),
        ),
        base_sysusers=validate_config(
            options.base_sysusers,
            Path("packaging/sysusers.d/hardened-blikvm.conf"),
        ),
        sysusers=validate_config(
            options.sysusers,
            Path("packaging/sysusers.d/hardened-blikvm-diagnostics.conf"),
        ),
        source_commit=options.source_commit,
    )


def stage_paths(binary_sha256: str, broker_sha256: str, run_id: str) -> Paths:
    if (
        SHA256.fullmatch(binary_sha256) is None
        or SHA256.fullmatch(broker_sha256) is None
        or RUN_ID.fullmatch(run_id) is None
    ):
        raise ValueError("staging identity is invalid")
    root = f"/tmp/hardened-blikvm-diagnostic-collector-{binary_sha256[:12]}-{run_id}"
    if STAGE_ROOT.fullmatch(root) is None:
        raise ValueError("staging root is invalid")
    marker_value = (
        f"diagnostic-collector:{binary_sha256}:{broker_sha256}:{run_id}"
    )
    unit_target = f"/run/systemd/system/{UNIT_NAME}"
    return Paths(
        run_id=run_id,
        root=root,
        marker=f"{root}/.hardened-blikvm-owner",
        marker_value=marker_value,
        binary=f"{root}/collector",
        broker_binary=f"{root}/platform-broker",
        unit=f"{root}/collector.service",
        base_sysusers=f"{root}/base.sysusers",
        sysusers=f"{root}/diagnostics.sysusers",
        client=f"{root}/client.py",
        rollback=f"{root}/rollback.sh",
        state=f"{root}/rollback.state",
        unit_target=unit_target,
        binary_parent=BINARY_PARENT,
        binary_target=BINARY_TARGET,
        binary_part=f"{BINARY_TARGET}.part",
        runtime_directory=RUNTIME_DIRECTORY,
        socket=SOCKET_PATH,
        export_socket=EXPORT_SOCKET_PATH,
        broker_runtime_directory=BROKER_RUNTIME_DIRECTORY,
        broker_socket=BROKER_SOCKET_PATH,
        broker_credentials=f"{root}/broker-credentials",
        broker_unit=f"hardened-blikvm-diagnostic-broker-{run_id}.service",
        client_state=f"{root}/client-state",
        account_backup=f"{root}/account-backup",
        watchdog=f"hardened-blikvm-diagnostic-rollback-{run_id}",
    )


def assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def encode_text(value: str) -> str:
    return base64.b64encode(value.encode("utf-8")).decode("ascii")


def policy_receipt() -> PolicyReceipt:
    return {
        "accountMode": "temporary-sysusers-and-exact-account-db-restore",
        "deviceActionsExecuted": False,
        "networkAccess": "private-af-unix-only",
        "persistentChangeScope": "temporary-binary-and-rollback-required",
        "persistentStateRestored": True,
        "physicalPowerOrReset": False,
        "rawLogsRead": False,
        "rollbackWatchdog": "ten-minute-systemd",
    }


def build_client(paths: Paths) -> str:
    return f'''#!/usr/bin/python3
import os
import json
import socket
import stat
import sys
import time

SOCKET = {paths.socket!r}
EXPORT_SOCKET = {paths.export_socket!r}
BROKER_SOCKET = {paths.broker_socket!r}
READY = bytes((1, 1, 0, 0, 0, 0, 0, 0))
CONTROL_DISABLED = bytes((1, 3, 2, 0, 0, 0, 0, 0))
ACCEPTED = bytes((1, 1, 0, 0))
EXPORT_READ = bytes((1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0))
EXPORT_HEADER_BYTES = 24
EXPORT_MAX_PAYLOAD = 8192
STATE_PREFIX = {(paths.client_state + '/')!r}
SEALED_STATE = {(paths.client_state + '/sealed.bin')!r}
BROKER_STATE = {(paths.client_state + '/broker-sealed.bin')!r}
BROKER_REQUEST = b'{{"schemaVersion":1,"requestId":71,"request":{{"type":"diagnostics"}}}}'

def receive_exact(stream, size):
    output = bytearray()
    while len(output) < size:
        chunk = stream.recv(size - len(output))
        if not chunk:
            raise RuntimeError("early eof")
        output.extend(chunk)
    return bytes(output)

def expect_closed(stream, label):
    try:
        value = stream.recv(1)
    except ConnectionResetError:
        return
    if value != b"":
        raise RuntimeError(label)

def export_response(stream):
    header = receive_exact(stream, EXPORT_HEADER_BYTES)
    if header[0] != 1 or any(header[18:24]):
        raise RuntimeError("invalid export header")
    length = int.from_bytes(header[16:18], "big")
    if length > EXPORT_MAX_PAYLOAD:
        raise RuntimeError("export payload too large")
    payload = receive_exact(stream, length)
    return header + payload

def export_binding(response):
    if len(response) < EXPORT_HEADER_BYTES or response[1] != 2:
        raise RuntimeError("sealed response missing")
    instance = int.from_bytes(response[4:12], "big")
    window = int.from_bytes(response[12:16], "big")
    length = int.from_bytes(response[16:18], "big")
    if instance == 0 or window == 0 or response[3] == 0 or length == 0:
        raise RuntimeError("sealed response invalid")
    payload = response[EXPORT_HEADER_BYTES:]
    if len(payload) != length:
        raise RuntimeError("sealed response length mismatch")
    text = payload.decode("ascii")
    if not text.startswith("schema=1 record_count=") or text.count("\\n") < 2:
        raise RuntimeError("sealed payload invalid")
    return instance, window

def export_ack(instance, window):
    if instance == 0 or window == 0:
        raise RuntimeError("ack binding invalid")
    return bytes((1, 2, 0, 0)) + instance.to_bytes(8, "big") + window.to_bytes(4, "big")

def load_state():
    descriptor = os.open(SEALED_STATE, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        metadata = os.fstat(descriptor)
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1
                or metadata.st_uid != os.geteuid()
                or stat.S_IMODE(metadata.st_mode) != 0o600
                or metadata.st_size > 24 + EXPORT_MAX_PAYLOAD):
            raise RuntimeError("state metadata invalid")
        response = b""
        while len(response) <= 24 + EXPORT_MAX_PAYLOAD:
            chunk = os.read(descriptor, 4096)
            if not chunk:
                break
            response += chunk
    finally:
        os.close(descriptor)
    if len(response) > 24 + EXPORT_MAX_PAYLOAD:
        raise RuntimeError("state oversized")
    return response, export_binding(response)

def expect_fixed(response, tag, binding=None):
    if len(response) != EXPORT_HEADER_BYTES or response[1] != tag:
        raise RuntimeError("fixed export response mismatch")
    if response[2] != 0 or response[3] != 0 or response[16:18] != bytes(2):
        raise RuntimeError("fixed export response payload mismatch")
    instance = int.from_bytes(response[4:12], "big")
    window = int.from_bytes(response[12:16], "big")
    if binding is None:
        if instance != 0 or window != 0:
            raise RuntimeError("unexpected export binding")
    elif (instance, window) != binding:
        raise RuntimeError("export binding mismatch")

def write_all_descriptor(descriptor, value):
    view = memoryview(value)
    while view:
        written = os.write(descriptor, view)
        if written <= 0:
            raise RuntimeError("state write failed")
        view = view[written:]

def strict_object(pairs):
    output = {{}}
    for key, value in pairs:
        if key in output:
            raise RuntimeError("duplicate broker json key")
        output[key] = value
    return output

def broker_response(expected):
    stream.sendall(len(BROKER_REQUEST).to_bytes(4, "big") + BROKER_REQUEST)
    size = int.from_bytes(receive_exact(stream, 4), "big")
    if size == 0 or size > 65536:
        raise RuntimeError("broker response size invalid")
    payload = receive_exact(stream, size)
    expect_closed(stream, "broker response has trailing data")
    try:
        envelope = json.loads(payload.decode("utf-8"), object_pairs_hook=strict_object)
    except (UnicodeDecodeError, ValueError) as error:
        raise RuntimeError("broker response json invalid") from error
    if set(envelope) != {{"schemaVersion", "requestId", "response"}}:
        raise RuntimeError("broker envelope fields invalid")
    if envelope["schemaVersion"] != 1 or envelope["requestId"] != 71:
        raise RuntimeError("broker envelope binding invalid")
    response = envelope["response"]
    if type(response) is not dict:
        raise RuntimeError("broker response object invalid")
    if expected != "snapshot":
        if response != {{"type": "error", "code": expected}}:
            raise RuntimeError("broker error response mismatch")
        return None
    if set(response) != {{"type", "snapshot"}} or response["type"] != "diagnostics":
        raise RuntimeError("broker diagnostic response invalid")
    snapshot = response["snapshot"]
    if type(snapshot) is not dict or set(snapshot) != {{
        "instanceId", "windowId", "omittedLifecycleEvents", "recordCount", "bundle"
    }}:
        raise RuntimeError("broker snapshot fields invalid")
    instance = snapshot["instanceId"]
    window = snapshot["windowId"]
    omitted = snapshot["omittedLifecycleEvents"]
    count = snapshot["recordCount"]
    text = snapshot["bundle"]
    if (type(instance) is not int or not 0 < instance < (1 << 64)
            or type(window) is not int or not 0 < window < (1 << 32)
            or type(omitted) is not int or not 0 <= omitted <= 255
            or type(count) is not int or not 0 < count <= 32
            or type(text) is not str):
        raise RuntimeError("broker snapshot values invalid")
    try:
        bundle = text.encode("ascii")
    except UnicodeEncodeError as error:
        raise RuntimeError("broker bundle is not ascii") from error
    if len(bundle) == 0 or len(bundle) > EXPORT_MAX_PAYLOAD or not bundle.endswith(b"\\n"):
        raise RuntimeError("broker bundle length invalid")
    lines = text.splitlines()
    if lines[0] != "schema=1 record_count=" + str(count) or len(lines) != count + 1:
        raise RuntimeError("broker bundle header invalid")
    if any(not line.startswith("offset_ms=") for line in lines[1:]):
        raise RuntimeError("broker bundle record invalid")
    header = bytearray(EXPORT_HEADER_BYTES)
    header[0] = 1
    header[1] = 2
    header[2] = omitted
    header[3] = count
    header[4:12] = instance.to_bytes(8, "big")
    header[12:16] = window.to_bytes(4, "big")
    header[16:18] = len(bundle).to_bytes(2, "big")
    return bytes(header) + bundle

def store_owned(path, value):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        write_all_descriptor(descriptor, value)
    finally:
        os.close(descriptor)

def load_owned(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        metadata = os.fstat(descriptor)
        if (not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1
                or metadata.st_uid != os.geteuid()
                or stat.S_IMODE(metadata.st_mode) != 0o600
                or metadata.st_size > EXPORT_HEADER_BYTES + EXPORT_MAX_PAYLOAD):
            raise RuntimeError("owned state metadata invalid")
        output = b""
        while len(output) <= EXPORT_HEADER_BYTES + EXPORT_MAX_PAYLOAD:
            chunk = os.read(descriptor, 4096)
            if not chunk:
                break
            output += chunk
    finally:
        os.close(descriptor)
    if len(output) > EXPORT_HEADER_BYTES + EXPORT_MAX_PAYLOAD:
        raise RuntimeError("owned state oversized")
    return output

mode = sys.argv[1] if len(sys.argv) > 1 else ""
stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
stream.settimeout(8)
if mode.startswith("export-"):
    target = EXPORT_SOCKET
elif mode.startswith("broker-"):
    target = BROKER_SOCKET
else:
    target = SOCKET
stream.connect(target)
if mode == "accepted":
    stream.sendall(READY)
    if receive_exact(stream, 4) != ACCEPTED:
        raise RuntimeError("response mismatch")
elif mode == "accepted-fast":
    started = time.monotonic()
    stream.sendall(READY)
    if receive_exact(stream, 4) != ACCEPTED or time.monotonic() - started >= 1.0:
        raise RuntimeError("producer response delayed")
elif mode == "fill":
    for _ in range(20):
        stream.sendall(READY)
        if receive_exact(stream, 4) != ACCEPTED:
            raise RuntimeError("fill response mismatch")
elif mode == "control-disabled":
    stream.sendall(CONTROL_DISABLED)
    if receive_exact(stream, 4) != ACCEPTED:
        raise RuntimeError("response mismatch")
elif mode == "rejected":
    expect_closed(stream, "rejected peer received data")
elif mode == "malformed":
    stream.sendall(bytes(8))
    if stream.recv(1) != b"":
        raise RuntimeError("malformed frame received data")
elif mode == "partial":
    started = time.monotonic()
    stream.sendall(READY[:1])
    if stream.recv(1) != b"":
        raise RuntimeError("partial frame received data")
    elapsed = time.monotonic() - started
    if elapsed < 4.0 or elapsed > 8.0:
        raise RuntimeError("partial timeout out of bounds")
elif mode == "hold":
    if len(sys.argv) != 3 or not sys.argv[2].startswith(STATE_PREFIX):
        raise RuntimeError("ready path invalid")
    stream.sendall(READY)
    if receive_exact(stream, 4) != ACCEPTED:
        raise RuntimeError("response mismatch")
    descriptor = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    expect_closed(stream, "hold received trailing data")
elif mode == "export-read":
    stream.sendall(EXPORT_READ)
    response = export_response(stream)
    export_binding(response)
    descriptor = os.open(
        SEALED_STATE,
        os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW,
        0o600,
    )
    try:
        write_all_descriptor(descriptor, response)
    finally:
        os.close(descriptor)
elif mode == "export-repeat":
    expected, _ = load_state()
    stream.sendall(EXPORT_READ)
    if export_response(stream) != expected:
        raise RuntimeError("sealed snapshot changed")
elif mode == "export-ack-mismatch":
    _, (instance, window) = load_state()
    wrong_instance = instance + 1 if instance < (1 << 64) - 1 else instance - 1
    stream.sendall(export_ack(wrong_instance, window))
    expect_fixed(export_response(stream), 4, (wrong_instance, window))
elif mode == "export-ack":
    _, binding = load_state()
    stream.sendall(export_ack(*binding))
    expect_fixed(export_response(stream), 3, binding)
elif mode == "export-ack-empty":
    _, binding = load_state()
    stream.sendall(export_ack(*binding))
    expect_fixed(export_response(stream), 1)
elif mode == "export-no-sealed":
    stream.sendall(EXPORT_READ)
    expect_fixed(export_response(stream), 1)
elif mode == "export-rejected":
    expect_closed(stream, "unauthorized export peer received data")
elif mode == "export-malformed":
    stream.sendall(bytes(16))
    expect_closed(stream, "malformed export request received data")
elif mode == "export-partial":
    started = time.monotonic()
    stream.sendall(EXPORT_READ[:1])
    expect_closed(stream, "partial export request received data")
    elapsed = time.monotonic() - started
    if elapsed < 1.5 or elapsed > 4.0:
        raise RuntimeError("export partial timeout out of bounds")
elif mode == "export-hold":
    if len(sys.argv) != 3 or not sys.argv[2].startswith(STATE_PREFIX):
        raise RuntimeError("export ready path invalid")
    stream.sendall(EXPORT_READ[:1])
    descriptor = os.open(sys.argv[2], os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    os.close(descriptor)
    expect_closed(stream, "export hold received data")
elif mode == "export-over-capacity":
    started = time.monotonic()
    expect_closed(stream, "over-capacity export peer received data")
    if time.monotonic() - started >= 1.0:
        raise RuntimeError("over-capacity rejection delayed")
elif mode == "broker-read":
    store_owned(BROKER_STATE, broker_response("snapshot"))
elif mode == "broker-repeat":
    if broker_response("snapshot") != load_owned(BROKER_STATE):
        raise RuntimeError("broker snapshot changed")
elif mode == "broker-unavailable":
    broker_response("backend_unavailable")
elif mode == "broker-rejected":
    broker_response("unauthorized")
else:
    raise RuntimeError("mode invalid")
stream.close()
'''


def build_preflight_script(paths: Paths, required_bytes: int) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "export_socket_path": paths.export_socket,
            "broker_runtime_directory": paths.broker_runtime_directory,
            "broker_socket_path": paths.broker_socket,
            "broker_unit": paths.broker_unit,
            "watchdog": paths.watchdog,
            "required_bytes": required_bytes,
            "collector_account": COLLECTOR_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "all_accounts": " ".join(ALL_ACCOUNTS),
            "account_files": " ".join(f"/etc/{name}" for name in ACCOUNT_DATABASE_FILES),
            "run_id": paths.run_id,
            "unit_name": UNIT_NAME,
        }
    )
    body = r'''
set -eu
export LC_ALL=C
emit() { printf '%s=%s\n' "$1" "$2"; }
[ "$(id -u)" = 0 ]
[ "$(uname -m)" = aarch64 ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
tmp_options=$(findmnt -n -o OPTIONS /tmp 2>/dev/null || findmnt -n -o OPTIONS /)
case ",$tmp_options," in *,noexec,*) exit 1 ;; *) : ;; esac
available=$(df -B1 --output=avail /tmp | tail -n 1 | tr -d ' ')
case "$available" in ''|*[!0-9]*) exit 1 ;; esac
[ "$available" -ge "$required_bytes" ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
[ ! -e "$runtime_directory" ]
[ ! -e "$socket_path" ]
[ ! -e "$export_socket_path" ]
[ ! -e "$broker_runtime_directory" ]
[ ! -e "$broker_socket_path" ]
[ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" != active ]
if systemctl cat "$broker_unit" >/dev/null 2>&1; then exit 1; fi
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
if systemctl cat "$unit_name" >/dev/null 2>&1; then exit 1; fi
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $all_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$account" >/dev/null 2>&1; then exit 1; fi
done
for database in $account_files; do
  [ -f "$database" ]
  [ ! -L "$database" ]
  [ "$(stat -c '%u:%h' "$database")" = 0:1 ]
  mode=$(stat -c '%a' "$database")
  case "$database:$mode" in
    /etc/passwd:644|/etc/group:644|/etc/shadow:600|/etc/shadow:640|/etc/gshadow:600|/etc/gshadow:640) ;;
    *) exit 1 ;;
  esac
  name=${database##*/}
  [ ! -e "/etc/.$name.hardened-blikvm-$run_id" ]
done
if [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_before=present
else
  binary_parent_before=absent
fi
version=$(systemctl --version | head -n 1 | awk '{print $2}')
case "$version" in ''|*[!0-9]*) exit 1 ;; esac
[ "$version" -ge 249 ]
for tool in awk base64 cat chmod cmp cp cut df find findmnt getent grep head id install kill mkdir mv python3 readlink rm rmdir sed setpriv sha256sum sleep stat sync systemctl systemd-analyze systemd-run systemd-sysusers tail tr uname wc; do
  command -v "$tool" >/dev/null 2>&1 || exit 1
done
emit account_absent_before yes
emit account_database_ready yes
emit architecture aarch64
emit binary_absent_before yes
emit binary_parent_before "$binary_parent_before"
emit broker_runtime_absent_before yes
emit broker_socket_absent_before yes
emit broker_unit_absent_before yes
emit identities_absent_before yes
emit kvmd_active_before active
emit kvmd_nginx_active_before active
emit rootfs_mode_before "$rootfs_mode"
emit runtime_absent_before yes
emit export_socket_absent_before yes
emit stage_absent_before yes
emit systemd_ready yes
emit tools_ready yes
emit unit_absent_before yes
emit watchdog_absent_before yes
'''.strip()
    return f"{values}\n{body}\n"


def build_first_upload_script(paths: Paths) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": paths.binary,
        }
    )
    body = r'''
set -eu
umask 077
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
[ ! -e "$stage_root" ]
mkdir -m 755 "$stage_root"
printf '%s\n' "$marker_value" > "$marker"
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod 755 "$destination"
'''.strip()
    return f"{values}\n{body}\n"


def build_upload_script(paths: Paths, destination: str, mode: str) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "destination": destination,
            "mode": mode,
        }
    )
    body = r'''
set -eu
[ -d "$stage_root" ]
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ ! -e "$destination" ]
cat > "$destination.part"
[ -s "$destination.part" ]
mv "$destination.part" "$destination"
chmod "$mode" "$destination"
'''.strip()
    return f"{values}\n{body}\n"


def build_rollback_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "state_file": paths.state,
            "unit_name": UNIT_NAME,
            "unit_target": paths.unit_target,
            "unit_sha": assets.unit.sha256,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "broker_binary": paths.broker_binary,
            "broker_binary_sha": assets.broker_binary.sha256,
            "broker_binary_size": assets.broker_binary.size_bytes,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "export_socket_path": paths.export_socket,
            "broker_runtime_directory": paths.broker_runtime_directory,
            "broker_socket_path": paths.broker_socket,
            "broker_unit": paths.broker_unit,
            "collector_account": COLLECTOR_ACCOUNT,
            "all_accounts": " ".join(ALL_ACCOUNTS),
            "shared_group": SHARED_GROUP,
            "account_files": " ".join(f"/etc/{name}" for name in ACCOUNT_DATABASE_FILES),
            "account_backup": paths.account_backup,
            "run_id": paths.run_id,
            "watchdog": paths.watchdog,
        }
    )
    body = r'''
set -eu
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ -f "$state_file" ]
parent_before=$(awk -F= '$1 == "parent_before" {print $2}' "$state_file")
case "$parent_before" in present|absent) ;; *) exit 1 ;; esac
systemctl stop "$broker_unit" 2>/dev/null || true
systemctl stop "$unit_name" 2>/dev/null || true
count=0
while [ "$count" -lt 100 ] && getent passwd "$collector_account" >/dev/null 2>&1; do
  sleep 0.1
  count=$((count + 1))
done
if [ -e "$runtime_directory" ]; then
  [ -d "$runtime_directory" ]
  [ ! -L "$runtime_directory" ]
  unexpected=$(find "$runtime_directory" -mindepth 1 -maxdepth 1 ! -name collector.sock ! -name export.sock -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$socket_path" ]; then
    [ -S "$socket_path" ]
    [ "$(stat -c '%a' "$socket_path")" = 660 ]
    rm -f -- "$socket_path"
  fi
  if [ -e "$export_socket_path" ]; then
    [ -S "$export_socket_path" ]
    [ "$(stat -c '%a' "$export_socket_path")" = 660 ]
    rm -f -- "$export_socket_path"
  fi
  rmdir "$runtime_directory"
fi
if [ -e "$broker_runtime_directory" ]; then
  [ -d "$broker_runtime_directory" ]
  [ ! -L "$broker_runtime_directory" ]
  unexpected=$(find "$broker_runtime_directory" -mindepth 1 -maxdepth 1 ! -name platform-broker.sock -print -quit)
  [ -z "$unexpected" ]
  if [ -e "$broker_socket_path" ]; then
    [ -S "$broker_socket_path" ]
    [ "$(stat -c '%a' "$broker_socket_path")" = 660 ]
    rm -f -- "$broker_socket_path"
  fi
  rmdir "$broker_runtime_directory"
fi
for database in $account_files; do
  name=${database##*/}
  before=$(awk -F= -v key="before_$name" '$1 == key {print $2}' "$state_file")
  modified=$(awk -F= -v key="modified_$name" '$1 == key {print $2}' "$state_file")
  case "$before" in *[!0-9a-f]*) exit 1 ;; esac
  [ "${#before}" = 64 ]
  current=$(sha256sum "$database" | cut -d ' ' -f 1)
  if [ -z "$modified" ]; then
    [ "$current" = "$before" ]
  elif [ "${#modified}" != 64 ]; then
    exit 1
  elif [ "$current" = "$modified" ]; then
    restore_tmp="/etc/.$name.hardened-blikvm-$run_id"
    [ ! -e "$restore_tmp" ]
    cp -a -- "$account_backup/$name" "$restore_tmp"
    sync "$restore_tmp"
    mv -f -- "$restore_tmp" "$database"
    sync "$database"
  elif [ "$current" != "$before" ]; then
    exit 3
  fi
  [ "$(sha256sum "$database" | cut -d ' ' -f 1)" = "$before" ]
  cmp -s "$account_backup/$name" "$database"
done
if [ -e "$unit_target" ]; then
  [ -f "$unit_target" ]
  [ ! -L "$unit_target" ]
  [ "$(sha256sum "$unit_target" | cut -d ' ' -f 1)" = "$unit_sha" ]
  rm -f -- "$unit_target"
fi
systemctl daemon-reload
systemctl reset-failed "$unit_name" 2>/dev/null || true
systemctl reset-failed "$broker_unit" 2>/dev/null || true
if [ -e "$binary_part" ]; then
  [ -f "$binary_part" ]
  [ ! -L "$binary_part" ]
  [ "$(stat -c '%a:%u:%g:%h' "$binary_part")" = 755:0:0:1 ]
  [ "$(stat -c %s "$binary_part")" -le "$binary_size" ]
  rm -f -- "$binary_part"
fi
if [ -e "$binary_target" ]; then
  [ -f "$binary_target" ]
  [ ! -L "$binary_target" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$binary_target")" = "755:0:0:1:$binary_size" ]
  [ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
  rm -f -- "$binary_target"
fi
if [ -e "$broker_binary" ]; then
  [ -f "$broker_binary" ]
  [ ! -L "$broker_binary" ]
  [ "$(stat -c '%a:%u:%g:%h:%s' "$broker_binary")" = "755:0:0:1:$broker_binary_size" ]
  [ "$(sha256sum "$broker_binary" | cut -d ' ' -f 1)" = "$broker_binary_sha" ]
fi
if [ "$parent_before" = absent ] && [ -e "$binary_parent" ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  rmdir "$binary_parent"
fi
[ ! -e "$runtime_directory" ]
[ ! -e "$export_socket_path" ]
[ ! -e "$broker_runtime_directory" ]
[ ! -e "$broker_socket_path" ]
[ ! -e "$binary_target" ]
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $all_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$account" >/dev/null 2>&1; then exit 1; fi
done
systemctl stop "$watchdog.timer" 2>/dev/null || true
if [ -z "${INVOCATION_ID:-}" ]; then systemctl stop "$watchdog.service" 2>/dev/null || true; fi
rm -rf -- "$stage_root"
'''.strip()
    return f"{values}\n{body}\n"


def build_run_script(paths: Paths, assets: Assets) -> str:
    client = build_client(paths)
    rollback = build_rollback_script(paths, assets)
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "binary": paths.binary,
            "binary_sha": assets.binary.sha256,
            "binary_size": assets.binary.size_bytes,
            "broker_binary": paths.broker_binary,
            "broker_binary_sha": assets.broker_binary.sha256,
            "broker_binary_size": assets.broker_binary.size_bytes,
            "unit_source": paths.unit,
            "unit_sha": assets.unit.sha256,
            "base_sysusers_source": paths.base_sysusers,
            "base_sysusers_sha": assets.base_sysusers.sha256,
            "sysusers_source": paths.sysusers,
            "sysusers_sha": assets.sysusers.sha256,
            "client": paths.client,
            "client_sha": hash_text(client),
            "client_b64": encode_text(client),
            "rollback_script": paths.rollback,
            "rollback_b64": encode_text(rollback),
            "rollback_state": paths.state,
            "unit_name": UNIT_NAME,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "export_socket_path": paths.export_socket,
            "broker_runtime_directory": paths.broker_runtime_directory,
            "broker_socket_path": paths.broker_socket,
            "broker_credentials": paths.broker_credentials,
            "broker_unit": paths.broker_unit,
            "client_state": paths.client_state,
            "account_backup": paths.account_backup,
            "account_files": " ".join(f"/etc/{name}" for name in ACCOUNT_DATABASE_FILES),
            "watchdog": paths.watchdog,
            "collector_account": COLLECTOR_ACCOUNT,
            "shared_group": SHARED_GROUP,
            "producer_accounts": " ".join(PRODUCER_ACCOUNTS),
            "all_accounts": " ".join(ALL_ACCOUNTS),
        }
    )
    body = r'''
set -eu
export LC_ALL=C
umask 077
phase=validate-assets
run_succeeded=no
emit() { printf '%s=%s\n' "$1" "$2"; }
on_exit() {
  exit_code=$?
  trap - EXIT HUP INT TERM
  if [ "$run_succeeded" != yes ]; then emit failure_phase "$phase"; fi
  exit "$exit_code"
}
trap on_exit EXIT
trap 'exit 1' HUP INT TERM
[ -f "$marker" ]
[ "$(cat "$marker")" = "$marker_value" ]
[ "$(stat -c %s "$binary")" = "$binary_size" ]
[ "$(sha256sum "$binary" | cut -d ' ' -f 1)" = "$binary_sha" ]
[ "$(stat -c '%a:%u:%g:%h:%s' "$broker_binary")" = "755:0:0:1:$broker_binary_size" ]
[ "$(sha256sum "$broker_binary" | cut -d ' ' -f 1)" = "$broker_binary_sha" ]
[ "$(sha256sum "$unit_source" | cut -d ' ' -f 1)" = "$unit_sha" ]
[ "$(sha256sum "$base_sysusers_source" | cut -d ' ' -f 1)" = "$base_sysusers_sha" ]
[ "$(sha256sum "$sysusers_source" | cut -d ' ' -f 1)" = "$sysusers_sha" ]
kvmd_pid_before=$(systemctl show -p MainPID --value kvmd.service)
nginx_pid_before=$(systemctl show -p MainPID --value kvmd-nginx.service)
for pid in "$kvmd_pid_before" "$nginx_pid_before"; do
  case "$pid" in ''|0|*[!0-9]*) exit 1 ;; esac
done
printf '%s' "$client_b64" | base64 -d > "$client"
chmod 755 "$client"
[ "$(sha256sum "$client" | cut -d ' ' -f 1)" = "$client_sha" ]
printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
chmod 700 "$rollback_script"
phase=verify-sysusers
synthetic_root="$stage_root/sysusers-root"
install -d -m 755 "$synthetic_root/etc" "$synthetic_root/usr/lib/sysusers.d"
install -m 0644 "$base_sysusers_source" "$synthetic_root/usr/lib/sysusers.d/40-base.conf"
install -m 0644 "$sysusers_source" "$synthetic_root/usr/lib/sysusers.d/50-diagnostics.conf"
systemd-sysusers --root="$synthetic_root" >/dev/null 2>&1
for account in $all_accounts; do
  awk -F: -v account="$account" '$1 == account && $3 != 0 && $6 == "/nonexistent" && $7 == "/usr/sbin/nologin" {found=1} END {exit !found}' "$synthetic_root/etc/passwd"
done
awk -F: -v group="$shared_group" -v account="$collector_account" '$1 == group && ("," $4 ",") ~ ("," account ",") {found=1} END {exit !found}' "$synthetic_root/etc/group"
if [ -e "$binary_parent" ]; then parent_before=present; else parent_before=absent; fi
printf 'parent_before=%s\n' "$parent_before" > "$rollback_state"
install -d -m 700 "$account_backup"
for database in $account_files; do
  name=${database##*/}
  cp -a -- "$database" "$account_backup/$name"
  before=$(sha256sum "$database" | cut -d ' ' -f 1)
  printf 'before_%s=%s\n' "$name" "$before" >> "$rollback_state"
done
chmod 600 "$rollback_state"
phase=arm-watchdog
systemd-run --quiet --unit="$watchdog" --on-active=10min --timer-property=AccuracySec=1s /bin/sh "$rollback_script"
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" = active ]
phase=create-static-identities
systemd-sysusers "$base_sysusers_source" "$sysusers_source" >/dev/null 2>&1
for database in $account_files; do
  name=${database##*/}
  modified=$(sha256sum "$database" | cut -d ' ' -f 1)
  printf 'modified_%s=%s\n' "$name" "$modified" >> "$rollback_state"
done
sync "$rollback_state"
phase=verify-static-identities
shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
case "$shared_gid" in ''|0|*[!0-9]*) exit 1 ;; esac
seen=:
for account in $all_accounts; do
  uid=$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')
  case "$uid" in ''|0|*[!0-9]*) exit 1 ;; esac
  case "$seen" in *":$uid:"*) exit 1 ;; esac
  seen="$seen$uid:"
done
phase=start-diagnostic-broker-without-collector
shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
broker_uid=$(getent passwd hardened-blikvm-broker | awk -F: 'NR == 1 {print $3}')
server_uid=$(getent passwd hardened-blikvm-server | awk -F: 'NR == 1 {print $3}')
for value in "$shared_gid" "$broker_uid" "$server_uid"; do
  case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac
done
install -d -m 0700 -o "$broker_uid" -g "$shared_gid" "$broker_credentials"
printf '%s\n' unused-local-gate-user > "$broker_credentials/kvmd-user"
printf '%s\n' unused-local-gate-value > "$broker_credentials/kvmd-password"
chown "$broker_uid:$shared_gid" "$broker_credentials/kvmd-user" "$broker_credentials/kvmd-password"
chmod 0400 "$broker_credentials/kvmd-user" "$broker_credentials/kvmd-password"
systemd-run --quiet --unit="$broker_unit" --service-type=simple \
  --property=User=hardened-blikvm-broker \
  --property=Group=hardened-blikvm \
  --property=UMask=0077 \
  --property=RuntimeDirectory=hardened-blikvm \
  --property=RuntimeDirectoryMode=0750 \
  --property=RuntimeDirectoryPreserve=no \
  --property=NoNewPrivileges=yes \
  --property=PrivateDevices=yes \
  --property=PrivateNetwork=yes \
  --property=ProtectSystem=strict \
  --property=ProtectHome=yes \
  --property=RestrictAddressFamilies=AF_UNIX \
  "--property=CapabilityBoundingSet=" \
  "--property=AmbientCapabilities=" \
  --property=DevicePolicy=closed \
  --property=IPAddressDeny=any \
  --property=ReadWritePaths=/run/hardened-blikvm \
  --setenv=CREDENTIALS_DIRECTORY="$broker_credentials" \
  "$broker_binary"
count=0
while [ "$count" -lt 100 ]; do
  if [ -S "$broker_socket_path" ]; then break; fi
  [ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S "$broker_socket_path" ]
broker_pid=$(systemctl show -p MainPID --value "$broker_unit")
case "$broker_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
[ "$(systemctl show -p User --value "$broker_unit")" = hardened-blikvm-broker ]
[ "$(systemctl show -p Group --value "$broker_unit")" = hardened-blikvm ]
[ "$(systemctl show -p PrivateNetwork --value "$broker_unit")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$broker_unit")" = closed ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$broker_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$broker_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$broker_pid/status")" = 1 ]
[ "$(readlink "/proc/$broker_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
[ "$(awk '/^Uid:/ {print $2}' "/proc/$broker_pid/status")" = "$broker_uid" ]
[ "$(readlink -f "/proc/$broker_pid/exe")" = "$broker_binary" ]
[ "$(stat -c '%a:%u:%g' "$broker_runtime_directory")" = "750:$broker_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' "$broker_socket_path")" = "660:$broker_uid:$shared_gid" ]
setpriv --reuid="$server_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" broker-unavailable
phase=install-runtime
if [ "$parent_before" = absent ]; then mkdir -m 755 "$binary_parent"; fi
install -m 0755 "$binary" "$binary_part"
[ "$(sha256sum "$binary_part" | cut -d ' ' -f 1)" = "$binary_sha" ]
mv "$binary_part" "$binary_target"
[ "$(sha256sum "$binary_target" | cut -d ' ' -f 1)" = "$binary_sha" ]
install -m 0644 "$unit_source" "$unit_target"
[ "$(sha256sum "$unit_target" | cut -d ' ' -f 1)" = "$unit_sha" ]
systemctl daemon-reload
systemd-analyze verify "$unit_name" >/dev/null 2>&1
phase=start-collector
systemctl start "$unit_name"
count=0
while [ "$count" -lt 100 ]; do
  if [ -S "$socket_path" ] && [ -S "$export_socket_path" ]; then break; fi
  [ "$(systemctl is-active "$unit_name" 2>/dev/null || true)" = active ] || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -S "$socket_path" ]
[ -S "$export_socket_path" ]
collector_pid=$(systemctl show -p MainPID --value "$unit_name")
case "$collector_pid" in ''|0|*[!0-9]*) exit 1 ;; esac
[ "$(systemctl show -p User --value "$unit_name")" = "$collector_account" ]
[ "$(systemctl show -p Group --value "$unit_name")" = "$shared_group" ]
[ "$(systemctl show -p DynamicUser --value "$unit_name")" = yes ]
[ "$(systemctl show -p PrivateNetwork --value "$unit_name")" = yes ]
[ "$(systemctl show -p DevicePolicy --value "$unit_name")" = closed ]
[ "$(awk '/^CapEff:/ {print $2}' "/proc/$collector_pid/status")" = 0000000000000000 ]
[ "$(awk '/^CapBnd:/ {print $2}' "/proc/$collector_pid/status")" = 0000000000000000 ]
[ "$(awk '/^NoNewPrivs:/ {print $2}' "/proc/$collector_pid/status")" = 1 ]
[ "$(readlink "/proc/$collector_pid/ns/net")" != "$(readlink /proc/1/ns/net)" ]
collector_uid=$(getent passwd "$collector_account" | awk -F: 'NR == 1 {print $3}')
shared_gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
for value in "$collector_uid" "$shared_gid"; do
  case "$value" in ''|0|*[!0-9]*) exit 1 ;; esac
done
[ "$(awk '/^Uid:/ {print $2}' "/proc/$collector_pid/status")" = "$collector_uid" ]
[ "$(stat -c '%a:%u:%g' "$runtime_directory")" = "750:$collector_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' "$socket_path")" = "660:$collector_uid:$shared_gid" ]
[ "$(stat -c '%a:%u:%g' "$export_socket_path")" = "660:$collector_uid:$shared_gid" ]
phase=accepted-sources
install -d -m 0770 -o root -g "$shared_group" "$client_state"
run_as() {
  account=$1
  shift
  uid=$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')
  gid=$(getent group "$shared_group" | awk -F: 'NR == 1 {print $3}')
  setpriv --reuid="$uid" --regid="$gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" "$@"
}
accepted_count=0
for account in $producer_accounts; do
  run_as "$account" accepted
  accepted_count=$((accepted_count + 1))
done
[ "$accepted_count" = 5 ]
sleep 0.2
run_as hardened-blikvm-control control-disabled
phase=export-connection-pressure
broker_uid=$(getent passwd hardened-blikvm-broker | awk -F: 'NR == 1 {print $3}')
export_ready_one="$client_state/export-one.ready"
export_ready_two="$client_state/export-two.ready"
setpriv --reuid="$broker_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" export-hold "$export_ready_one" &
export_holder_one=$!
setpriv --reuid="$broker_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" export-hold "$export_ready_two" &
export_holder_two=$!
count=0
while [ "$count" -lt 40 ] && { [ ! -f "$export_ready_one" ] || [ ! -f "$export_ready_two" ]; }; do
  kill -0 "$export_holder_one" 2>/dev/null || exit 1
  kill -0 "$export_holder_two" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -f "$export_ready_one" ]
[ -f "$export_ready_two" ]
sleep 0.2
run_as hardened-blikvm-broker export-over-capacity
run_as hardened-blikvm-control accepted-fast
wait "$export_holder_one"
wait "$export_holder_two"
rm -f -- "$export_ready_one" "$export_ready_two"
phase=create-sealed-window
run_as hardened-blikvm-broker fill
sleep 0.2
run_as hardened-blikvm-broker export-read
run_as hardened-blikvm-broker export-repeat
phase=broker-read-only-route
run_as hardened-blikvm-server broker-read
cmp -s "$client_state/sealed.bin" "$client_state/broker-sealed.bin"
run_as hardened-blikvm-server broker-repeat
cmp -s "$client_state/sealed.bin" "$client_state/broker-sealed.bin"
python3 "$client" broker-rejected
run_as hardened-blikvm-broker broker-rejected
run_as hardened-blikvm-control broker-rejected
run_as hardened-blikvm-broker export-repeat
phase=stop-diagnostic-broker
systemctl stop "$broker_unit"
[ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" != active ]
[ ! -e "$broker_socket_path" ]
[ ! -e "$broker_runtime_directory" ]
phase=export-rejected-peers
python3 "$client" export-rejected
run_as hardened-blikvm-server export-rejected
run_as hardened-blikvm-broker export-malformed
run_as hardened-blikvm-broker export-partial
phase=export-acknowledgement
run_as hardened-blikvm-broker export-ack-mismatch
run_as hardened-blikvm-broker export-repeat
run_as hardened-blikvm-broker export-ack
run_as hardened-blikvm-broker export-no-sealed
run_as hardened-blikvm-broker export-ack-empty
phase=rejected-peers
python3 "$client" rejected
unknown_uid=$(getent passwd nobody | awk -F: 'NR == 1 {print $3}')
case "$unknown_uid" in ''|0|*[!0-9]*) exit 1 ;; esac
for account in $producer_accounts; do
  [ "$unknown_uid" != "$(getent passwd "$account" | awk -F: 'NR == 1 {print $3}')" ]
done
[ "$unknown_uid" != "$collector_uid" ]
setpriv --reuid="$unknown_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" rejected
phase=duplicate-peer
server_uid=$(getent passwd hardened-blikvm-server | awk -F: 'NR == 1 {print $3}')
duplicate_ready="$client_state/duplicate.ready"
setpriv --reuid="$server_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" hold "$duplicate_ready" &
holder_pid=$!
count=0
while [ "$count" -lt 80 ] && [ ! -f "$duplicate_ready" ]; do
  kill -0 "$holder_pid" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -f "$duplicate_ready" ]
run_as hardened-blikvm-server rejected
kill "$holder_pid"
wait "$holder_pid" 2>/dev/null || true
rm -f -- "$duplicate_ready"
sleep 0.2
phase=partial-frame
run_as hardened-blikvm-server partial
sleep 0.2
phase=malformed-frame
run_as hardened-blikvm-server malformed
[ "$(systemctl is-active "$unit_name" 2>/dev/null || true)" = active ]
phase=graceful-shutdown
update_uid=$(getent passwd hardened-blikvm-update | awk -F: 'NR == 1 {print $3}')
shutdown_ready="$client_state/shutdown.ready"
export_shutdown_ready="$client_state/export-shutdown.ready"
setpriv --reuid="$update_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" hold "$shutdown_ready" &
shutdown_client_pid=$!
setpriv --reuid="$broker_uid" --regid="$shared_gid" --clear-groups --bounding-set=-all --no-new-privs python3 "$client" export-hold "$export_shutdown_ready" &
export_shutdown_client_pid=$!
count=0
while [ "$count" -lt 80 ] && { [ ! -f "$shutdown_ready" ] || [ ! -f "$export_shutdown_ready" ]; }; do
  kill -0 "$shutdown_client_pid" 2>/dev/null || exit 1
  kill -0 "$export_shutdown_client_pid" 2>/dev/null || exit 1
  sleep 0.1
  count=$((count + 1))
done
[ -f "$shutdown_ready" ]
[ -f "$export_shutdown_ready" ]
systemctl stop "$unit_name"
wait "$shutdown_client_pid"
wait "$export_shutdown_client_pid"
[ ! -e "$runtime_directory" ]
[ "$(systemctl show -p MainPID --value kvmd.service)" = "$kvmd_pid_before" ]
[ "$(systemctl show -p MainPID --value kvmd-nginx.service)" = "$nginx_pid_before" ]
phase=rollback
/bin/sh "$rollback_script"
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$runtime_directory" ]
[ ! -e "$broker_runtime_directory" ]
[ ! -e "$broker_socket_path" ]
run_succeeded=yes
emit binary_hash_verified yes
emit broker_binary_hash_verified yes
emit broker_caps none
emit broker_exact_reader server
emit broker_identity exact-platform-broker
emit broker_matches_export yes
emit broker_network_namespace private
emit broker_no_ack yes
emit broker_no_new_privs yes
emit broker_repeatable yes
emit broker_runtime_removed yes
emit broker_socket_mode 660
emit broker_socket_owner static-shared
emit broker_started_without_collector yes
emit broker_unauthorized_rejected root-and-wrong-role
emit broker_unavailable_without_collector typed
emit broker_unit active-then-stopped
emit collector_caps none
emit collector_identity static-sysusers
emit collector_network_namespace private
emit collector_no_new_privs yes
emit collector_unit active-then-stopped
emit control_rejection_accepted yes
emit duplicate_rejected yes
emit export_acknowledged yes
emit export_exact_consumer platform-broker
emit export_malformed_rejected yes
emit export_mismatch_preserved yes
emit export_no_sealed_after_ack yes
emit export_over_capacity_rejected yes
emit export_partial_timeout bounded
emit export_repeatable yes
emit export_socket_mode 660
emit export_socket_owner static-shared
emit export_unauthorized_rejected root-and-wrong-role
emit five_sources_accepted 5
emit graceful_export_shutdown yes
emit graceful_shutdown yes
emit kvmd_active_after_run active
emit kvmd_nginx_active_after_run active
emit malformed_rejected yes
emit partial_timeout bounded
emit producer_during_export_stall bounded
emit root_rejected yes
emit rollback_confirmed yes
emit runtime_removed yes
emit socket_mode 660
emit socket_owner static-shared
emit sysusers_synthetic yes
emit static_identities created-and-restored
emit account_database_restored yes
emit unit_hash_verified yes
emit unknown_rejected yes
emit watchdog_armed yes
'''.strip()
    return f"{values}\n{body}\n"


def build_cleanup_script(paths: Paths, assets: Assets) -> str:
    values = assignments(
        {
            "stage_root": paths.root,
            "marker": paths.marker,
            "marker_value": paths.marker_value,
            "rollback_script": paths.rollback,
            "rollback_state": paths.state,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "broker_runtime_directory": paths.broker_runtime_directory,
            "broker_socket_path": paths.broker_socket,
            "broker_unit": paths.broker_unit,
            "all_accounts": " ".join(ALL_ACCOUNTS),
            "shared_group": SHARED_GROUP,
            "rollback_b64": encode_text(build_rollback_script(paths, assets)),
        }
    )
    body = r'''
set -eu
case "$stage_root" in /tmp/hardened-blikvm-diagnostic-collector-*) ;; *) exit 1 ;; esac
if [ -e "$stage_root" ]; then
  [ -d "$stage_root" ]
  [ ! -L "$stage_root" ]
  [ -f "$marker" ]
  [ "$(cat "$marker")" = "$marker_value" ]
  if [ ! -x "$rollback_script" ]; then
    printf '%s' "$rollback_b64" | base64 -d > "$rollback_script"
    chmod 700 "$rollback_script"
  fi
  if [ ! -f "$rollback_state" ]; then
    [ ! -e "$binary_target" ]
    [ ! -e /run/systemd/system/hardened-blikvm-diagnostic-collector.service ]
    [ ! -e /run/hardened-blikvm-diagnostics ]
    [ ! -e "$broker_runtime_directory" ]
    [ ! -e "$broker_socket_path" ]
    [ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" != active ]
    if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
    for account in $all_accounts; do
      if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
      if getent group "$account" >/dev/null 2>&1; then exit 1; fi
    done
    rm -rf -- "$stage_root"
  else
    /bin/sh "$rollback_script"
  fi
fi
[ ! -e "$stage_root" ]
printf 'cleanup_restored=yes\n'
'''.strip()
    return f"{values}\n{body}\n"


def build_final_script(paths: Paths, binary_parent_before: str) -> str:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    values = assignments(
        {
            "stage_root": paths.root,
            "unit_target": paths.unit_target,
            "binary_parent": paths.binary_parent,
            "binary_target": paths.binary_target,
            "binary_part": paths.binary_part,
            "binary_parent_before": binary_parent_before,
            "runtime_directory": paths.runtime_directory,
            "socket_path": paths.socket,
            "export_socket_path": paths.export_socket,
            "broker_runtime_directory": paths.broker_runtime_directory,
            "broker_socket_path": paths.broker_socket,
            "broker_unit": paths.broker_unit,
            "watchdog": paths.watchdog,
            "collector_account": COLLECTOR_ACCOUNT,
            "all_accounts": " ".join(ALL_ACCOUNTS),
            "shared_group": SHARED_GROUP,
        }
    )
    body = r'''
set -eu
emit() { printf '%s=%s\n' "$1" "$2"; }
[ ! -e "$stage_root" ]
[ ! -e "$unit_target" ]
[ ! -e "$binary_target" ]
[ ! -e "$binary_part" ]
if [ "$binary_parent_before" = present ]; then
  [ -d "$binary_parent" ]
  [ ! -L "$binary_parent" ]
  [ "$(stat -c '%a:%u:%g' "$binary_parent")" = 755:0:0 ]
  binary_parent_final=present
else
  [ ! -e "$binary_parent" ]
  binary_parent_final=absent
fi
[ ! -e "$runtime_directory" ]
[ ! -e "$socket_path" ]
[ ! -e "$export_socket_path" ]
[ ! -e "$broker_runtime_directory" ]
[ ! -e "$broker_socket_path" ]
[ "$(systemctl is-active "$broker_unit" 2>/dev/null || true)" != active ]
if getent passwd "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$collector_account" >/dev/null 2>&1; then exit 1; fi
if getent group "$shared_group" >/dev/null 2>&1; then exit 1; fi
for account in $all_accounts; do
  if getent passwd "$account" >/dev/null 2>&1; then exit 1; fi
  if getent group "$account" >/dev/null 2>&1; then exit 1; fi
done
[ "$(systemctl is-active "$watchdog.timer" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active "$watchdog.service" 2>/dev/null || true)" != active ]
[ "$(systemctl is-active kvmd.service 2>/dev/null || true)" = active ]
[ "$(systemctl is-active kvmd-nginx.service 2>/dev/null || true)" = active ]
root_options=$(findmnt -n -o OPTIONS /)
case ",$root_options," in *,rw,*) rootfs_mode=rw ;; *) exit 1 ;; esac
emit account_absent_final yes
emit binary_absent_final yes
emit binary_parent_final "$binary_parent_final"
emit broker_runtime_absent_final yes
emit broker_socket_absent_final yes
emit broker_unit_absent_final yes
emit kvmd_active_final active
emit kvmd_nginx_active_final active
emit rootfs_mode_final "$rootfs_mode"
emit runtime_absent_final yes
emit export_socket_absent_final yes
emit stage_absent_final yes
emit unit_absent_final yes
emit watchdog_inactive_final yes
'''.strip()
    return f"{values}\n{body}\n"


def require_result(stage: str, result: subprocess.CompletedProcess[bytes]) -> bytes:
    if result.returncode != 0 or len(result.stdout) > 16 * 1024:
        raise RuntimeError(stage)
    return result.stdout


def parse_runtime_failure(raw: bytes) -> str:
    facts = native.parse_facts(raw, FAILURE_KEYS)
    phase = facts["failure_phase"]
    if SAFE_PHASE.fullmatch(phase) is None:
        raise ValueError("runtime failure phase is invalid")
    return phase


def validate_preflight(facts: dict[str, str]) -> None:
    expected = {
        "account_absent_before": "yes",
        "account_database_ready": "yes",
        "architecture": "aarch64",
        "binary_absent_before": "yes",
        "binary_parent_before": facts.get("binary_parent_before", ""),
        "broker_runtime_absent_before": "yes",
        "broker_socket_absent_before": "yes",
        "broker_unit_absent_before": "yes",
        "identities_absent_before": "yes",
        "kvmd_active_before": "active",
        "kvmd_nginx_active_before": "active",
        "rootfs_mode_before": "rw",
        "runtime_absent_before": "yes",
        "export_socket_absent_before": "yes",
        "stage_absent_before": "yes",
        "systemd_ready": "yes",
        "tools_ready": "yes",
        "unit_absent_before": "yes",
        "watchdog_absent_before": "yes",
    }
    if expected["binary_parent_before"] not in {"present", "absent"}:
        raise ValueError("binary parent state is invalid")
    if facts != dict(sorted(expected.items())):
        raise ValueError("preflight invariant failed")


def validate_run(facts: dict[str, str]) -> None:
    expected = {
        "account_database_restored": "yes",
        "binary_hash_verified": "yes",
        "broker_binary_hash_verified": "yes",
        "broker_caps": "none",
        "broker_exact_reader": "server",
        "broker_identity": "exact-platform-broker",
        "broker_matches_export": "yes",
        "broker_network_namespace": "private",
        "broker_no_ack": "yes",
        "broker_no_new_privs": "yes",
        "broker_repeatable": "yes",
        "broker_runtime_removed": "yes",
        "broker_socket_mode": "660",
        "broker_socket_owner": "static-shared",
        "broker_started_without_collector": "yes",
        "broker_unauthorized_rejected": "root-and-wrong-role",
        "broker_unavailable_without_collector": "typed",
        "broker_unit": "active-then-stopped",
        "collector_caps": "none",
        "collector_identity": "static-sysusers",
        "collector_network_namespace": "private",
        "collector_no_new_privs": "yes",
        "collector_unit": "active-then-stopped",
        "control_rejection_accepted": "yes",
        "duplicate_rejected": "yes",
        "export_acknowledged": "yes",
        "export_exact_consumer": "platform-broker",
        "export_malformed_rejected": "yes",
        "export_mismatch_preserved": "yes",
        "export_no_sealed_after_ack": "yes",
        "export_over_capacity_rejected": "yes",
        "export_partial_timeout": "bounded",
        "export_repeatable": "yes",
        "export_socket_mode": "660",
        "export_socket_owner": "static-shared",
        "export_unauthorized_rejected": "root-and-wrong-role",
        "five_sources_accepted": "5",
        "graceful_export_shutdown": "yes",
        "graceful_shutdown": "yes",
        "kvmd_active_after_run": "active",
        "kvmd_nginx_active_after_run": "active",
        "malformed_rejected": "yes",
        "partial_timeout": "bounded",
        "producer_during_export_stall": "bounded",
        "root_rejected": "yes",
        "rollback_confirmed": "yes",
        "runtime_removed": "yes",
        "socket_mode": "660",
        "socket_owner": "static-shared",
        "static_identities": "created-and-restored",
        "sysusers_synthetic": "yes",
        "unit_hash_verified": "yes",
        "unknown_rejected": "yes",
        "watchdog_armed": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("runtime invariant failed")


def validate_final(facts: dict[str, str], binary_parent_before: str) -> None:
    if binary_parent_before not in {"present", "absent"}:
        raise ValueError("binary parent baseline is invalid")
    expected = {
        "account_absent_final": "yes",
        "binary_absent_final": "yes",
        "binary_parent_final": binary_parent_before,
        "broker_runtime_absent_final": "yes",
        "broker_socket_absent_final": "yes",
        "broker_unit_absent_final": "yes",
        "kvmd_active_final": "active",
        "kvmd_nginx_active_final": "active",
        "rootfs_mode_final": "rw",
        "runtime_absent_final": "yes",
        "export_socket_absent_final": "yes",
        "stage_absent_final": "yes",
        "unit_absent_final": "yes",
        "watchdog_inactive_final": "yes",
    }
    if facts != dict(sorted(expected.items())):
        raise ValueError("final-state invariant failed")


def upload(
    target: prototype.RemoteTarget,
    secret: bytearray,
    source: Asset,
    script: str,
) -> None:
    with source.path.open("rb") as stream:
        _ = require_result(
            "upload",
            prototype.run_ssh(target, secret, script, stdin=stream, timeout=60),
        )


def attempt_cleanup(
    target: prototype.RemoteTarget,
    secret: bytearray,
    paths: Paths,
    assets: Assets,
) -> CleanupReceipt:
    try:
        output = require_result(
            "cleanup",
            prototype.run_ssh(
                target,
                secret,
                build_cleanup_script(paths, assets),
                timeout=45,
            ),
        )
        facts = native.parse_facts(output, CLEANUP_KEYS)
        return {"attempted": True, "confirmed": facts == {"cleanup_restored": "yes"}}
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError):
        return {"attempted": True, "confirmed": False}


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    assets = validate_assets(options)
    target = native.target_from_known_hosts(options.known_hosts, options.endpoint)
    paths = stage_paths(
        assets.binary.sha256,
        assets.broker_binary.sha256,
        secrets.token_hex(8),
    )
    required_bytes = (
        assets.binary.size_bytes * 4
        + assets.broker_binary.size_bytes * 2
        + MIN_TMP_HEADROOM_BYTES
    )
    secret = prototype.read_secret_stdin()
    stage = "preflight"
    cleanup_required = False
    try:
        preflight = native.parse_facts(
            require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_preflight_script(paths, required_bytes),
                    timeout=25,
                ),
            ),
            PREFLIGHT_KEYS,
        )
        validate_preflight(preflight)

        stage = "upload"
        cleanup_required = True
        upload(target, secret, assets.binary, build_first_upload_script(paths))
        upload(
            target,
            secret,
            assets.broker_binary,
            build_upload_script(paths, paths.broker_binary, "755"),
        )
        upload(target, secret, assets.unit, build_upload_script(paths, paths.unit, "600"))
        upload(
            target,
            secret,
            assets.base_sysusers,
            build_upload_script(paths, paths.base_sysusers, "600"),
        )
        upload(
            target,
            secret,
            assets.sysusers,
            build_upload_script(paths, paths.sysusers, "600"),
        )

        stage = "runtime-and-rollback"
        runtime_result = prototype.run_ssh(
            target,
            secret,
            build_run_script(paths, assets),
            timeout=180,
        )
        if runtime_result.returncode != 0:
            try:
                stage = f"runtime-{parse_runtime_failure(runtime_result.stdout)}"
            except (UnicodeError, ValueError):
                stage = "runtime-unknown"
            raise RuntimeError(stage)
        checks = native.parse_facts(
            require_result(stage, runtime_result),
            RUN_KEYS,
        )
        validate_run(checks)

        stage = "final-state"
        final_state = native.parse_facts(
            require_result(
                stage,
                prototype.run_ssh(
                    target,
                    secret,
                    build_final_script(paths, preflight["binary_parent_before"]),
                    timeout=25,
                ),
            ),
            FINAL_KEYS,
        )
        validate_final(final_state, preflight["binary_parent_before"])
        cleanup_required = False

        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-broker-route",
            "result": "passed",
            "sourceCommit": assets.source_commit,
            "artifacts": {
                "binary": {
                    "sha256": assets.binary.sha256,
                    "sizeBytes": assets.binary.size_bytes,
                },
                "brokerBinary": {
                    "sha256": assets.broker_binary.sha256,
                    "sizeBytes": assets.broker_binary.size_bytes,
                },
                "unitSha256": assets.unit.sha256,
                "baseSysusersSha256": assets.base_sysusers.sha256,
                "sysusersSha256": assets.sysusers.sha256,
            },
            "policy": policy_receipt(),
            "preflight": preflight,
            "checks": checks,
            "finalState": final_state,
            "cleanup": {"attempted": True, "confirmed": True},
        }
        write_json_0600_atomic(output, receipt)
        return 0
    except (OSError, RuntimeError, subprocess.SubprocessError, UnicodeError, ValueError) as error:
        cleanup = (
            attempt_cleanup(target, secret, paths, assets)
            if cleanup_required
            else {"attempted": False, "confirmed": True}
        )
        receipt = {
            "schemaVersion": 1,
            "probe": "hardened-blikvm-diagnostic-broker-route",
            "result": "failed",
            "sourceCommit": assets.source_commit,
            "failure": {"stage": stage, "failureType": type(error).__name__},
            "cleanup": cleanup,
        }
        write_json_0600_atomic(output, receipt)
        return 1
    finally:
        native.zero(secret)


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
