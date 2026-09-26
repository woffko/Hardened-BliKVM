#!/usr/bin/env python3
"""Build a signed RAW system bundle from the SD image builder's root image.

The bundle is what `bli-kvm-system-image-verifier` reads and the RAW system
update stages (docs/design/system-raw-update-v1.md): `manifest.json` (compact
JSON in the verifier's field order), `manifest.sig` (Ed25519 over the
manifest bytes with the system key), `payload/` and
`payload/rootfs.img.zst`, every entry owned by 0:0 with mtime 0.

The root image is shrunk to its content first (`resize2fs -M`); the device
grows it to the partition after writing. The private key stays outside the
repository and is used through OpenSSL; the trust file names the key id,
public key and card layout the bundle must match.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile


FORMAT = "hardened-blikvm-system-v1"
PRODUCT = "Hardened BliKVM"
TARGET = "bli-h616-pikvm"
ARCHITECTURE = "aarch64"
PAYLOAD_NAME = "rootfs.img.zst"
SOURCE_COMMIT = re.compile(r"^[0-9a-f]{40}$")
VERSION = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$")
BLOCK = 4096
# DER prefix of an Ed25519 SubjectPublicKeyInfo; the raw key follows.
ED25519_SPKI_PREFIX = bytes.fromhex("302a300506032b6570032100")


class BuildError(Exception):
    pass


def run(*command: str) -> str:
    result = subprocess.run(
        command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False, text=True
    )
    if result.returncode != 0:
        raise BuildError(f"{command[0]} failed: {result.stderr.strip()}")
    return result.stdout


def sha256_file(path: Path) -> tuple[int, str]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as handle:
        while chunk := handle.read(1 << 20):
            digest.update(chunk)
            size += len(chunk)
    return size, digest.hexdigest()


def public_key_hex(private_key: Path) -> str:
    der = subprocess.run(
        ["openssl", "pkey", "-in", str(private_key), "-pubout", "-outform", "DER"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if der.returncode != 0 or not der.stdout.startswith(ED25519_SPKI_PREFIX):
        raise BuildError("the signing key is not an Ed25519 private key")
    return der.stdout[len(ED25519_SPKI_PREFIX):].hex()


def shrink_image(source: Path, target: Path) -> int:
    """Copies the ext4 image, shrinks it to its content and returns its size."""
    shutil.copyfile(source, target)
    fsck = subprocess.run(
        ["e2fsck", "-f", "-y", str(target)],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False,
    )
    if fsck.returncode not in (0, 1):
        raise BuildError("the root image has file-system errors")
    run("resize2fs", "-M", str(target))
    info = run("dumpe2fs", "-h", str(target))
    fields = dict(
        line.split(":", 1) for line in info.splitlines() if ":" in line
    )
    blocks = int(fields["Block count"].strip())
    block_size = int(fields["Block size"].strip())
    if block_size != BLOCK:
        raise BuildError("the root image must use 4096-byte blocks")
    size = blocks * block_size
    os.truncate(target, size)
    return size


def manifest_bytes(
    *,
    key_id: str,
    version: str,
    sequence: int,
    layout_id: str,
    source_commit: str,
    payload: tuple[int, str],
    image: tuple[int, str],
) -> bytes:
    # Field order and separators match the verifier's canonical form.
    manifest = {
        "format": FORMAT,
        "product": PRODUCT,
        "target": TARGET,
        "architecture": ARCHITECTURE,
        "keyId": key_id,
        "version": version,
        "sequence": sequence,
        "minimumRecoverySequence": 1,
        "minimumBootControlSequence": 1,
        "layoutId": layout_id,
        "sourceCommit": source_commit,
        "components": [
            {
                "role": "rootfs",
                "path": PAYLOAD_NAME,
                "contentType": "ext4_zstd",
                "size": payload[0],
                "sha256": payload[1],
                "imageSize": image[0],
                "imageSha256": image[1],
            }
        ],
    }
    return json.dumps(manifest, separators=(",", ":"), ensure_ascii=False).encode()


def sign(private_key: Path, data: bytes, work: Path) -> bytes:
    message = work / "manifest.json"
    signature = work / "manifest.sig"
    message.write_bytes(data)
    run(
        "openssl", "pkeyutl", "-sign", "-rawin", "-inkey", str(private_key),
        "-in", str(message), "-out", str(signature),
    )
    raw = signature.read_bytes()
    if len(raw) != 64:
        raise BuildError("OpenSSL did not produce an Ed25519 signature")
    return raw


def tar_info(name: str, size: int, directory: bool = False) -> tarfile.TarInfo:
    info = tarfile.TarInfo(name)
    info.type = tarfile.DIRTYPE if directory else tarfile.REGTYPE
    info.mode = 0o755 if directory else 0o644
    info.uid = info.gid = 0
    info.uname = info.gname = ""
    info.mtime = 0
    info.size = 0 if directory else size
    return info


def write_bundle(
    output: Path, manifest: bytes, signature: bytes, payload: Path
) -> None:
    temporary = output.with_name(f".{output.name}.part")
    with tarfile.open(temporary, "w", format=tarfile.GNU_FORMAT) as archive:
        archive.addfile(tar_info("manifest.json", len(manifest)), io.BytesIO(manifest))
        archive.addfile(tar_info("manifest.sig", len(signature)), io.BytesIO(signature))
        archive.addfile(tar_info("payload/", 0, directory=True))
        with payload.open("rb") as handle:
            archive.addfile(
                tar_info(f"payload/{PAYLOAD_NAME}", payload.stat().st_size), handle
            )
    temporary.chmod(0o644)
    temporary.replace(output)


def build(
    *,
    rootfs: Path,
    private_key: Path,
    trust: Path,
    version: str,
    sequence: int,
    source_commit: str,
    output_dir: Path,
    compression_level: int = 19,
) -> Path:
    if not VERSION.match(version):
        raise BuildError("invalid version")
    if not SOURCE_COMMIT.match(source_commit):
        raise BuildError("the source commit must be 40 lowercase hex digits")
    if sequence < 1:
        raise BuildError("the sequence must be positive")
    trust_root = json.loads(trust.read_text())
    if set(trust_root) != {"keyId", "publicKey", "layoutId"}:
        raise BuildError("the trust file is invalid")
    if public_key_hex(private_key) != trust_root["publicKey"]:
        raise BuildError("the signing key does not match the trust file")
    output_dir.mkdir(parents=True, exist_ok=True)
    output = output_dir / f"hardened-blikvm-system-{version}-seq{sequence}.tar"
    with tempfile.TemporaryDirectory(prefix="hardened-system-bundle.") as temporary:
        work = Path(temporary)
        image = work / "root.ext4"
        shrink_image(rootfs, image)
        image_digest = sha256_file(image)
        payload = work / PAYLOAD_NAME
        run("zstd", "-q", f"-{compression_level}", "-T0", str(image), "-o", str(payload))
        manifest = manifest_bytes(
            key_id=trust_root["keyId"],
            version=version,
            sequence=sequence,
            layout_id=trust_root["layoutId"],
            source_commit=source_commit,
            payload=sha256_file(payload),
            image=image_digest,
        )
        signature = sign(private_key, manifest, work)
        write_bundle(output, manifest, signature, payload)
    return output


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rootfs", required=True, type=Path)
    parser.add_argument("--key", required=True, type=Path)
    parser.add_argument(
        "--trust", type=Path, default=Path("packaging/system-update/system-trust.json")
    )
    parser.add_argument("--version", required=True)
    parser.add_argument("--sequence", required=True, type=int)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    arguments = parser.parse_args()
    try:
        bundle = build(
            rootfs=arguments.rootfs,
            private_key=arguments.key,
            trust=arguments.trust,
            version=arguments.version,
            sequence=arguments.sequence,
            source_commit=arguments.source_commit,
            output_dir=arguments.output_dir,
        )
    except (BuildError, OSError, KeyError, ValueError) as error:
        print(f"build_system_raw_bundle: {error}", file=sys.stderr)
        return 1
    size, digest = sha256_file(bundle)
    print(f"{digest}  {bundle}  ({size} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
