#!/usr/bin/env python3
"""Build one production-signed application bundle from an accepted runtime package."""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import stat
import subprocess
import sys
import tarfile
from typing import Final, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import generate_production_update_signing_key as key_ceremony
from tools import stage_device_debian_runtime_package as package_gate
from tools import stage_device_prototype as prototype


FORMAT: Final = "hardened-blikvm-application-v1"
PRODUCT: Final = "Hardened BliKVM"
TARGET: Final = "bli-h616-pikvm"
ARCHITECTURE: Final = "aarch64"
MAX_REQUEST_BYTES: Final = 4096
MAX_MANIFEST_BYTES: Final = 64 * 1024
MAX_BUNDLE_BYTES: Final = 128 * 1024 * 1024
MAX_PAYLOAD_BYTES: Final = 256 * 1024 * 1024
MAX_PAYLOAD_FILES: Final = 4096
PRIVATE_KEY_MAX_BYTES: Final = 4096
ALLOWED_OUTPUT_ROOTS: Final = (Path("/tmp"), Path("/mnt/d/debug/blikvm"))
RUNTIME_BINARY_MAPPING: Final = {
    PurePosixPath("usr/libexec/hardened-blikvm/bli-kvm-server"): "bin/bli-kvm-server",
    PurePosixPath(
        "usr/libexec/hardened-blikvm/bli-kvm-platform-broker"
    ): "libexec/bli-kvm-platform-broker",
    PurePosixPath(
        "usr/libexec/hardened-blikvm/bli-kvm-video-proxy"
    ): "libexec/bli-kvm-video-proxy",
    PurePosixPath(
        "usr/libexec/hardened-blikvm/bli-kvm-control-broker"
    ): "libexec/bli-kvm-control-broker",
}
WEB_PREFIX: Final = PurePosixPath("usr/share/hardened-blikvm/web")
REQUIRED_BUNDLE_PATHS: Final = {
    "bin/bli-kvm-server",
    "libexec/bli-kvm-platform-broker",
    "libexec/bli-kvm-video-proxy",
    "libexec/bli-kvm-control-broker",
    "share/web/index.html",
}


@dataclass(frozen=True)
class Options:
    package: str
    expected_package_sha256: str
    expected_source_commit: str
    expected_source_bundle_sha256: str
    expected_builder_commit: str
    enrollment_request: str
    expected_enrollment_request_sha256: str
    key_id: str
    version: str
    sequence: int
    minimum_system_sequence: int
    output_dir: str


@dataclass(frozen=True)
class Enrollment:
    key_id: str
    public_key: bytes
    request_sha256: str


class BundleReceipt(TypedDict):
    schemaVersion: int
    artifact: str
    bundleSha256: str
    bundleBytes: int
    manifestSha256: str
    payloadBytes: int
    payloadFiles: int
    keyId: str
    publicKeySha256: str
    enrollmentRequestSha256: str
    packageSha256: str
    sourceCommit: str
    version: str
    sequence: int
    minimumSystemSequence: int
    privateKeyIncluded: bool
    ownerOnlySnapshotRequired: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--package", required=True)
    _ = parser.add_argument("--expected-package-sha256", required=True)
    _ = parser.add_argument("--expected-source-commit", required=True)
    _ = parser.add_argument("--expected-source-bundle-sha256", required=True)
    _ = parser.add_argument("--expected-builder-commit", required=True)
    _ = parser.add_argument("--enrollment-request", required=True)
    _ = parser.add_argument("--expected-enrollment-request-sha256", required=True)
    _ = parser.add_argument("--key-id", required=True)
    _ = parser.add_argument("--version", required=True)
    _ = parser.add_argument("--sequence", required=True, type=int)
    _ = parser.add_argument("--minimum-system-sequence", required=True, type=int)
    _ = parser.add_argument("--output-dir", required=True)
    namespace = parser.parse_args()
    return Options(
        package=cast(str, namespace.package),
        expected_package_sha256=cast(str, namespace.expected_package_sha256),
        expected_source_commit=cast(str, namespace.expected_source_commit),
        expected_source_bundle_sha256=cast(
            str, namespace.expected_source_bundle_sha256
        ),
        expected_builder_commit=cast(str, namespace.expected_builder_commit),
        enrollment_request=cast(str, namespace.enrollment_request),
        expected_enrollment_request_sha256=cast(
            str, namespace.expected_enrollment_request_sha256
        ),
        key_id=cast(str, namespace.key_id),
        version=cast(str, namespace.version),
        sequence=cast(int, namespace.sequence),
        minimum_system_sequence=cast(int, namespace.minimum_system_sequence),
        output_dir=cast(str, namespace.output_dir),
    )


