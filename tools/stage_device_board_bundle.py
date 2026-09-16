#!/usr/bin/env python3
"""Stage the verified board bundle into the gate runtime root.

The reversible board gate extracts and verifies ``bundle.tar.gz`` under its
runtime root, but it never uploads one. This tool performs exactly that single
transfer: it re-verifies the local bundle (members, manifest profile and
features, recorded hashes, size bounds), streams it over the SSH process stdin
into a ``.part`` file, checks the remote SHA-256, and only then moves it into
place. Nothing else is touched: no unit, service, retained slot, catalog or
permanent write policy.

The SSH password is read once from stdin; it never appears in the arguments, the
environment or the receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import sys
from pathlib import Path
from typing import Any, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import verify_board_profile_bundle as bundle_verifier

GATE_ROOT = "/var/lib/hardened-blikvm-gadget/gate"
TARGET_PATH = f"{GATE_ROOT}/bundle.tar.gz"
MIN_BUNDLE_BYTES = 1024 * 1024
MAX_BUNDLE_BYTES = 64 * 1024 * 1024
UPLOAD_FACTS = {
    "aarch64",
    "bundle_bytes",
    "bundle_sha256",
    "bundle_present",
    "part_absent",
}


def validate_local_bundle(path: Path, expected_sha256: str) -> dict[str, Any]:
    """Re-verify the bundle before anything is streamed to the device."""
    if not path.is_file() or path.is_symlink():
        raise ValueError("bundle must be a regular non-symlink file")
    size = path.stat().st_size
    if not MIN_BUNDLE_BYTES <= size <= MAX_BUNDLE_BYTES:
        raise ValueError("bundle size is out of range")
    expected = prototype.validate_sha256(expected_sha256)
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    actual = digest.hexdigest()
    if actual != expected:
        raise ValueError("bundle SHA-256 mismatch")
    try:
        verified = bundle_verifier.verify_bundle(path, "")
    except Exception as error:  # noqa: BLE001 - any verifier failure is a reject
        raise ValueError(f"bundle verification failed: {type(error).__name__}") from error
    if verified.get("sha256") != expected:
        raise ValueError("bundle verification disagrees with the expected hash")
    return {"size": size, "sha256": actual, "members": verified.get("memberCount")}


def build_upload_script(expected_sha256: str) -> str:
    expected = prototype.validate_sha256(expected_sha256)
    return f"""set -eu
export LC_ALL=C
emit() {{ printf '%s=%s\\n' "$1" "$2"; }}
arch=$(uname -m)
if [ "$arch" = aarch64 ]; then emit aarch64 yes; else emit aarch64 no; fi
root={GATE_ROOT}
target={TARGET_PATH}
part=${{target}}.part
install -d -m 0700 "$root"
rm -f "$part"
cat > "$part"
[ -s "$part" ] && emit bundle_bytes "$(wc -c < "$part" | tr -d ' ')"
sha=$(sha256sum "$part" | cut -d ' ' -f 1)
if [ "$sha" = "{expected}" ]; then
  mv "$part" "$target"
fi
if [ -f "$target" ] && [ "$(sha256sum "$target" | cut -d ' ' -f 1)" = "{expected}" ]; then
  emit bundle_present yes
else
  emit bundle_present no
fi
emit bundle_sha256 "$sha"
if [ -e "$part" ]; then emit part_absent no; else emit part_absent yes; fi
"""


def validate_upload(
    facts: dict[str, str], expected_sha256: str, local_size: int
) -> None:
    if facts["aarch64"] != "yes":
        raise ValueError("the target is not an aarch64 board")
    if facts["bundle_bytes"] != str(local_size):
        raise ValueError(
            f"the uploaded byte count differs: {facts['bundle_bytes']} != {local_size}"
        )
    if facts["bundle_sha256"] != expected_sha256:
        raise ValueError("the uploaded archive does not match the expected hash")
    if facts["bundle_present"] != "yes":
        raise ValueError("the bundle was not moved into place")
    if facts["part_absent"] != "yes":
        raise ValueError("a partial upload was left behind")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--bundle", required=True)
    _ = parser.add_argument("--expected-sha256", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    bundle = Path(args.bundle).resolve(strict=True)
    local = validate_local_bundle(bundle, cast(str, args.expected_sha256))
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    username = prototype.validate_username(args.username)
    known_hosts = prototype.validate_private_file(args.known_hosts)
    target = prototype.RemoteTarget(host, ssh_port, username, known_hosts)
    secret = prototype.read_secret_stdin()

    with bundle.open("rb") as stream:
        result = prototype.run_ssh(
            target,
            secret,
            build_upload_script(local["sha256"]),
            stdin=stream,
            timeout=120,
        )
    result.check_returncode()
    facts = prototype.parse_facts(result.stdout, set(UPLOAD_FACTS))
    validate_upload(facts, str(local["sha256"]), int(local["size"]))

    receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-board-bundle-transfer",
        "target": {"host": host, "aligned": facts["aarch64"] == "yes"},
        "bundle": {
            "path": TARGET_PATH,
            "bytes": int(local["size"]),
            "localSha256": local["sha256"],
            "remoteSha256": facts["bundle_sha256"],
            "members": local["members"],
        },
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": True,
            "receiptContainsSecrets": False,
            "touchesUnitsOrRetainedSlots": False,
        },
    }
    write_json_0600_atomic(output, receipt)
    print(
        f"board_bundle_staged={output} bytes={local['size']} sha256={local['sha256']}"
    )
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError) as error:
        print(f"board_bundle_stage: {type(error).__name__}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
