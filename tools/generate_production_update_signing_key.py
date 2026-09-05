#!/usr/bin/env python3
"""Generate one production Ed25519 application-update signing key.

The private key is created outside the repository under an owner-only directory.
Encrypted mode is the default. Passwordless mode requires an explicit host-risk
acknowledgement. This process never accepts, stores, or prints a passphrase.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import stat
import subprocess
import sys
from typing import Final, cast


PRODUCT: Final = "Hardened BliKVM"
TARGET: Final = "bli-h616-pikvm"
SPKI_PREFIX: Final = bytes.fromhex("302a300506032b6570032100")
KEY_ID_PATTERN: Final = re.compile(r"^app-[a-z0-9._-]{1,60}$")
PRIVATE_KEY_NAME: Final = "private-key.pk8.pem"
PUBLIC_KEY_NAME: Final = "public-key.pem"
PUBLIC_RAW_NAME: Final = "public-key.raw"
REQUEST_NAME: Final = "enrollment-request.json"
REQUEST_HASH_NAME: Final = "enrollment-request.sha256"
REVIEW_NAME: Final = "REVIEW.txt"
POLICY_NAME: Final = "key-policy.json"
ENCRYPTED_PROTECTION: Final = "openssl-aes-256-cbc"
UNENCRYPTED_PROTECTION: Final = "unencrypted-owner-only"


@dataclass(frozen=True)
class Options:
    key_id: str
    without_passphrase: bool
    acknowledge_unencrypted_host_key_risk: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--key-id", required=True)
    _ = parser.add_argument("--without-passphrase", action="store_true")
    _ = parser.add_argument(
        "--acknowledge-unencrypted-host-key-risk", action="store_true"
    )
    namespace = parser.parse_args()
    options = Options(
        key_id=validate_key_id(cast(str, namespace.key_id)),
        without_passphrase=cast(bool, namespace.without_passphrase),
        acknowledge_unencrypted_host_key_risk=cast(
            bool, namespace.acknowledge_unencrypted_host_key_risk
        ),
    )
    if options.without_passphrase != options.acknowledge_unencrypted_host_key_risk:
        raise ValueError(
            "passwordless mode requires both explicit unencrypted-key flags"
        )
    return options


def validate_key_id(value: str) -> str:
    if (
        not (5 <= len(value) <= 64)
        or KEY_ID_PATTERN.fullmatch(value) is None
        or value.endswith(".")
        or ".." in value
    ):
        raise ValueError("key ID does not satisfy the application verifier contract")
    return value


def secure_home() -> Path:
    entry = pwd.getpwuid(os.geteuid())
    home = Path(entry.pw_dir)
    if not home.is_absolute() or not home.is_dir() or home.is_symlink():
        raise ValueError("login home directory is unsafe")
    if home.stat().st_uid != os.geteuid():
        raise ValueError("login home directory has an unexpected owner")
    return home.resolve(strict=True)


def ensure_private_directory(path: Path, *, create: bool) -> None:
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=False)
    metadata = path.lstat()
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or path.is_symlink()
        or metadata.st_uid != os.geteuid()
        or stat.S_IMODE(metadata.st_mode) != 0o700
    ):
        raise ValueError(f"private directory is unsafe: {path}")


def create_new_file(path: Path, content: bytes, mode: int) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, mode)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.fchmod(descriptor, mode)
    finally:
        os.close(descriptor)


def generate_private_key(path: Path, *, encrypted: bool) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, 0o600)
    try:
        arguments = ["openssl", "genpkey", "-algorithm", "Ed25519"]
        if encrypted:
            arguments.append("-aes-256-cbc")
        subprocess.run(
            arguments,
            check=True,
            stdin=None,
            stdout=descriptor,
        )
        os.fsync(descriptor)
        os.fchmod(descriptor, 0o400)
    finally:
        os.close(descriptor)


def generate_encrypted_private_key(path: Path) -> None:
    generate_private_key(path, encrypted=True)


def derive_public_der(private_key: Path) -> bytes:
    flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(private_key, flags)
    try:
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_uid != os.geteuid()
            or stat.S_IMODE(metadata.st_mode) != 0o400
            or metadata.st_nlink != 1
        ):
            raise ValueError("private key metadata is unsafe")
        result = subprocess.run(
            [
                "openssl",
                "pkey",
                "-in",
                f"/proc/self/fd/{descriptor}",
                "-pubout",
                "-outform",
                "DER",
            ],
            check=True,
            stdin=None,
            stdout=subprocess.PIPE,
            pass_fds=(descriptor,),
        )
    finally:
        os.close(descriptor)
    return result.stdout


def raw_public_key(public_der: bytes) -> bytes:
    if len(public_der) != len(SPKI_PREFIX) + 32 or not public_der.startswith(SPKI_PREFIX):
        raise ValueError("OpenSSL returned an unexpected Ed25519 public-key encoding")
    public_key = public_der[len(SPKI_PREFIX) :]
    if public_key == bytes(32):
        raise ValueError("OpenSSL returned an invalid all-zero public key")
    return public_key


def public_pem(public_der: bytes) -> bytes:
    encoded = base64.b64encode(public_der).decode("ascii")
    return (
        "-----BEGIN PUBLIC KEY-----\n"
        + "\n".join(encoded[index : index + 64] for index in range(0, len(encoded), 64))
        + "\n-----END PUBLIC KEY-----\n"
    ).encode("ascii")


def enrollment_request(key_id: str, public_key: bytes) -> bytes:
    public_hex = public_key.hex()
    public_sha256 = hashlib.sha256(public_key).hexdigest()
    document: dict[str, object] = {
        "schemaVersion": 1,
        "product": PRODUCT,
        "target": TARGET,
        "generation": 1,
        "replacesKeyId": None,
        "keyId": key_id,
        "publicKeyHex": public_hex,
        "publicKeySha256": public_sha256,
    }
    return json.dumps(document, separators=(",", ":"), ensure_ascii=True).encode("ascii") + b"\n"


def review_document(
    key_id: str, public_key: bytes, request: bytes, protection: str
) -> bytes:
    public_sha256 = hashlib.sha256(public_key).hexdigest()
    request_sha256 = hashlib.sha256(request).hexdigest()
    return (
        "Hardened BliKVM production application-update signing key\n"
        f"keyId={key_id}\n"
        f"publicKeySha256={public_sha256}\n"
        f"enrollmentRequestSha256={request_sha256}\n"
        f"privateKeyProtection={protection}\n"
        "\n"
        "Review these public values before enrollment. Keep private-key.pk8.pem\n"
        "offline and backed up separately. Never copy it into Git, /tmp, Project\n"
        "Memory, the BliKVM device, chat, logs, or an update bundle.\n"
    ).encode("ascii")


def policy_document(key_id: str, protection: str) -> bytes:
    if protection not in {ENCRYPTED_PROTECTION, UNENCRYPTED_PROTECTION}:
        raise ValueError("private-key protection policy is invalid")
    return (
        json.dumps(
            {
                "schemaVersion": 1,
                "keyId": validate_key_id(key_id),
                "privateKeyProtection": protection,
                "hostRiskAcknowledged": protection == UNENCRYPTED_PROTECTION,
                "privateKeyIncluded": False,
            },
            sort_keys=True,
            separators=(",", ":"),
        ).encode("ascii")
        + b"\n"
    )


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def main() -> int:
    options = parse_args()
    encrypted = not options.without_passphrase
    if encrypted and (not sys.stdin.isatty() or not sys.stderr.isatty()):
        raise ValueError("encrypted generation requires an interactive local terminal")
    key_id = options.key_id
    protection = ENCRYPTED_PROTECTION if encrypted else UNENCRYPTED_PROTECTION
    home = secure_home()
    root = home / ".local" / "share" / "hardened-blikvm" / "production-signing"
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    ensure_private_directory(root, create=False)
    destination = root / key_id
    ensure_private_directory(destination, create=True)

    private_key = destination / PRIVATE_KEY_NAME
    if encrypted:
        print("Create a strong unique passphrase when OpenSSL prompts twice.", flush=True)
    else:
        print(
            "Generating an explicitly acknowledged passwordless owner-only key.",
            flush=True,
        )
    generate_private_key(private_key, encrypted=encrypted)
    if encrypted:
        print("Enter the same passphrase once more to derive the public key.", flush=True)
    public_der = derive_public_der(private_key)
    public_key = raw_public_key(public_der)
    request = enrollment_request(key_id, public_key)
    request_sha256 = hashlib.sha256(request).hexdigest()

    create_new_file(destination / PUBLIC_KEY_NAME, public_pem(public_der), 0o400)
    create_new_file(destination / PUBLIC_RAW_NAME, public_key, 0o400)
    create_new_file(destination / REQUEST_NAME, request, 0o400)
    create_new_file(
        destination / REQUEST_HASH_NAME,
        (request_sha256 + "\n").encode("ascii"),
        0o400,
    )
    create_new_file(
        destination / REVIEW_NAME,
        review_document(key_id, public_key, request, protection),
        0o400,
    )
    create_new_file(
        destination / POLICY_NAME,
        policy_document(key_id, protection),
        0o400,
    )
    sync_directory(destination)
    sync_directory(root)

    print()
    print(request.decode("ascii"), end="")
    print(f"enrollmentRequestSha256={request_sha256}")
    print(f"Private key directory: {destination}")
    print("Generation completed. No device enrollment was performed.")
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (OSError, subprocess.CalledProcessError, ValueError) as error:
        print(f"production key generation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from None
