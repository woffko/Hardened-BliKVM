#!/usr/bin/env python3
"""Build a hardened opaque-ID media catalog from a device's KVMD MSD directory.

The tool is read-only: it lists regular files in the fixed MSD directory over
SSH, computes each file's SHA-256, and writes a catalog whose opaque IDs map to
the real KVMD image names so a later `select` reaches the board's images. It
never mounts, ejects, uploads or deletes anything.

The SSH password is supplied on stdin; the tool never prints it.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


DEFAULT_MSD_DIR = "/var/lib/kvmd/msd"
MAX_IMAGES = 64
IMAGE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$")
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--msd-dir", default=DEFAULT_MSD_DIR)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def build_list_script(msd_dir: str) -> str:
    return (
        "set -eu\n"
        f"dir={msd_dir!r}\n"
        '[ -d "$dir" ] || { echo "msd_dir_missing" >&2; exit 1; }\n'
        'find "$dir" -maxdepth 1 -type f -printf "%f\\t%s\\n" | LC_ALL=C sort\n'
    )


def build_digest_script(msd_dir: str, names: list[str]) -> str:
    lines = ["set -eu", f"dir={msd_dir!r}"]
    for name in names:
        lines.append(f'sha256sum "$dir/{name}" | cut -d " " -f 1')
    return "\n".join(lines) + "\n"


def parse_listing(raw: bytes) -> list[tuple[str, int]]:
    entries: list[tuple[str, int]] = []
    for line in raw.decode("ascii", errors="strict").splitlines():
        name, separator, size = line.partition("\t")
        if separator != "\t" or not IMAGE_NAME.fullmatch(name):
            raise ValueError("MSD listing contained an unsafe name")
        if not size.isdigit():
            raise ValueError("MSD listing contained an invalid size")
        entries.append((name, int(size)))
    if not entries:
        raise ValueError("MSD directory has no eligible images")
    if len(entries) > MAX_IMAGES:
        raise ValueError("MSD directory has too many images")
    return entries


def parse_digests(raw: bytes, count: int) -> list[str]:
    digests = raw.decode("ascii", errors="strict").split()
    if len(digests) != count or any(not SHA256.fullmatch(value) for value in digests):
        raise ValueError("MSD digest output is invalid")
    return digests


def build_catalog(entries: list[tuple[str, int]], digests: list[str]) -> dict[str, object]:
    return {
        "schemaVersion": 1,
        "entries": [
            {
                "imageId": index + 1,
                "imageName": name,
                "size": size,
                "sha256": digest,
                "cdromAllowed": True,
                "removableAllowed": True,
                "writableAllowed": False,
            }
            for index, ((name, size), digest) in enumerate(zip(entries, digests))
        ],
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    if not args.msd_dir.startswith("/") or ".." in args.msd_dir:
        raise ValueError("MSD directory must be an absolute safe path")
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()

    listing = prototype.run_ssh(
        target, secret, build_list_script(args.msd_dir), timeout=60
    )
    listing.check_returncode()
    entries = parse_listing(listing.stdout)
    digests = prototype.run_ssh(
        target,
        secret,
        build_digest_script(args.msd_dir, [name for name, _ in entries]),
        timeout=600,
    )
    digests.check_returncode()
    catalog = build_catalog(entries, parse_digests(digests.stdout, len(entries)))
    write_json_0600_atomic(output, catalog)
    digest = hashlib.sha256(json.dumps(catalog, sort_keys=True).encode()).hexdigest()
    print(f"catalog={output} images={len(entries)} catalogSha256={digest}")
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, subprocess.SubprocessError):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
