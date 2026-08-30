#!/usr/bin/env python3
"""Build public-only signed system-image fixtures with an ephemeral key."""

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


FORMAT = "hardened-blikvm-system-v1"
PRODUCT = "Hardened BliKVM"
TARGET = "bli-h616-pikvm"
ARCHITECTURE = "aarch64"
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
ALLOWED_ROOTS = (Path("/tmp"), Path("/mnt/d/debug/blikvm"))
ROOTFS_BYTES = 4096
BOOT_BYTES = 1024


class ArtifactReceipt(TypedDict):
    path: str
    sequence: int
    layoutId: str
    rootfsBytes: int
    bootBytes: int | None
    sha256: str
    sizeBytes: int


class FixtureReceipt(TypedDict):
    schemaVersion: int
    fixture: str
    keyId: str
    publicKeyHex: str
    sourceCommit: str
    posixModesEnforced: bool
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


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--output-dir", required=True)
    _ = parser.add_argument("--source-commit", required=True)
    namespace = parser.parse_args()
    return Options(
        output_dir=cast(str, namespace.output_dir),
        source_commit=cast(str, namespace.source_commit),
    )


def validate_new_output(path_value: str) -> Path:
    path = Path(path_value)
    if not path.is_absolute() or path.name in ("", ".", ".."):
        raise ValueError("output directory must be an absolute child path")
    resolved = path.resolve(strict=False)
    if not any(
        resolved.is_relative_to(root) and resolved != root for root in ALLOWED_ROOTS
    ):
        raise ValueError("output directory is outside approved temporary roots")
    if path.exists() or path.is_symlink():
        raise ValueError("output directory must not already exist")
    if not resolved.parent.is_dir() or resolved.parent.is_symlink():
        raise ValueError("output parent must be an existing directory")
    return resolved


def validate_source_commit(value: str) -> str:
    if not SOURCE_COMMIT.fullmatch(value) or set(value) == {"0"}:
        raise ValueError("source commit must be nonzero lowercase 40-hex")
    return value


def rootfs_payload(sequence: int) -> bytes:
    payload = bytearray((index + sequence * 17) % 251 for index in range(ROOTFS_BYTES))
    payload[1080:1082] = b"\x53\xef"
    return bytes(payload)


def boot_payload(sequence: int) -> bytes:
    payload = bytearray((index + sequence * 29) % 251 for index in range(BOOT_BYTES))
    payload[510:512] = b"\x55\xaa"
    return bytes(payload)


def manifest_document(
    sequence: int,
    key_id: str,
    source_commit: str,
    layout_id: str,
    rootfs: bytes,
    boot: bytes | None,
) -> dict[str, object]:
    components: list[dict[str, object]] = []
    if boot is not None:
        components.append(
            {
                "role": "boot",
                "path": "boot.img",
                "contentType": "vfat",
                "size": len(boot),
                "sha256": hashlib.sha256(boot).hexdigest(),
            }
        )
    components.append(
        {
            "role": "rootfs",
            "path": "rootfs.img",
            "contentType": "ext4",
            "size": len(rootfs),
            "sha256": hashlib.sha256(rootfs).hexdigest(),
        }
    )
    return {
        "format": FORMAT,
        "product": PRODUCT,
        "target": TARGET,
        "architecture": ARCHITECTURE,
        "keyId": key_id,
        "version": f"0.0.{sequence}-fixture",
        "sequence": sequence,
        "minimumRecoverySequence": 1,
        "minimumBootControlSequence": 1,
        "layoutId": layout_id,
        "sourceCommit": source_commit,
        "components": components,
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


def build_bundle(
    signing_key: SigningKeyProtocol,
    sequence: int,
    key_id: str,
    source_commit: str,
    layout_id: str,
    with_boot: bool,
) -> bytes:
    rootfs = rootfs_payload(sequence)
    boot = boot_payload(sequence) if with_boot else None
    manifest = json.dumps(
        manifest_document(
            sequence,
            key_id,
            source_commit,
            layout_id,
            rootfs,
            boot,
        ),
        ensure_ascii=True,
        separators=(",", ":"),
    ).encode("ascii")
    signature = signing_key.sign(manifest)
    if len(signature) != 64:
        raise ValueError("unexpected Ed25519 signature size")

    payloads = {"payload/rootfs.img": rootfs}
    if boot is not None:
        payloads["payload/boot.img"] = boot
    entries = ["manifest.json", "manifest.sig", "payload/", *payloads]
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
            elif name == "payload/":
                archive.addfile(_tar_info(name, 0o755, 0, tarfile.DIRTYPE))
            else:
                content = payloads[name]
                archive.addfile(
                    _tar_info(name, 0o644, len(content), tarfile.REGTYPE),
                    io.BytesIO(content),
                )
    return output.getvalue()


def write_exclusive(path: Path, content: bytes) -> None:
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
    finally:
        os.close(descriptor)


def build_fixture(output: Path, source_commit: str) -> FixtureReceipt:
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
    key_id = f"sys-fixture-{hashlib.sha256(public_key).hexdigest()[:16]}"
    definitions = (
        ("system-rootfs-only.tar", 1, "layout-h616-fixture-rootfs-v1", False),
        ("system-split-boot.tar", 2, "layout-h616-fixture-split-v1", True),
    )
    artifacts: list[ArtifactReceipt] = []
    for name, sequence, layout_id, with_boot in definitions:
        content = build_bundle(
            signing_key,
            sequence,
            key_id,
            source_commit,
            layout_id,
            with_boot,
        )
        write_exclusive(output / name, content)
        artifacts.append(
            {
                "path": name,
                "sequence": sequence,
                "layoutId": layout_id,
                "rootfsBytes": ROOTFS_BYTES,
                "bootBytes": BOOT_BYTES if with_boot else None,
                "sha256": hashlib.sha256(content).hexdigest(),
                "sizeBytes": len(content),
            }
        )
    posix_modes_enforced = (output.stat().st_mode & 0o777) == 0o700 and all(
        (output / artifact["path"]).stat().st_mode & 0o777 == 0o600
        for artifact in artifacts
    )
    receipt: FixtureReceipt = {
        "schemaVersion": 1,
        "fixture": "hardened-blikvm-ephemeral-system-image",
        "keyId": key_id,
        "publicKeyHex": public_key.hex(),
        "sourceCommit": source_commit,
        "posixModesEnforced": posix_modes_enforced,
        "artifacts": artifacts,
    }
    write_exclusive(
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
    receipt = build_fixture(output, source_commit)
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
