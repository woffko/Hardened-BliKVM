#!/usr/bin/env python3
"""Build ephemeral public-only signed application fixtures for update gates."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import importlib
import io
import json
import os
from pathlib import Path
import re
import sys
import tarfile
from typing import Protocol, TypedDict, cast


FORMAT = "hardened-blikvm-application-v1"
PRODUCT = "Hardened BliKVM"
TARGET = "bli-h616-pikvm"
ARCHITECTURE = "aarch64"
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
ALLOWED_ROOTS = (Path("/tmp"), Path("/mnt/d/debug/blikvm"))


class ArtifactReceipt(TypedDict):
    path: str
    sequence: int
    sha256: str
    sizeBytes: int


class FixtureReceipt(TypedDict):
    schemaVersion: int
    fixture: str
    keyId: str
    publicKeyHex: str
    sourceCommit: str
    artifacts: list[ArtifactReceipt]


class PublicKeyProtocol(Protocol):
    def public_bytes(self, *, encoding: object, format: object) -> bytes: ...


class SigningKeyProtocol(Protocol):
    def public_key(self) -> PublicKeyProtocol: ...

    def sign(self, data: bytes) -> bytes: ...


class SigningKeyFactoryProtocol(Protocol):
    def generate(self) -> SigningKeyProtocol: ...


class RawEnumProtocol(Protocol):
    Raw: object


class SerializationProtocol(Protocol):
    Encoding: RawEnumProtocol
    PublicFormat: RawEnumProtocol


@dataclass(frozen=True)
class Options:
    output_dir: str
    source_commit: str
    server_binary: str | None = None
    expected_server_sha256: str | None = None


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--output-dir", required=True)
    _ = parser.add_argument("--source-commit", required=True)
    _ = parser.add_argument("--server-binary")
    _ = parser.add_argument("--expected-server-sha256")
    namespace = parser.parse_args()
    return Options(
        output_dir=cast(str, namespace.output_dir),
        source_commit=cast(str, namespace.source_commit),
        server_binary=cast(str | None, namespace.server_binary),
        expected_server_sha256=cast(str | None, namespace.expected_server_sha256),
    )


def validate_new_output(path_value: str) -> Path:
    path = Path(path_value)
    if not path.is_absolute() or path.name in ("", ".", ".."):
        raise ValueError("output directory must be an absolute child path")
    resolved = path.resolve(strict=False)
    if not any(resolved.is_relative_to(root) and resolved != root for root in ALLOWED_ROOTS):
        raise ValueError("output directory is outside approved temporary roots")
    if path.exists() or path.is_symlink():
        raise ValueError("output directory must not already exist")
    parent = resolved.parent
    if not parent.is_dir() or parent.is_symlink():
        raise ValueError("output parent must be an existing directory")
    return resolved


def validate_source_commit(value: str) -> str:
    if not SOURCE_COMMIT.fullmatch(value) or set(value) == {"0"}:
        raise ValueError("source commit must be nonzero lowercase 40-hex")
    return value


def payload(sequence: int) -> dict[str, tuple[int, bytes]]:
    return {
        "bin/bli-kvm-server": (0o755, f"device-gate-server-{sequence}".encode()),
        "libexec/bli-kvm-control-broker": (
            0o755,
            f"device-gate-control-{sequence}".encode(),
        ),
        "libexec/bli-kvm-platform-broker": (
            0o755,
            f"device-gate-broker-{sequence}".encode(),
        ),
        "libexec/bli-kvm-video-proxy": (
            0o755,
            f"device-gate-video-{sequence}".encode(),
        ),
        "share/web/index.html": (
            0o644,
            f"<html>device-gate-{sequence}</html>".encode(),
        ),
    }


def manifest_document(
    sequence: int,
    key_id: str,
    source_commit: str,
    files: dict[str, tuple[int, bytes]],
) -> dict[str, object]:
    return {
        "format": FORMAT,
        "product": PRODUCT,
        "target": TARGET,
        "architecture": ARCHITECTURE,
        "keyId": key_id,
        "version": f"0.0.{sequence}-device-gate",
        "sequence": sequence,
        "minimumSystemSequence": 1,
        "sourceCommit": source_commit,
        "files": [
            {
                "path": path,
                "mode": mode,
                "size": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
            for path, (mode, content) in sorted(files.items())
        ],
    }


def _tar_info(name: str, mode: int, size: int, entry_type: bytes) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.mode = mode
    info.uid = 0
    info.gid = 0
    info.mtime = 0
    info.size = size
    info.type = entry_type
    return info


def _directories(files: dict[str, tuple[int, bytes]]) -> set[str]:
    directories = {"payload"}
    for relative in files:
        current = Path("payload")
        for component in Path(relative).parts[:-1]:
            current /= component
            directories.add(current.as_posix())
    return directories


def build_bundle(
    signing_key: SigningKeyProtocol,
    sequence: int,
    key_id: str,
    source_commit: str,
    server_binary: bytes | None = None,
) -> bytes:
    files = payload(sequence)
    if server_binary is not None and sequence in (1, 2):
        files["bin/bli-kvm-server"] = (0o755, server_binary)
    manifest = json.dumps(
        manifest_document(sequence, key_id, source_commit, files),
        ensure_ascii=True,
        separators=(",", ":"),
    ).encode("ascii")
    signature = signing_key.sign(manifest)
    if len(signature) != 64:
        raise ValueError("unexpected Ed25519 signature size")
    directories = _directories(files)
    entries = {"manifest.json", "manifest.sig", *directories}
    entries.update(f"payload/{path}" for path in files)
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for name in sorted(entries):
            if name == "manifest.json":
                archive.addfile(
                    _tar_info(name, 0o644, len(manifest), tarfile.REGTYPE),
                    io.BytesIO(manifest),
                )
            elif name == "manifest.sig":
                archive.addfile(
                    _tar_info(name, 0o644, len(signature), tarfile.REGTYPE),
                    io.BytesIO(signature),
                )
            elif name in directories:
                archive.addfile(_tar_info(name, 0o755, 0, tarfile.DIRTYPE))
            else:
                relative = name.removeprefix("payload/")
                mode, content = files[relative]
                archive.addfile(
                    _tar_info(name, mode, len(content), tarfile.REGTYPE),
                    io.BytesIO(content),
                )
    return output.getvalue()


def write_private(path: Path, content: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
    finally:
        os.close(descriptor)


def build_fixture(output: Path, source_commit: str, server_binary: bytes | None = None) -> FixtureReceipt:
    output.mkdir(mode=0o700)
    ed25519 = importlib.import_module(
        "cryptography.hazmat.primitives.asymmetric.ed25519"
    )
    factory = cast(SigningKeyFactoryProtocol, getattr(ed25519, "Ed25519PrivateKey"))
    signing_key = factory.generate()
    serialization = cast(
        SerializationProtocol,
        importlib.import_module("cryptography.hazmat.primitives.serialization"),
    )
    public_key = signing_key.public_key().public_bytes(
        encoding=serialization.Encoding.Raw,
        format=serialization.PublicFormat.Raw,
    )
    key_id = f"app-device-gate-{hashlib.sha256(public_key).hexdigest()[:16]}"
    artifacts: list[ArtifactReceipt] = []
    for sequence in (1, 2, 3):
        content = build_bundle(signing_key, sequence, key_id, source_commit, server_binary)
        name = f"application-{sequence}.tar"
        write_private(output / name, content)
        artifacts.append(
            {
                "path": name,
                "sequence": sequence,
                "sha256": hashlib.sha256(content).hexdigest(),
                "sizeBytes": len(content),
            }
        )
    receipt: FixtureReceipt = {
        "schemaVersion": 1,
        "fixture": "hardened-blikvm-ephemeral-update-device-gate",
        "keyId": key_id,
        "publicKeyHex": public_key.hex(),
        "sourceCommit": source_commit,
        "artifacts": artifacts,
    }
    write_private(
        output / "fixture.json",
        json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode("ascii"),
    )
    directory = os.open(output, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    return receipt


def execute(options: Options) -> int:
    output = validate_new_output(options.output_dir)
    source_commit = validate_source_commit(options.source_commit)
    server_binary = None
    if options.server_binary is not None or options.expected_server_sha256 is not None:
        if options.server_binary is None or options.expected_server_sha256 is None:
            raise ValueError("both server input and hash binding are required")
        path = Path(options.server_binary)
        if not path.is_absolute() or path.is_symlink() or not path.is_file() or not 0 < path.stat().st_size <= 8 * 1024 * 1024:
            raise ValueError("unsafe server fixture input")
        server_binary = path.read_bytes()
        if not server_binary.startswith(b"\x7fELF") or hashlib.sha256(server_binary).hexdigest() != options.expected_server_sha256:
            raise ValueError("server fixture binding mismatch")
    receipt = build_fixture(output, source_commit, server_binary)
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"fixture build failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
