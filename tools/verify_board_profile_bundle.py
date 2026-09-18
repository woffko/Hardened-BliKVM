#!/usr/bin/env python3
"""Verify the board-owner test bundle before it is staged on the board.

Checks the archive members are safe and complete, the MANIFEST declares the
board-owner profile and its features, and SHA256SUMS matches the packaged files.
Writes a sanitized receipt; it never prints file contents or secrets.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import stat
import sys
import tarfile
from pathlib import Path, PurePosixPath

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


EXPECTED_ROOT = "hardened-blikvm-board-profile-aarch64"
EXPECTED_PROFILE = "board-owner-test"
EXPECTED_FEATURES = [
    "restricted-control-surface",
    "gadget-media-profile",
    "gadget-hid-profile",
    "chardev-atx-profile",
    "system-action-profile",
    "network-write-profile",
]
REQUIRED_MEMBERS = {
    "MANIFEST.json",
    "SHA256SUMS",
    "libexec/bli-kvm-server",
    "libexec/bli-kvm-control-broker",
    "libexec/bli-kvm-boardd",
    "libexec/bli-kvm-video-proxy",
    "libexec/bli-kvm-atx-gpio-bench-gate",
    "share/systemd/hardened-blikvm-board-owner.service",
    "share/systemd/hardened-blikvm-board-server.service",
    "share/systemd/hardened-blikvm-board-video-proxy.service",
    "share/systemd/hardened-blikvm-ustreamer.service",
    "share/systemd/hardened-blikvm-boardd.service",
    "share/systemd/hardened-blikvm-msd-writable.service",
    "share/systemd/systemd-journald.service.d/99-hardened-blikvm-persistent-journal.conf",
    "share/systemd/hardened-blikvm-board-owner.service.d/90-socket-group.conf",
    "share/tmpfiles.d/hardened-blikvm-gadget.conf",
    "share/tmpfiles.d/hardened-blikvm-video.conf",
    "share/udev/99-hardened-blikvm-video.rules",
    "share/atx.env.example",
    "share/boardd.json.example",
    "share/web/index.html",
}
MAX_MEMBERS = 4096
MAX_UNCOMPRESSED_BYTES = 512 * 1024 * 1024
MAX_MANIFEST_BYTES = 64 * 1024
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
SHA_LINE = re.compile(r"^([0-9a-f]{64})  (.+)$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--expected-sha256", default="")
    return parser.parse_args()


def _member_name(name: str) -> str:
    path = PurePosixPath(name)
    if path.is_absolute() or any(part in ("..", "") for part in path.parts):
        raise ValueError("unsafe bundle member")
    if path.parts and path.parts[0] != EXPECTED_ROOT:
        raise ValueError("unexpected bundle root")
    return str(path.relative_to(EXPECTED_ROOT))


def verify_bundle(bundle_path: Path, expected_sha256: str) -> dict[str, object]:
    digest = hashlib.sha256()
    with bundle_path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    actual_sha256 = digest.hexdigest()
    if expected_sha256 and actual_sha256 != expected_sha256:
        raise ValueError("bundle checksum does not match")

    members: dict[str, str] = {}
    total = 0
    with tarfile.open(bundle_path, "r:gz") as archive:
        names = archive.getnames()
        if len(names) > MAX_MEMBERS:
            raise ValueError("bundle has too many members")
        files: dict[str, bytes] = {}
        for member in archive.getmembers():
            name = _member_name(member.name)
            if member.isdir():
                continue
            if not member.isfile():
                raise ValueError("bundle contains a non-regular member")
            total += member.size
            if total > MAX_UNCOMPRESSED_BYTES:
                raise ValueError("bundle exceeds the size limit")
            extracted = archive.extractfile(member)
            if extracted is None:
                raise ValueError("bundle member could not be read")
            files[name] = extracted.read()

    if set(REQUIRED_MEMBERS) - set(files):
        raise ValueError("bundle is missing required members")

    manifest_bytes = files["MANIFEST.json"]
    if len(manifest_bytes) > MAX_MANIFEST_BYTES:
        raise ValueError("manifest is too large")
    manifest = json.loads(manifest_bytes.decode("utf-8"))
    if manifest.get("profile") != EXPECTED_PROFILE:
        raise ValueError("unexpected bundle profile")
    if manifest.get("cargoFeatures") != EXPECTED_FEATURES:
        raise ValueError("unexpected cargo features")
    if manifest.get("releaseAccepted") is not False or manifest.get("deviceAccepted") is not False:
        raise ValueError("test bundle must not claim acceptance")

    for line in files["SHA256SUMS"].decode("utf-8").splitlines():
        match = SHA_LINE.fullmatch(line)
        if match is None:
            raise ValueError("malformed SHA256SUMS entry")
        recorded, name = match.groups()
        name = name.lstrip("./")
        if name not in files:
            raise ValueError("SHA256SUMS references an unknown member")
        if hashlib.sha256(files[name]).hexdigest() != recorded:
            raise ValueError("SHA256SUMS does not match a member")
        members[name] = recorded

    return {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-board-profile-bundle",
        "sha256": actual_sha256,
        "profile": EXPECTED_PROFILE,
        "cargoFeatures": EXPECTED_FEATURES,
        "memberCount": len(members),
        "members": sorted(members),
        "mutationsPerformed": False,
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    bundle = Path(args.bundle).resolve(strict=True)
    metadata = bundle.lstat()
    if not stat.S_ISREG(metadata.st_mode) or Path(args.bundle).is_symlink():
        raise ValueError("bundle must be a regular non-symlink file")
    expected = args.expected_sha256
    if expected and not HEX_SHA256.fullmatch(expected):
        raise ValueError("expected sha256 must be 64 lowercase hex characters")
    receipt = verify_bundle(bundle, expected)
    write_json_0600_atomic(output, receipt)
    print(f"board_bundle_ok={output} sha256={receipt['sha256']} members={receipt['memberCount']}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as error:
        _ = error
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
