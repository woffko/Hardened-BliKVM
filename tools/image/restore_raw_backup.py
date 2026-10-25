#!/usr/bin/env python3
"""Write a board's root back from the backup a RAW system update keeps.

The RAW update backs the whole root partition up to the data partition
(`.hardened-system-update/root.backup.zst`) before it writes the new root,
and the board restores it by itself when the new root does not come up. A
power loss while the new root is written leaves a root that U-Boot cannot
boot, so the board never reaches that step: this tool does it on another
Linux computer, with the board's card in a reader
(docs/design/system-raw-update-v1.md).

    restore_raw_backup.py CARD [--yes]

CARD is the whole card: its block device (`/dev/sdX`, `/dev/mmcblkN`, run as
root) or an image file of it. The tool reads the update's record and its
backup from the second partition, checks the backup against the record,
writes it over the first partition and reads it back. The record is left as
it is: at the next boot the board finds it and restores the same backup once
more, then records the update as rolled back.

Needs `sfdisk`, `debugfs` (e2fsprogs) and `zstd`.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import subprocess
import sys
import tempfile
from pathlib import Path

STAGING = "/.hardened-system-update"
RECORD = "apply.json"
BACKUP = "root.backup.zst"
# The phases in which the board itself would write the backup back.
RESTORABLE = {"writing", "written", "rollback_requested"}
CHUNK = 1024 * 1024
SECTOR = 512


class Refused(Exception):
    pass


def partitions(card: Path) -> list[dict]:
    table = json.loads(
        subprocess.run(
            ["sfdisk", "-J", str(card)], check=True, capture_output=True, text=True
        ).stdout
    )["partitiontable"]
    if table.get("unit") != "sectors" or table.get("sectorsize", SECTOR) != SECTOR:
        raise Refused("the card's partition table is not in 512-byte sectors")
    found = table.get("partitions", [])
    if len(found) < 2:
        raise Refused("the card has no data partition")
    return found


def is_block(card: Path) -> bool:
    return stat.S_ISBLK(os.stat(card).st_mode)


def dump(card: Path, data: dict, name: str, target: Path) -> None:
    source = data["node"] if is_block(card) else f"{card}?offset={data['start'] * SECTOR}"
    result = subprocess.run(
        ["debugfs", "-R", f"dump {STAGING}/{name} {target}", source],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0 or not target.is_file():
        raise Refused(f"the data partition holds no {name}")


def backup_stream(backup: Path):
    return subprocess.Popen(["zstd", "-dc", str(backup)], stdout=subprocess.PIPE)


def check_backup(backup: Path, size: int, sha256: str) -> None:
    hasher = hashlib.sha256()
    total = 0
    with backup_stream(backup) as stream:
        while chunk := stream.stdout.read(CHUNK):
            total += len(chunk)
            hasher.update(chunk)
        if stream.wait() != 0:
            raise Refused("the backup does not decompress")
    if (total, hasher.hexdigest()) != (size, sha256):
        raise Refused("the backup is not the one the record names")


def root_target(card: Path, root: dict) -> tuple[str, int]:
    if is_block(card):
        return root["node"], 0
    return str(card), root["start"] * SECTOR


def write_back(card: Path, root: dict, backup: Path, size: int, sha256: str) -> None:
    path, offset = root_target(card, root)
    with open(path, "r+b", buffering=0) as out, backup_stream(backup) as stream:
        out.seek(offset)
        while chunk := stream.stdout.read(CHUNK):
            out.write(chunk)
        if stream.wait() != 0:
            raise Refused("the backup stopped decompressing while it was written")
        os.fsync(out.fileno())
    hasher = hashlib.sha256()
    left = size
    with open(path, "rb") as written:
        written.seek(offset)
        while left:
            chunk = written.read(min(CHUNK, left))
            if not chunk:
                break
            hasher.update(chunk)
            left -= len(chunk)
    if left or hasher.hexdigest() != sha256:
        raise Refused("the root partition read back differently")


def restore(card: Path, yes: bool) -> str:
    found = partitions(card)
    root, data = found[0], found[1]
    with tempfile.TemporaryDirectory() as work:
        work = Path(work)
        dump(card, data, RECORD, work / RECORD)
        record = json.loads((work / RECORD).read_text())
        phase = record.get("phase")
        if phase not in RESTORABLE:
            raise Refused(f"the update's record is {phase!r}: there is no backup to write back")
        size, sha256 = record.get("backupSize", 0), record.get("backupSha256", "")
        if not size or len(sha256) != 64:
            raise Refused("the update's record names no backup")
        if size > root["size"] * SECTOR:
            raise Refused("the backup is larger than the root partition")
        dump(card, data, BACKUP, work / BACKUP)
        check_backup(work / BACKUP, size, sha256)
        if not yes:
            answer = input(
                f"Write the backup of {record.get('version', 'the earlier system')!r} "
                f"({size} bytes) over the first partition of {card}? Type yes: "
            )
            if answer.strip() != "yes":
                raise Refused("not confirmed")
        write_back(card, root, work / BACKUP, size, sha256)
    return (
        f"the root was written back ({size} bytes) and read back; at the next boot "
        "the board restores the same backup once more and records the update as rolled back"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("card", type=Path, help="the card's block device or an image file of it")
    parser.add_argument("--yes", action="store_true", help="do not ask before writing")
    arguments = parser.parse_args()
    try:
        print(restore(arguments.card, arguments.yes))
    except (Refused, subprocess.CalledProcessError, OSError, ValueError) as error:
        print(f"restore_raw_backup: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
