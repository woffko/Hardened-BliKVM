"""Fixed host policy for runtime identities and the dedicated KVMD credential."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path, PurePosixPath
import stat

from tools import stage_device_debian_runtime_package as package_gate


PROJECT_ROOT = Path(__file__).resolve().parent.parent
SYSUSERS_SOURCE = PROJECT_ROOT / "packaging/sysusers.d/hardened-blikvm.conf"
INSTALLED_SYSUSERS_PATH = (
    "/usr/share/doc/hardened-blikvm-runtime/examples/hardened-blikvm-sysusers.conf"
)
MAX_SYSUSERS_BYTES = 4096
EXPECTED_SYSUSERS_SHA256 = (
    "629bfaa0e3653acd59271c135d3322910c275ecb6533221b6b689639ff9ea070"
)
SHARED_GROUP = "hardened-blikvm"
KVMD_ACCOUNT = "hardened-blikvm-broker"
USERNAME_BYTES = len(KVMD_ACCOUNT.encode("ascii")) + 1
ACCOUNT_NAMES = (
    "hardened-blikvm-broker",
    "hardened-blikvm-video",
    "hardened-blikvm-control",
    "hardened-blikvm-hid",
    "hardened-blikvm-server",
    "hardened-blikvm-update",
)
GROUP_NAMES = (SHARED_GROUP, *ACCOUNT_NAMES)
ACCOUNT_DATABASE_NAMES = ("passwd", "group", "shadow", "gshadow")
ACCOUNT_AUXILIARY_NAMES = ("passwd-", "group-", "shadow-", "gshadow-", ".pwd.lock")
CONFIG_ROOT = "/etc/hardened-blikvm"
USERNAME_PATH = f"{CONFIG_ROOT}/kvmd-user"
PASSWORD_PATH = f"{CONFIG_ROOT}/kvmd-password"
EXPECTED_SYSUSERS_LINES = (
    "g hardened-blikvm - -",
    'u hardened-blikvm-broker - "Hardened BliKVM platform broker" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-broker hardened-blikvm",
    'u hardened-blikvm-video - "Hardened BliKVM MJPEG proxy" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-video hardened-blikvm",
    'u hardened-blikvm-control - "Hardened BliKVM control policy broker" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-control hardened-blikvm",
    'u hardened-blikvm-hid - "Hardened BliKVM HID transition credential" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-hid hardened-blikvm",
    'u hardened-blikvm-server - "Hardened BliKVM network server" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-server hardened-blikvm",
    'u hardened-blikvm-update - "Hardened BliKVM application update supervisor" /nonexistent /usr/sbin/nologin',
    "m hardened-blikvm-update hardened-blikvm",
)


@dataclass(frozen=True)
class SysusersInfo:
    path: Path
    sha256: str
    size_bytes: int
    payload: bytes


def validate_sysusers_payload(payload: bytes) -> tuple[str, int]:
    if not 0 < len(payload) <= MAX_SYSUSERS_BYTES or b"\x00" in payload or b"\r" in payload:
        raise ValueError("runtime sysusers source size or encoding is invalid")
    text = payload.decode("ascii", errors="strict")
    if not text.endswith("\n"):
        raise ValueError("runtime sysusers source must end with one newline")
    lines = tuple(
        line.strip()
        for line in text.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    )
    if lines != EXPECTED_SYSUSERS_LINES:
        raise ValueError("runtime sysusers source is outside the fixed identity policy")
    digest = hashlib.sha256(payload).hexdigest()
    if digest != EXPECTED_SYSUSERS_SHA256:
        raise ValueError("runtime sysusers source bytes are not the pinned template")
    return digest, len(payload)


def load_sysusers_source(package: package_gate.PackageInfo) -> SysusersInfo:
    path = SYSUSERS_SOURCE
    metadata = path.lstat()
    if (
        not stat.S_ISREG(metadata.st_mode)
        or path.is_symlink()
        or metadata.st_mode & 0o022
    ):
        raise ValueError("runtime sysusers source metadata is unsafe")
    payload = path.read_bytes()
    digest, size = validate_sysusers_payload(payload)
    installed = package.installed_files.get(INSTALLED_SYSUSERS_PATH)
    if installed != (digest, 0o644):
        raise ValueError("retained package sysusers documentation does not match source")
    return SysusersInfo(path, digest, size, payload)


def validate_credential_username(payload: bytes) -> None:
    if payload != f"{KVMD_ACCOUNT}\n".encode("ascii"):
        raise ValueError("dedicated KVMD username payload is invalid")


def dedicated_payload_path(path: str) -> PurePosixPath:
    parsed = PurePosixPath(path)
    if parsed not in {PurePosixPath(USERNAME_PATH), PurePosixPath(PASSWORD_PATH)}:
        raise ValueError("credential path is outside the fixed configuration root")
    return parsed
