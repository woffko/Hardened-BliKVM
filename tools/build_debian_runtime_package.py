#!/usr/bin/env python3
"""Build a deterministic, inert Debian package from one validated prototype bundle."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
import tempfile
from typing import BinaryIO, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype


PACKAGE_NAME = "hardened-blikvm-runtime"
PACKAGE_VERSION = "0.1.0~dev1"
PACKAGE_ARCHITECTURE = "arm64"
PACKAGE_FILENAME = f"{PACKAGE_NAME}_{PACKAGE_VERSION}_{PACKAGE_ARCHITECTURE}.deb"
BUNDLE_ROOT = "hardened-blikvm-prototype-0.1.0-dev-aarch64"

FILE_MAP = {
    "bin/bli-kvm-server": "usr/libexec/hardened-blikvm/bli-kvm-server",
    "libexec/bli-kvm-platform-broker": "usr/libexec/hardened-blikvm/bli-kvm-platform-broker",
    "libexec/bli-kvm-video-proxy": "usr/libexec/hardened-blikvm/bli-kvm-video-proxy",
    "libexec/bli-kvm-control-broker": "usr/libexec/hardened-blikvm/bli-kvm-control-broker",
    "share/systemd/hardened-blikvm-platform-broker.service":
        "lib/systemd/system/hardened-blikvm-platform-broker.service",
    "share/systemd/hardened-blikvm-video-proxy.service":
        "lib/systemd/system/hardened-blikvm-video-proxy.service",
    "share/systemd/hardened-blikvm-control-broker.service":
        "lib/systemd/system/hardened-blikvm-control-broker.service",
    "share/systemd/hardened-blikvm-server.service":
        "lib/systemd/system/hardened-blikvm-server.service",
    "share/sysusers.d/hardened-blikvm.conf":
        "usr/lib/sysusers.d/hardened-blikvm.conf",
    "LICENSE": "usr/share/doc/hardened-blikvm-runtime/copyright",
    "README.md": "usr/share/doc/hardened-blikvm-runtime/README.md",
    "share/docs/packaging-boundary.md":
        "usr/share/doc/hardened-blikvm-runtime/packaging-boundary.md",
    "share/docs/security-boundaries.md":
        "usr/share/doc/hardened-blikvm-runtime/security-boundaries.md",
    "share/nginx/hardened-blikvm.conf":
        "usr/share/doc/hardened-blikvm-runtime/examples/hardened-blikvm-nginx.conf",
}

EXECUTABLE_DESTINATIONS = {
    "usr/libexec/hardened-blikvm/bli-kvm-server",
    "usr/libexec/hardened-blikvm/bli-kvm-platform-broker",
    "usr/libexec/hardened-blikvm/bli-kvm-video-proxy",
    "usr/libexec/hardened-blikvm/bli-kvm-control-broker",
}

FORBIDDEN_PACKAGE_NAMES = {
    "bli-kvm-update-supervisor",
    "bli-kvm-update-health-reporter",
    "hardened-blikvm-update-watchdog.service",
    "hardened-blikvm-update-watchdog.timer",
    "hardened-blikvm-update-confirm.socket",
    "hardened-blikvm-diagnostic-collector.service",
    "hardened-blikvm-diagnostic-persistence-gate.service",
    "hardened-blikvm-diagnostic-store-init.service",
    "hardened-blikvm-system-action-store-init.service",
}


class BuildError(Exception):
    pass


def parse_args() -> tuple[str, str, str]:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--output-dir", required=True)
    namespace = parser.parse_args()
    return (
        cast(str, namespace.bundle),
        cast(str, namespace.expected_sha256),
        cast(str, namespace.output_dir),
    )


def validate_output_dir(value: str) -> Path:
    output = Path(value)
    if not output.is_absolute():
        raise ValueError("package output directory must be absolute")
    if output.exists():
        if output.is_symlink() or not output.is_dir() or any(output.iterdir()):
            raise ValueError("package output directory must be absent or empty")
    return output


def package_manifest(bundle: prototype.BundleInfo) -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "package": PACKAGE_NAME,
        "version": PACKAGE_VERSION,
        "architecture": PACKAGE_ARCHITECTURE,
        "sourceCommit": bundle.source_commit,
        "sourceBundleSha256": bundle.sha256,
        "scope": "inert-read-only-runtime",
        "autoStart": False,
        "credentialsIncluded": False,
        "nginxConfigActive": False,
        "updateRuntimeIncluded": False,
        "diagnosticPersistenceIncluded": False,
        "hardwareMutationIncluded": False,
    }


def _extract_regular_member(archive: tarfile.TarFile, source: str, destination: Path) -> None:
    member = archive.getmember(f"{BUNDLE_ROOT}/{source}")
    if not member.isfile() or member.issym() or member.islnk():
        raise BuildError(f"package source member is not a regular file: {source}")
    stream = archive.extractfile(member)
    if stream is None:
        raise BuildError(f"package source member is unreadable: {source}")
    destination.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
    with destination.open("xb") as output:
        shutil.copyfileobj(cast(BinaryIO, stream), output, length=1024 * 1024)


def _extract_web_tree(archive: tarfile.TarFile, package_root: Path) -> None:
    prefix = f"{BUNDLE_ROOT}/share/web/"
    destination_root = package_root / "usr/share/hardened-blikvm/web"
    found_index = False
    for member in archive.getmembers():
        if not member.name.startswith(prefix):
            continue
        relative = PurePosixPath(member.name.removeprefix(prefix))
        if not relative.parts or relative.is_absolute() or ".." in relative.parts:
            raise BuildError("web member path is unsafe")
        destination = destination_root.joinpath(*relative.parts)
        if member.isdir():
            destination.mkdir(mode=0o755, parents=True, exist_ok=True)
            continue
        if not member.isfile() or member.issym() or member.islnk():
            raise BuildError("web tree contains an unsupported member")
        stream = archive.extractfile(member)
        if stream is None:
            raise BuildError("web member is unreadable")
        destination.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
        with destination.open("xb") as output:
            shutil.copyfileobj(cast(BinaryIO, stream), output, length=1024 * 1024)
        if relative == PurePosixPath("index.html"):
            found_index = True
    if not found_index:
        raise BuildError("web tree is missing index.html")


def _write_text(path: Path, value: str, mode: int = 0o644) -> None:
    path.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
    path.write_text(value, encoding="utf-8", newline="\n")
    path.chmod(mode)


def _normalize_tree(root: Path) -> None:
    for path in sorted(root.rglob("*"), key=lambda item: (len(item.parts), str(item))):
        if path.is_symlink():
            raise BuildError("package tree contains a symlink")
        path.chmod(0o755 if path.is_dir() or str(path.relative_to(root)) in EXECUTABLE_DESTINATIONS else 0o644)
    for path in sorted(root.rglob("*"), reverse=True):
        os.utime(path, (0, 0), follow_symlinks=False)
    os.utime(root, (0, 0), follow_symlinks=False)


def audit_package_tree(root: Path) -> None:
    names = {path.name for path in root.rglob("*")}
    if names & FORBIDDEN_PACKAGE_NAMES:
        raise BuildError("excluded update, persistence, or system-action artifact entered the package")
    forbidden_bytes = {
        "network-prefix": b"10." + b"0.",
        "host-home": b"/home/" + b"w0w",
        "host-mount": b"/mnt/" + b"d",
        "private-key": b"BEGIN " + b"PRIVATE" + b" KEY",
    }
    for path in root.rglob("*"):
        if path.is_file() and path.stat().st_size <= 8 * 1024 * 1024:
            payload = path.read_bytes()
            for marker_class, marker in forbidden_bytes.items():
                if marker in payload:
                    relative = path.relative_to(root)
                    raise BuildError(
                        f"{marker_class} content entered package path {relative}"
                    )
    web_root = root / "usr/share/hardened-blikvm/web"
    for path in web_root.rglob("*"):
        if path.is_file():
            payload = path.read_bytes()
            if b"NanoKVM" in payload or b"Hardened_NanoKVM" in payload:
                raise BuildError("legacy user-visible branding entered the package")


def build_package(bundle_value: str, expected_sha256: str, output_value: str) -> Path:
    output_dir = validate_output_dir(output_value)
    expected = prototype.validate_sha256(expected_sha256)
    bundle = prototype.validate_bundle(bundle_value, expected)
    dpkg_deb = shutil.which("dpkg-deb")
    if not dpkg_deb:
        raise BuildError("dpkg-deb is unavailable")

    output_dir.mkdir(mode=0o755, parents=True, exist_ok=True)
    output_dir.chmod(0o755)
    package_path = output_dir / PACKAGE_FILENAME
    with tempfile.TemporaryDirectory(prefix="hardened-blikvm-deb.", dir="/tmp") as temporary:
        package_root = Path(temporary) / "package"
        package_root.mkdir(mode=0o755)
        with tarfile.open(bundle.path, mode="r:gz") as archive:
            for source, destination in FILE_MAP.items():
                _extract_regular_member(archive, source, package_root / destination)
            _extract_web_tree(archive, package_root)

        control = "\n".join(
            (
                f"Package: {PACKAGE_NAME}",
                f"Version: {PACKAGE_VERSION}",
                "Section: admin",
                "Priority: optional",
                f"Architecture: {PACKAGE_ARCHITECTURE}",
                "Maintainer: Hardened BliKVM Project <2505149+woffko@users.noreply.github.com>",
                "Depends: systemd (>= 249)",
                "Recommends: nginx",
                "Description: inert read-only Hardened BliKVM runtime",
                " Installs disabled service templates, static Rust binaries, and the web tree.",
                " It contains no credential, active Nginx configuration, update trust, or mutation backend.",
                "",
            )
        )
        _write_text(package_root / "DEBIAN/control", control)
        _write_text(
            package_root / "usr/share/doc/hardened-blikvm-runtime/PACKAGE-MANIFEST.json",
            json.dumps(package_manifest(bundle), sort_keys=True, separators=(",", ":")) + "\n",
        )
        _normalize_tree(package_root)
        audit_package_tree(package_root)

        environment = os.environ.copy()
        environment.update({"SOURCE_DATE_EPOCH": "0", "TZ": "UTC", "LC_ALL": "C"})
        temporary_package = output_dir / f".{PACKAGE_FILENAME}.part"
        result = subprocess.run(
            [
                dpkg_deb,
                "--build",
                "--root-owner-group",
                "--uniform-compression",
                "-Zxz",
                "-z9",
                str(package_root),
                str(temporary_package),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=environment,
            check=False,
        )
        if result.returncode != 0:
            raise BuildError("dpkg-deb rejected the deterministic package tree")
        temporary_package.replace(package_path)
        package_path.chmod(0o644)

    digest = hashlib.sha256(package_path.read_bytes()).hexdigest()
    _write_text(output_dir / f"{PACKAGE_FILENAME}.sha256", f"{digest}  {PACKAGE_FILENAME}\n")
    return package_path


def main() -> int:
    try:
        bundle, expected, output = parse_args()
        package = build_package(bundle, expected, output)
        digest = hashlib.sha256(package.read_bytes()).hexdigest()
        print(f"{digest}  {package}")
        return 0
    except (BuildError, OSError, subprocess.SubprocessError, tarfile.TarError, ValueError):
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
