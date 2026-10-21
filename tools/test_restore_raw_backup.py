"""The host tool that writes a board's root back from a RAW update's backup.

A card image is made of a root partition and an ext4 data partition holding
the update's record and backup, as the board leaves them when power is lost
while the new root is written (code audit of 2026-10-20, AUD-01). The tool
must write the backup back byte for byte, and must refuse a backup the record
does not name or a record with no backup to write.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / "tools/image/restore_raw_backup.py"
SECTOR = 512
ROOT_START, ROOT_SECTORS = 2048, 8192  # 4 MiB
DATA_START, DATA_SECTORS = 10240, 16384  # 8 MiB
TOOLS = ("sfdisk", "mkfs.ext4", "debugfs", "zstd")


@unittest.skipUnless(all(shutil.which(tool) for tool in TOOLS), "needs e2fsprogs, sfdisk and zstd")
class RestoreRawBackupTests(unittest.TestCase):
    def setUp(self) -> None:
        self.work = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.work)
        self.original = os.urandom(3 * 1024 * 1024)

    def card(self, phase: str, backup: bytes | None = None, sha256: str | None = None) -> Path:
        staging = self.work / "data/.hardened-system-update"
        staging.mkdir(parents=True)
        (self.work / "root.img").write_bytes(self.original)
        subprocess.run(
            ["zstd", "-q", "-o", str(staging / "root.backup.zst"), str(self.work / "root.img")],
            check=True,
        )
        if backup is not None:
            (staging / "root.backup.zst").write_bytes(backup)
        record = {
            "phase": phase,
            "version": "0.1.0-test",
            "backupSize": len(self.original),
            "backupSha256": sha256 or hashlib.sha256(self.original).hexdigest(),
        }
        (staging / "apply.json").write_text(json.dumps(record))
        data = self.work / "data.img"
        subprocess.run(
            ["mkfs.ext4", "-q", "-F", "-d", str(self.work / "data"), str(data), f"{DATA_SECTORS // 2}k"],
            check=True,
        )
        card = self.work / "card.img"
        with open(card, "wb") as out:
            out.truncate((DATA_START + DATA_SECTORS) * SECTOR)
        subprocess.run(
            ["sfdisk", "-q", str(card)],
            input=(
                "label: dos\n"
                f"start={ROOT_START}, size={ROOT_SECTORS}, type=83\n"
                f"start={DATA_START}, size={DATA_SECTORS}, type=83\n"
            ),
            text=True,
            check=True,
        )
        with open(card, "r+b") as out:
            out.seek(DATA_START * SECTOR)
            out.write(data.read_bytes())
            # Half of a new root written over the old one.
            out.seek(ROOT_START * SECTOR)
            out.write(b"new root, cut short" * 1000)
        return card

    def root_of(self, card: Path) -> bytes:
        with open(card, "rb") as source:
            source.seek(ROOT_START * SECTOR)
            return source.read(len(self.original))

    def run_tool(self, card: Path) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-I", str(TOOL), str(card), "--yes"],
            capture_output=True,
            text=True,
        )

    def test_an_interrupted_write_is_undone_from_the_backup(self) -> None:
        card = self.card("writing")
        result = self.run_tool(card)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.root_of(card), self.original)

    def test_a_record_without_a_backup_to_write_is_refused(self) -> None:
        card = self.card("confirmed")
        before = self.root_of(card)
        result = self.run_tool(card)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("no backup", result.stderr)
        self.assertEqual(self.root_of(card), before)

    def test_a_backup_the_record_does_not_name_is_refused(self) -> None:
        card = self.card("writing", sha256="0" * 64)
        before = self.root_of(card)
        result = self.run_tool(card)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("not the one the record names", result.stderr)
        self.assertEqual(self.root_of(card), before)


if __name__ == "__main__":
    unittest.main()
