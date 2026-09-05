#!/usr/bin/env python3
"""Build a deterministic inert application-update runtime Debian package."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_debian_runtime_package as runtime_package
from tools import stage_device_prototype as prototype


PACKAGE_NAME = "hardened-blikvm-update-runtime"
PACKAGE_VERSION = "0.1.0~dev7"
SUPPORTED_PACKAGE_VERSIONS = ("0.1.0~dev1", "0.1.0~dev2", "0.1.0~dev3", "0.1.0~dev4", "0.1.0~dev5", "0.1.0~dev6", PACKAGE_VERSION)
PACKAGE_ARCHITECTURE = "arm64"
PACKAGE_FILENAME = f"{PACKAGE_NAME}_{PACKAGE_VERSION}_{PACKAGE_ARCHITECTURE}.deb"
RUNTIME_DEPENDENCY = (
    f"{runtime_package.PACKAGE_NAME} (= {runtime_package.PACKAGE_VERSION})"
)
FILE_MAP = {
    "share/systemd/hardened-blikvm-update-status.socket": (
        "lib/systemd/system/hardened-blikvm-update-status.socket"
    ),
    "share/systemd/hardened-blikvm-update-status@.service": (
        "lib/systemd/system/hardened-blikvm-update-status@.service"
    ),
    "share/systemd/hardened-blikvm-update-recover.service": (
        "lib/systemd/system/hardened-blikvm-update-recover.service"
    ),
    "libexec/bli-kvm-update-supervisor": (
        "usr/libexec/hardened-blikvm/bli-kvm-update-supervisor"
    ),
    "libexec/bli-kvm-update-health-reporter": (
        "usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter"
    ),
    "share/systemd/hardened-blikvm-update-watchdog.service": (
        "lib/systemd/system/hardened-blikvm-update-watchdog.service"
    ),
    "share/systemd/hardened-blikvm-update-watchdog.timer": (
        "lib/systemd/system/hardened-blikvm-update-watchdog.timer"
    ),
    "share/systemd/hardened-blikvm-update-confirm.socket": (
        "lib/systemd/system/hardened-blikvm-update-confirm.socket"
    ),
    "share/systemd/hardened-blikvm-update-confirm@.service": (
        "lib/systemd/system/hardened-blikvm-update-confirm@.service"
    ),
    "share/systemd/hardened-blikvm-update-activate@.service": (
        "lib/systemd/system/hardened-blikvm-update-activate@.service"
    ),
    "share/systemd/hardened-blikvm-update-apply-candidate.service": (
        "lib/systemd/system/hardened-blikvm-update-apply-candidate.service"
    ),
    "share/systemd/hardened-blikvm-update-apply-confirmed.service": (
        "lib/systemd/system/hardened-blikvm-update-apply-confirmed.service"
    ),
    "share/systemd/hardened-blikvm-server-current.service": (
        "lib/systemd/system/hardened-blikvm-server-current.service"
    ),
    "share/systemd/hardened-blikvm-server-candidate.service": (
        "lib/systemd/system/hardened-blikvm-server-candidate.service"
    ),
    "share/tmpfiles.d/hardened-blikvm-update.conf": (
        "usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-update-tmpfiles.conf"
    ),
    "share/sysusers.d/hardened-blikvm.conf": (
        "usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-sysusers.conf"
    ),
    "share/docs/application-update-v1.md": (
        "usr/share/doc/hardened-blikvm-update-runtime/application-update-v1.md"
    ),
    "share/docs/application-activation-v1.md": (
        "usr/share/doc/hardened-blikvm-update-runtime/application-activation-v1.md"
    ),
}
EXECUTABLE_DESTINATIONS = {
    "usr/libexec/hardened-blikvm/bli-kvm-update-supervisor",
    "usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter",
}
ACTIVE_CONFIGURATION_PREFIXES = (
    "etc/",
    "usr/lib/sysusers.d/",
    "usr/lib/tmpfiles.d/",
    "lib/tmpfiles.d/",
)


class BuildError(Exception):
    pass


def file_map(version: str) -> dict[str, str]:
    if version not in SUPPORTED_PACKAGE_VERSIONS:
        raise BuildError("unsupported update package version")
    revision = SUPPORTED_PACKAGE_VERSIONS.index(version) + 1
    introduced = {
        "hardened-blikvm-update-recover.service": 5,
        "hardened-blikvm-update-status.socket": 6,
        "hardened-blikvm-update-status@.service": 6,
    }
    return {source: destination for source, destination in FILE_MAP.items()
            if revision >= introduced.get(source.rsplit("/", 1)[-1], 1)}


def parse_args() -> tuple[str, str, str]:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output-dir", required=True)
    namespace = parser.parse_args()
    return (
        cast(str, namespace.bundle),
        cast(str, namespace.expected_sha256),
        cast(str, namespace.output_dir),
    )


def package_manifest(
    bundle: prototype.BundleInfo, builder_commit: str, *, version: str = PACKAGE_VERSION
) -> dict[str, object]:
    if version not in SUPPORTED_PACKAGE_VERSIONS:
        raise BuildError("unsupported update package version")
    return {
        "schemaVersion": 1,
        "package": PACKAGE_NAME,
        "version": version,
        "architecture": PACKAGE_ARCHITECTURE,
        "runtimeDependency": RUNTIME_DEPENDENCY,
        "sourceCommit": bundle.source_commit,
        "sourceBundleSha256": bundle.sha256,
        "builderCommit": builder_commit,
        "scope": "inert-application-update-runtime",
        "autoStart": False,
        "unitsEnabled": False,
        "trustConfigured": False,
        "privateKeyIncluded": False,
        "configurationIncluded": False,
        "stateInitialized": False,
        "slotRootCreated": False,
        "httpRouteIncluded": False,
        "guiRouteIncluded": False,
        "systemUpdateIncluded": False,
        "hardwareMutationIncluded": False,
    }


def audit_package_tree(root: Path) -> None:
    files = [path for path in root.rglob("*") if path.is_file()]
    relative_files = {str(path.relative_to(root)) for path in files}
    expected = {"DEBIAN/control", *FILE_MAP.values()}
    expected.add(
        "usr/share/doc/hardened-blikvm-update-runtime/PACKAGE-MANIFEST.json"
    )
    if relative_files != expected:
        raise BuildError("update package file tree is not exact")
    for relative in relative_files:
        if relative.startswith(ACTIVE_CONFIGURATION_PREFIXES):
            raise BuildError("update package entered an active configuration path")
    control_root = root / "DEBIAN"
    if {path.name for path in control_root.iterdir()} != {"control"}:
        raise BuildError("update package contains a maintainer script")
    forbidden = {
        "private-key": b"BEGIN " + b"PRIVATE" + b" KEY",
        "host-home": b"/home/" + b"w0w",
        "host-mount": b"/mnt/" + b"d",
        "network-prefix": b"10." + b"0.",
    }
    for path in files:
        if path.stat().st_size > 8 * 1024 * 1024:
            continue
        payload = path.read_bytes()
        for name, marker in forbidden.items():
            if marker in payload:
                raise BuildError(f"{name} content entered update package")


def build_package(bundle_value: str, expected_sha256: str, output_value: str) -> Path:
    output_dir = runtime_package.validate_output_dir(output_value)
    expected = prototype.validate_sha256(expected_sha256)
    bundle = prototype.validate_bundle(bundle_value, expected)
    builder_commit = runtime_package.clean_builder_commit()
    dpkg_deb = shutil.which("dpkg-deb")
    if not dpkg_deb:
        raise BuildError("dpkg-deb is unavailable")
    output_dir.mkdir(mode=0o755, parents=True)
    output_dir.chmod(0o755)
    package_path = output_dir / PACKAGE_FILENAME
    with tempfile.TemporaryDirectory(
        prefix="hardened-blikvm-update-deb.", dir="/tmp"
    ) as temporary:
        package_root = Path(temporary) / "package"
        package_root.mkdir(mode=0o755)
        with tarfile.open(bundle.path, mode="r:gz") as archive:
            for source, destination in FILE_MAP.items():
                runtime_package.extract_regular_member(
                    archive, source, package_root / destination
                )
        control = "\n".join(
            (
                f"Package: {PACKAGE_NAME}",
                f"Version: {PACKAGE_VERSION}",
                "Section: admin",
                "Priority: optional",
                f"Architecture: {PACKAGE_ARCHITECTURE}",
                "Maintainer: Hardened BliKVM Project <2505149+woffko@users.noreply.github.com>",
                f"Depends: systemd (>= 249), {RUNTIME_DEPENDENCY}",
                "Description: inert Hardened BliKVM application update runtime",
                " Installs disabled update binaries and systemd templates only.",
                " It contains no trust key, active configuration, state, slot, API, or GUI route.",
                "",
            )
        )
        runtime_package.write_text(package_root / "DEBIAN/control", control)
        runtime_package.write_text(
            package_root
            / "usr/share/doc/hardened-blikvm-update-runtime/PACKAGE-MANIFEST.json",
            json.dumps(
                package_manifest(bundle, builder_commit),
                sort_keys=True,
                separators=(",", ":"),
            )
            + "\n",
        )
        runtime_package.normalize_tree(package_root, EXECUTABLE_DESTINATIONS)
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
            raise BuildError("dpkg-deb rejected the deterministic update package")
        temporary_package.replace(package_path)
        package_path.chmod(0o644)
    digest = hashlib.sha256(package_path.read_bytes()).hexdigest()
    runtime_package.write_text(
        output_dir / f"{PACKAGE_FILENAME}.sha256",
        f"{digest}  {PACKAGE_FILENAME}\n",
    )
    return package_path


def main() -> int:
    try:
        bundle, expected, output = parse_args()
        package = build_package(bundle, expected, output)
        print(f"{hashlib.sha256(package.read_bytes()).hexdigest()}  {package}")
        return 0
    except (
        BuildError,
        runtime_package.BuildError,
        OSError,
        subprocess.SubprocessError,
        tarfile.TarError,
        ValueError,
    ):
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
