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
from tools import ve_module


PACKAGE_NAME = "hardened-blikvm-update-runtime"
PACKAGE_VERSION = "0.1.0~dev43"
SUPPORTED_PACKAGE_VERSIONS = ("0.1.0~dev1", "0.1.0~dev2", "0.1.0~dev3", "0.1.0~dev4", "0.1.0~dev5", "0.1.0~dev6", "0.1.0~dev7", "0.1.0~dev8", "0.1.0~dev9", "0.1.0~dev10", "0.1.0~dev11", "0.1.0~dev12", "0.1.0~dev13", "0.1.0~dev14", "0.1.0~dev15", "0.1.0~dev16", "0.1.0~dev17", "0.1.0~dev18", "0.1.0~dev19", "0.1.0~dev20", "0.1.0~dev21", "0.1.0~dev22", "0.1.0~dev23", "0.1.0~dev24", "0.1.0~dev25", "0.1.0~dev26", "0.1.0~dev27", "0.1.0~dev28", "0.1.0~dev29", "0.1.0~dev33", "0.1.0~dev34", "0.1.0~dev35", "0.1.0~dev36", "0.1.0~dev37", "0.1.0~dev38", "0.1.0~dev39", "0.1.0~dev40", "0.1.0~dev41", "0.1.0~dev42", PACKAGE_VERSION)
PACKAGE_ARCHITECTURE = "arm64"
PACKAGE_FILENAME = f"{PACKAGE_NAME}_{PACKAGE_VERSION}_{PACKAGE_ARCHITECTURE}.deb"
RUNTIME_DEPENDENCY = (
    f"{runtime_package.PACKAGE_NAME} (= {runtime_package.PACKAGE_VERSION})"
)
FILE_MAP = {
    "share/systemd/hardened-blikvm-update-activation.socket": (
        "lib/systemd/system/hardened-blikvm-update-activation.socket"
    ),
    "share/systemd/hardened-blikvm-update-activation@.service": (
        "lib/systemd/system/hardened-blikvm-update-activation@.service"
    ),
    "share/systemd/hardened-blikvm-update-upload.socket": (
        "lib/systemd/system/hardened-blikvm-update-upload.socket"
    ),
    "share/systemd/hardened-blikvm-update-upload@.service": (
        "lib/systemd/system/hardened-blikvm-update-upload@.service"
    ),
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
# Native profile (dev11): the root helper and native boot units are installed
# but not enabled; managed-unit replacements are examples only, because they
# would otherwise shadow the runtime package's base units.
FILE_MAP.update({
    "libexec/bli-kvm-update-exec-helper": (
        "usr/libexec/hardened-blikvm/bli-kvm-update-exec-helper"
    ),
    "share/systemd/hardened-blikvm-update-exec.socket": (
        "lib/systemd/system/hardened-blikvm-update-exec.socket"
    ),
    "share/systemd/hardened-blikvm-update-exec@.service": (
        "lib/systemd/system/hardened-blikvm-update-exec@.service"
    ),
    "share/systemd/hardened-blikvm-update-native-boot.service": (
        "lib/systemd/system/hardened-blikvm-update-native-boot.service"
    ),
    # dev20: root administration helper, used only while switched on.
    "libexec/bli-kvm-admin-helper": (
        "usr/libexec/hardened-blikvm/bli-kvm-admin-helper"
    ),
    "share/systemd/hardened-blikvm-admin.socket": (
        "lib/systemd/system/hardened-blikvm-admin.socket"
    ),
    "share/systemd/hardened-blikvm-admin@.service": (
        "lib/systemd/system/hardened-blikvm-admin@.service"
    ),
    # dev23: unprivileged remote image download, used only while switched on.
    "share/systemd/hardened-blikvm-image-download.socket": (
        "lib/systemd/system/hardened-blikvm-image-download.socket"
    ),
    "share/systemd/hardened-blikvm-image-download@.service": (
        "lib/systemd/system/hardened-blikvm-image-download@.service"
    ),
    # dev24: PicoClaw runtime and configuration helper, default off.
    "share/systemd/hardened-blikvm-picoclaw.service": (
        "lib/systemd/system/hardened-blikvm-picoclaw.service"
    ),
    "share/systemd/hardened-blikvm-picoclaw-api.socket": (
        "lib/systemd/system/hardened-blikvm-picoclaw-api.socket"
    ),
    "share/systemd/hardened-blikvm-picoclaw-api@.service": (
        "lib/systemd/system/hardened-blikvm-picoclaw-api@.service"
    ),
    "share/picoclaw/AGENT.md": "usr/share/hardened-blikvm/picoclaw/AGENT.md",
    "share/picoclaw/AGENT_KVM.md": "usr/share/hardened-blikvm/picoclaw/AGENT_KVM.md",
    # dev25: Tailscale's apt repository key for the admin helper's install.
    "share/tailscale/tailscale-archive-keyring.asc": (
        "usr/share/hardened-blikvm/tailscale/tailscale-archive-keyring.asc"
    ),
    # dev26: RAW system update step, trust root and first-boot report; the
    # initramfs files take effect only when the initramfs is next built.
    "libexec/bli-kvm-system-update": (
        "usr/libexec/hardened-blikvm/bli-kvm-system-update"
    ),
    "share/systemd/hardened-blikvm-system-update-booted.service": (
        "lib/systemd/system/hardened-blikvm-system-update-booted.service"
    ),
    "share/system-update/system-trust.json": (
        "usr/share/hardened-blikvm/system-update/system-trust.json"
    ),
    "share/system-update/initramfs-hook": (
        "usr/share/initramfs-tools/hooks/hardened-blikvm-system-update"
    ),
    "share/system-update/initramfs-local-premount": (
        "usr/share/initramfs-tools/scripts/local-premount/hardened-blikvm-system-update"
    ),
    # dev35: the QUIC gateway, installed disabled; the board owner's switch
    # enables it (docs/design/quic-transport-v1.md). Its user has a sysusers
    # fragment of its own: the runtime's identities are a pinned set. dev37
    # carries the gateway of the transport's protocol 2, and dev38 one that
    # leaves a silent page unjudged; both have dev36's files. dev39 carries the
    # fixes of the security review of 2026-10-18, and the download worker's
    # sysusers fragment beside them.
    "libexec/bli-kvm-quic-gateway": (
        "usr/libexec/hardened-blikvm/bli-kvm-quic-gateway"
    ),
    "share/systemd/hardened-blikvm-quic.service": (
        "lib/systemd/system/hardened-blikvm-quic.service"
    ),
    "share/sysusers.d/hardened-blikvm-quic.conf": (
        "usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-quic-sysusers.conf"
    ),
    # dev39: the image download worker's own account, a fragment of its own
    # beside the runtime's pinned identities, applied at installation.
    "share/sysusers.d/hardened-blikvm-download.conf": (
        "usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-download-sysusers.conf"
    ),
    # dev40: PicoClaw's installer, a unit, a socket and an account of their
    # own, so no image download writes a PicoClaw file (code audit of
    # 2026-10-20, AUD-05.2).
    "share/systemd/hardened-blikvm-picoclaw-install.socket": (
        "lib/systemd/system/hardened-blikvm-picoclaw-install.socket"
    ),
    "share/systemd/hardened-blikvm-picoclaw-install@.service": (
        "lib/systemd/system/hardened-blikvm-picoclaw-install@.service"
    ),
    "share/sysusers.d/hardened-blikvm-picoclaw-install.conf": (
        "usr/share/doc/hardened-blikvm-update-runtime/examples/"
        "hardened-blikvm-picoclaw-install-sysusers.conf"
    ),
    # dev42: the system's own nginx configuration, outside the directory a
    # RAW update carries, and the step that puts it back before nginx starts
    # (code audit of 2026-10-20, gate Y).
    "share/nginx/hardened-blikvm.conf": (
        "usr/share/hardened-blikvm/nginx/hardened-blikvm.conf"
    ),
    "share/systemd/hardened-blikvm-nginx.service.d/50-system-config.conf": (
        "lib/systemd/system/hardened-blikvm-nginx.service.d/50-system-config.conf"
    ),
    # dev19: HID gadget functions before the owner (cold-boot HID access).
    "share/systemd/hardened-blikvm-hid-gadget.service": (
        "lib/systemd/system/hardened-blikvm-hid-gadget.service"
    ),
})
# dev36: the encoder driver of the H616's video engine, for exactly this
# kernel release (docs/design/h616-ve-encoder-driver-v1.md). The package
# installs the module, its loader and the loader's unit, disabled; the rule
# that gives the device to the encoder's group, that group, and the line
# that keeps the decoder driver from being loaded are examples, as every
# active configuration of this package is. The module is GPL-2.0-only and
# comes with its source.
VE_KERNEL_RELEASE = "5.19.4-sunxi64"
VE_MODULE_DESTINATION = (
    f"{ve_module.MODULE_ROOT}/{VE_KERNEL_RELEASE}/{ve_module.MODULE_FILE}"
)
VE_MODULE_SOURCE_FILES = (
    "README.md", "ve_enc.c", "ve_enc_abi.h", "ve_enc_plan.c", "ve_enc_plan.h", "ve_hw.c", "ve_hw.h",
)
FILE_MAP.update({
    f"share/ve/{VE_KERNEL_RELEASE}/{ve_module.MODULE_FILE}": VE_MODULE_DESTINATION,
    "libexec/bli-kvm-ve-load": "usr/libexec/hardened-blikvm/bli-kvm-ve-load",
    "share/systemd/hardened-blikvm-ve.service": (
        "lib/systemd/system/hardened-blikvm-ve.service"
    ),
    "share/udev/99-hardened-blikvm-ve.rules": (
        f"usr/share/doc/{PACKAGE_NAME}/examples/99-hardened-blikvm-ve.rules"
    ),
    "share/sysusers.d/hardened-blikvm-ve.conf": (
        f"usr/share/doc/{PACKAGE_NAME}/examples/hardened-blikvm-ve-sysusers.conf"
    ),
    "share/modprobe.d/hardened-blikvm-ve.conf": (
        f"usr/share/doc/{PACKAGE_NAME}/examples/hardened-blikvm-ve-modprobe.conf"
    ),
})
FILE_MAP.update({
    f"share/ve/source/{name}": f"usr/share/doc/{PACKAGE_NAME}/ve-module-source/{name}"
    for name in VE_MODULE_SOURCE_FILES
})
NATIVE_PROFILE_UNITS = (
    "board-owner", "board-video-proxy", "boardd", "server-current", "server-candidate",
    "update-activation@",
)
FILE_MAP.update({
    f"share/systemd/native/hardened-blikvm-{unit}.service":
    f"usr/share/doc/{PACKAGE_NAME}/examples/native/hardened-blikvm-{unit}.service"
    for unit in NATIVE_PROFILE_UNITS
})
FILE_MAP.update({
    f"share/systemd/hardened-blikvm-{role}.service.d/94-application-update.conf":
    f"usr/share/doc/{PACKAGE_NAME}/examples/hardened-blikvm-{role}.service.d/94-application-update.conf"
    for role in ("board-server", "server-candidate", "server-current")
})
EXECUTABLE_DESTINATIONS = {
    "usr/libexec/hardened-blikvm/bli-kvm-update-supervisor",
    "usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter",
    "usr/libexec/hardened-blikvm/bli-kvm-update-exec-helper",
    "usr/libexec/hardened-blikvm/bli-kvm-admin-helper",
    "usr/libexec/hardened-blikvm/bli-kvm-system-update",
    "usr/libexec/hardened-blikvm/bli-kvm-quic-gateway",
    "usr/libexec/hardened-blikvm/bli-kvm-ve-load",
    "usr/share/initramfs-tools/hooks/hardened-blikvm-system-update",
    "usr/share/initramfs-tools/scripts/local-premount/hardened-blikvm-system-update",
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
        "bli-kvm-update-exec-helper": 11,
        "hardened-blikvm-update-exec.socket": 11,
        "hardened-blikvm-update-exec@.service": 11,
        "hardened-blikvm-update-native-boot.service": 11,
        "hardened-blikvm-hid-gadget.service": 19,
        "bli-kvm-admin-helper": 20,
        "hardened-blikvm-admin.socket": 20,
        "hardened-blikvm-admin@.service": 20,
        "hardened-blikvm-image-download.socket": 23,
        "hardened-blikvm-image-download@.service": 23,
        "hardened-blikvm-picoclaw.service": 24,
        "hardened-blikvm-picoclaw-api.socket": 24,
        "hardened-blikvm-picoclaw-api@.service": 24,
        "AGENT.md": 24,
        "AGENT_KVM.md": 24,
        "tailscale-archive-keyring.asc": 25,
        "bli-kvm-system-update": 26,
        "hardened-blikvm-system-update-booted.service": 26,
        "system-trust.json": 26,
        "initramfs-hook": 26,
        "initramfs-local-premount": 26,
        # The revision counts the supported versions: dev35 is the 32nd.
        "bli-kvm-quic-gateway": 32,
        "hardened-blikvm-quic.service": 32,
        "hardened-blikvm-quic.conf": 32,
        # dev36 is the 33rd.
        "bli-kvm-ve-load": 33,
        "hardened-blikvm-ve.service": 33,
        "99-hardened-blikvm-ve.rules": 33,
        # The group's fragment and the modprobe line have one name.
        "hardened-blikvm-ve.conf": 33,
        # dev39 is the 36th.
        "hardened-blikvm-download.conf": 36,
        # dev40 is the 37th.
        "hardened-blikvm-picoclaw-install.socket": 37,
        "hardened-blikvm-picoclaw-install@.service": 37,
        "hardened-blikvm-picoclaw-install.conf": 37,
        # dev42 is the 39th.
        "50-system-config.conf": 39,
        "94-application-update.conf": 10,
        "hardened-blikvm-update-activation.socket": 9,
        "hardened-blikvm-update-activation@.service": 9,
        "hardened-blikvm-update-upload.socket": 8,
        "hardened-blikvm-update-upload@.service": 8,
        "hardened-blikvm-update-recover.service": 5,
        "hardened-blikvm-update-status.socket": 6,
        "hardened-blikvm-update-status@.service": 6,
    }
    def since(source: str) -> int:
        # Native examples share base-unit names, so they are keyed by directory.
        if source.startswith("share/systemd/native/"):
            return 11
        # The driver and its source, by their directory.
        if source.startswith("share/ve/"):
            return 33
        # nginx's file shares its name with the base sysusers fragment.
        if source.startswith("share/nginx/"):
            return 39
        return introduced.get(source.rsplit("/", 1)[-1], 1)

    return {source: destination for source, destination in FILE_MAP.items()
            if revision >= since(source)}


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
        # dev26 carries the RAW system update step, inert until the operator
        # switches it on and the initramfs is rebuilt.
        "systemUpdateIncluded": SUPPORTED_PACKAGE_VERSIONS.index(version) >= 25,
        "hardwareMutationIncluded": False,
        # dev36 carries a kernel module, loaded by a unit that is installed
        # disabled.
        "kernelModuleIncluded": SUPPORTED_PACKAGE_VERSIONS.index(version) >= 32,
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
    # The one module is the encoder driver for the release its directory
    # names; nothing else under the package's tree is a module.
    for path in files:
        relative = str(path.relative_to(root))
        if relative == VE_MODULE_DESTINATION:
            try:
                ve_module.check(path.read_bytes(), VE_KERNEL_RELEASE)
            except ve_module.ModuleError as error:
                raise BuildError("the packaged kernel module is not the encoder driver") from error
        elif path.name.endswith(".ko"):
            raise BuildError("update package contains an unexpected kernel module")
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
                f" The encoder driver of the video engine is for kernel {VE_KERNEL_RELEASE} only;",
                " it is GPL-2.0-only (/usr/share/common-licenses/GPL-2) and its source is in",
                f" /usr/share/doc/{PACKAGE_NAME}/ve-module-source.",
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
