#!/usr/bin/env python3
"""Materialize the owner's catalog image from the KVMD MSD directory.

The board owner serves ``BLI_KVM_GADGET_IMAGES`` and resolves a catalog entry to
``<images dir>/<image name>``. On this board the reviewed image already exists in
KVMD's MSD directory, so the owner needs a link, not a second 6.8 GiB copy. This
tool verifies the source against the catalogued size and SHA-256 and then creates
that link; teardown removes it, so the change is reversible.

Only symlinks are created or replaced, and only inside the owner's image
directory. Nothing else on the device is touched.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

IMAGE_DIR = "/var/lib/hardened-blikvm-gadget/images"
SOURCE_DIR = "/var/lib/kvmd/msd"
MAX_IMAGE_BYTES = 32 * 1024 * 1024 * 1024
IMAGE_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")
STAGE_FACTS = {
    "source_present",
    "source_bytes",
    "source_sha256",
    "link_created",
}


def build_stage_script(image: str, source: str, expected_sha256: str) -> str:
    if not IMAGE_NAME.fullmatch(image) or not IMAGE_NAME.fullmatch(source):
        raise ValueError("image and source names must be plain file names")
    expected = prototype.validate_sha256(expected_sha256)
    return f"""set -eu
export LC_ALL=C
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
image_dir={IMAGE_DIR}
source={SOURCE_DIR}/{source}
target=$image_dir/{image}
install -d -m 0700 "$image_dir"
if [ -f "$source" ]; then
  emit source_present yes
  emit source_bytes "$(wc -c < "$source" | tr -d ' ')"
  emit source_sha256 "$(sha256sum "$source" | cut -d ' ' -f 1)"
else
  emit source_present no
  emit source_bytes 0
  emit source_sha256 none
fi
if [ "$(sha256sum "$source" 2>/dev/null | cut -d ' ' -f 1)" = "{expected}" ]; then
  ln -sfn "$source" "$target"
  if [ -L "$target" ] && [ "$(readlink "$target")" = "$source" ]; then
    emit link_created yes
  else
    emit link_created no
  fi
else
  emit link_created no
fi
"""


def validate_stage(
    facts: dict[str, str], expected_sha256: str, expected_bytes: int
) -> None:
    if facts["source_present"] != "yes":
        raise ValueError("the source image is missing")
    if facts["source_bytes"] != str(expected_bytes):
        raise ValueError(
            f"source size differs: {facts['source_bytes']} != {expected_bytes}"
        )
    if facts["source_sha256"] != expected_sha256:
        raise ValueError("the source image does not match the catalogued hash")
    if facts["link_created"] != "yes":
        raise ValueError("the owner image link was not created")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--image", default="kubuntu.img")
    _ = parser.add_argument("--source", default="kubuntu.img")
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--expected-bytes", required=True, type=int)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    expected_bytes = cast(int, args.expected_bytes)
    if not 0 < expected_bytes <= MAX_IMAGE_BYTES:
        raise ValueError("expected size is out of range")
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()

    script = build_stage_script(
        cast(str, args.image), cast(str, args.source), cast(str, args.expected_sha256)
    )
    result = prototype.run_ssh(target, secret, script, timeout=900)
    result.check_returncode()
    facts = prototype.parse_facts(result.stdout, set(STAGE_FACTS))
    validate_stage(facts, str(args.expected_sha256), expected_bytes)

    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-owner-image",
        "image": {
            "name": args.image,
            "path": f"{IMAGE_DIR}/{args.image}",
            "source": f"{SOURCE_DIR}/{args.source}",
            "bytes": expected_bytes,
            "sha256": args.expected_sha256,
            "link": True,
        },
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": True,
            "copiedData": False,
            "cleanupHint": f"rm -f {IMAGE_DIR}/{args.image}",
        },
    }
    write_json_0600_atomic(output, receipt)
    print(
        f"owner_image={output} name={args.image} bytes={expected_bytes} verified=yes"
    )
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"owner_image: {type(error).__name__}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