def validate_version(value: str) -> str:
    if (
        not 1 <= len(value) <= 64
        or not value[0].isdigit()
        or not value[-1].isalnum()
        or ".." in value
        or any(not (character.isascii() and (character.isalnum() or character in ".+-")) for character in value)
    ):
        raise ValueError("application version is invalid")
    return value


def validate_output_directory(path_value: str) -> Path:
    path = Path(path_value)
    if not path.is_absolute() or path.name in ("", ".", "..") or path.is_symlink():
        raise ValueError("output directory must be a new absolute child path")
    resolved = path.resolve(strict=False)
    if not any(
        resolved.is_relative_to(root) and resolved != root for root in ALLOWED_OUTPUT_ROOTS
    ):
        raise ValueError("output directory is outside approved staging roots")
    if path.exists():
        raise ValueError("output directory already exists")
    parent = resolved.parent
    if not parent.is_dir() or parent.is_symlink():
        raise ValueError("output parent is unsafe")
    return resolved


def _hash_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def load_enrollment(
    path_value: str, expected_sha256: str, expected_key_id: str
) -> Enrollment:
    path = Path(path_value)
    expected = prototype.validate_sha256(expected_sha256)
    key_id = key_ceremony.validate_key_id(expected_key_id)
    if not path.is_absolute() or path.is_symlink():
        raise ValueError("enrollment request path is unsafe")
    resolved = path.resolve(strict=True)
    metadata = resolved.stat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or metadata.st_mode & 0o022
        or not 0 < metadata.st_size <= MAX_REQUEST_BYTES
        or _hash_file(resolved) != expected
    ):
        raise ValueError("enrollment request metadata or hash is invalid")
    decoded = cast(object, json.loads(resolved.read_bytes()))
    if not isinstance(decoded, dict):
        raise ValueError("enrollment request root is invalid")
    request = cast(dict[str, object], decoded)
    if set(request) != {
        "schemaVersion",
        "product",
        "target",
        "generation",
        "replacesKeyId",
        "keyId",
        "publicKeyHex",
        "publicKeySha256",
    }:
        raise ValueError("enrollment request fields are invalid")
    if (
        request.get("schemaVersion") != 1
        or request.get("product") != PRODUCT
        or request.get("target") != TARGET
        or request.get("generation") != 1
        or request.get("replacesKeyId") is not None
        or request.get("keyId") != key_id
    ):
        raise ValueError("enrollment request identity is invalid")
    public_hex = request.get("publicKeyHex")
    fingerprint = request.get("publicKeySha256")
    if (
        not isinstance(public_hex, str)
        or len(public_hex) != 64
        or any(character not in "0123456789abcdef" for character in public_hex)
        or not isinstance(fingerprint, str)
    ):
        raise ValueError("enrollment public key is invalid")
    public_key = bytes.fromhex(public_hex)
    if public_key == bytes(32) or hashlib.sha256(public_key).hexdigest() != fingerprint:
        raise ValueError("enrollment public-key fingerprint is invalid")
    return Enrollment(key_id, public_key, expected)


def production_private_key(key_id: str) -> Path:
    root = (
        key_ceremony.secure_home()
        / ".local"
        / "share"
        / "hardened-blikvm"
        / "production-signing"
    )
    key_ceremony.ensure_private_directory(root, create=False)
    directory = root / key_ceremony.validate_key_id(key_id)
    key_ceremony.ensure_private_directory(directory, create=False)
    path = directory / key_ceremony.PRIVATE_KEY_NAME
    metadata = path.lstat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or path.is_symlink()
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or stat.S_IMODE(metadata.st_mode) != 0o400
        or not 0 < metadata.st_size <= PRIVATE_KEY_MAX_BYTES
    ):
        raise ValueError("production private-key metadata is unsafe")
    return path


def _bundle_path(package_path: PurePosixPath) -> str | None:
    if package_path in RUNTIME_BINARY_MAPPING:
        return RUNTIME_BINARY_MAPPING[package_path]
    if WEB_PREFIX in package_path.parents:
        relative = package_path.relative_to(WEB_PREFIX)
        return str(PurePosixPath("share/web") / relative)
    return None


def extract_application_payload(
    archive_bytes: bytes, package: package_gate.PackageInfo
) -> dict[str, tuple[int, bytes]]:
    files: dict[str, tuple[int, bytes]] = {}
    observed_package_paths: set[str] = set()
    total_bytes = 0
    with tarfile.open(fileobj=io.BytesIO(archive_bytes), mode="r:") as archive:
        members = archive.getmembers()
        for member in members:
            if member.name in (".", "./"):
                continue
            package_gate.validate_tar_metadata(member)
            package_path = package_gate.package_member_path(member.name)
            absolute = "/" + str(package_path)
            if absolute in observed_package_paths:
                raise ValueError("runtime package contains a duplicate path")
            observed_package_paths.add(absolute)
            bundle_path = _bundle_path(package_path)
            if bundle_path is None:
                continue
            if member.isdir() and WEB_PREFIX in package_path.parents:
                continue
            if not member.isfile() or member.issym() or member.islnk():
                raise ValueError("selected runtime payload member is not a regular file")
            if bundle_path in files:
                raise ValueError("runtime payload paths collide after mapping")
            expected = package.installed_files.get(absolute)
            if expected is None or expected[1] != member.mode:
                raise ValueError("runtime payload metadata differs from package validation")
            stream = archive.extractfile(member)
            if stream is None:
                raise ValueError("runtime payload member is unreadable")
            content = stream.read(MAX_PAYLOAD_BYTES + 1)
            if len(content) != member.size or hashlib.sha256(content).hexdigest() != expected[0]:
                raise ValueError("runtime payload content differs from package validation")
            total_bytes += len(content)
            if total_bytes > MAX_PAYLOAD_BYTES:
                raise ValueError("runtime payload exceeds the application limit")
            files[bundle_path] = (member.mode, content)
    if len(files) > MAX_PAYLOAD_FILES or not REQUIRED_BUNDLE_PATHS.issubset(files):
        raise ValueError("runtime package cannot provide a complete application payload")
    expected_paths = {
        mapped
        for absolute in package.installed_files
        if (mapped := _bundle_path(PurePosixPath(absolute.removeprefix("/")))) is not None
    }
    if set(files) != expected_paths:
        raise ValueError("runtime payload extraction is incomplete")
    return dict(sorted(files.items()))


def manifest_document(
    options: Options,
    enrollment: Enrollment,
    files: dict[str, tuple[int, bytes]],
) -> dict[str, object]:
    return {
        "format": FORMAT,
        "product": PRODUCT,
        "target": TARGET,
        "architecture": ARCHITECTURE,
        "keyId": enrollment.key_id,
        "version": validate_version(options.version),
        "sequence": options.sequence,
        "minimumSystemSequence": options.minimum_system_sequence,
        "sourceCommit": options.expected_source_commit,
        "files": [
            {
                "path": path,
                "mode": mode,
                "size": len(content),
                "sha256": hashlib.sha256(content).hexdigest(),
            }
            for path, (mode, content) in files.items()
        ],
    }


def canonical_manifest(document: dict[str, object]) -> bytes:
    encoded = json.dumps(document, ensure_ascii=True, separators=(",", ":")).encode("ascii")
    if not encoded or len(encoded) > MAX_MANIFEST_BYTES:
        raise ValueError("application manifest exceeds its size limit")
    return encoded


def _memory_file(name: str, content: bytes) -> int:
    descriptor = os.memfd_create(name, flags=os.MFD_CLOEXEC)
    remaining = memoryview(content)
    while remaining:
        written = os.write(descriptor, remaining)
        if written <= 0:
            os.close(descriptor)
            raise OSError("short write to private memory file")
        remaining = remaining[written:]
    os.lseek(descriptor, 0, os.SEEK_SET)
    return descriptor


def sign_manifest(private_key: Path, manifest: bytes) -> bytes:
    private_flags = os.O_RDONLY | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        private_flags |= os.O_NOFOLLOW
    private_descriptor = os.open(private_key, private_flags)
    manifest_descriptor = _memory_file("hardened-blikvm-manifest", manifest)
    try:
        result = subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-sign",
                "-rawin",
                "-inkey",
                f"/proc/self/fd/{private_descriptor}",
                "-in",
                f"/proc/self/fd/{manifest_descriptor}",
            ],
            check=True,
            stdin=None,
            stdout=subprocess.PIPE,
            pass_fds=(private_descriptor, manifest_descriptor),
        )
    finally:
        os.close(manifest_descriptor)
        os.close(private_descriptor)
    if len(result.stdout) != 64:
        raise ValueError("OpenSSL returned an invalid Ed25519 signature")
    return result.stdout


def _public_pem(public_key: bytes) -> bytes:
    der = key_ceremony.SPKI_PREFIX + public_key
    encoded = base64.b64encode(der).decode("ascii")
    return (
        "-----BEGIN PUBLIC KEY-----\n"
        + "\n".join(encoded[index : index + 64] for index in range(0, len(encoded), 64))
        + "\n-----END PUBLIC KEY-----\n"
    ).encode("ascii")


def verify_signature(public_key: bytes, manifest: bytes, signature: bytes) -> None:
    public_descriptor = _memory_file("hardened-blikvm-public-key", _public_pem(public_key))
    manifest_descriptor = _memory_file("hardened-blikvm-manifest", manifest)
    signature_descriptor = _memory_file("hardened-blikvm-signature", signature)
    try:
        result = subprocess.run(
            [
                "openssl",
                "pkeyutl",
                "-verify",
                "-rawin",
                "-pubin",
                "-inkey",
                f"/proc/self/fd/{public_descriptor}",
                "-in",
                f"/proc/self/fd/{manifest_descriptor}",
                "-sigfile",
                f"/proc/self/fd/{signature_descriptor}",
            ],
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            pass_fds=(public_descriptor, manifest_descriptor, signature_descriptor),
        )
    finally:
        os.close(signature_descriptor)
        os.close(manifest_descriptor)
        os.close(public_descriptor)
    if result.returncode != 0:
        raise ValueError("production signature does not match the enrolled public key")


def _directories(files: dict[str, tuple[int, bytes]]) -> set[str]:
    directories = {"payload"}
    for relative in files:
        current = PurePosixPath("payload")
        for component in PurePosixPath(relative).parts[:-1]:
            current /= component
            directories.add(str(current))
    return directories


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
    manifest: bytes, signature: bytes, files: dict[str, tuple[int, bytes]]
) -> bytes:
    if len(signature) != 64:
        raise ValueError("Ed25519 signature must contain exactly 64 bytes")
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
    bundle = output.getvalue()
    if not bundle or len(bundle) > MAX_BUNDLE_BYTES:
        raise ValueError("application bundle exceeds its size limit")
    return bundle


def write_new(path: Path, content: bytes) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    descriptor = os.open(path, flags, 0o600)
    try:
        with os.fdopen(descriptor, "wb", closefd=False) as stream:
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
    finally:
        os.close(descriptor)


def execute(options: Options) -> int:
    if not sys.stdin.isatty() or not sys.stderr.isatty():
        raise ValueError("run this builder in an interactive local terminal")
    if options.sequence <= 0 or options.minimum_system_sequence <= 0:
        raise ValueError("application sequences must be positive")
    output = validate_output_directory(options.output_dir)
    enrollment = load_enrollment(
        options.enrollment_request,
        options.expected_enrollment_request_sha256,
        options.key_id,
    )
    private_key = production_private_key(enrollment.key_id)
    package = package_gate.validate_package(
        options.package,
        options.expected_package_sha256,
        options.expected_source_commit,
        options.expected_source_bundle_sha256,
        options.expected_builder_commit,
        expected_version="0.1.0~dev2",
    )
    archive_bytes = package_gate.read_dpkg_deb_archive(package.path, "--fsys-tarfile")
    files = extract_application_payload(archive_bytes, package)
    manifest = canonical_manifest(manifest_document(options, enrollment, files))
    print("Enter the production key passphrase once to sign the exact manifest.", flush=True)
    signature = sign_manifest(private_key, manifest)
    verify_signature(enrollment.public_key, manifest, signature)
    bundle = build_bundle(manifest, signature, files)

    output.mkdir(mode=0o700)
    artifact = f"hardened-blikvm-application-{options.version}-seq{options.sequence}.tar"
    write_new(output / artifact, bundle)
    receipt: BundleReceipt = {
        "schemaVersion": 1,
        "artifact": artifact,
        "bundleSha256": hashlib.sha256(bundle).hexdigest(),
        "bundleBytes": len(bundle),
        "manifestSha256": hashlib.sha256(manifest).hexdigest(),
        "payloadBytes": sum(len(content) for _, content in files.values()),
        "payloadFiles": len(files),
        "keyId": enrollment.key_id,
        "publicKeySha256": hashlib.sha256(enrollment.public_key).hexdigest(),
        "enrollmentRequestSha256": enrollment.request_sha256,
        "packageSha256": package.sha256,
        "sourceCommit": package.source_commit,
        "version": options.version,
        "sequence": options.sequence,
        "minimumSystemSequence": options.minimum_system_sequence,
        "privateKeyIncluded": False,
        "ownerOnlySnapshotRequired": True,
    }
    write_new(
        output / "bundle.json",
        json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode("ascii") + b"\n",
    )
    directory = os.open(output, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (
        json.JSONDecodeError,
        OSError,
        subprocess.SubprocessError,
        tarfile.TarError,
        UnicodeError,
        ValueError,
    ) as error:
        print(f"production bundle build failed: {type(error).__name__}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
