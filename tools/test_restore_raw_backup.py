"""The host tool that writes a board's root back from a RAW update's backup.

A card image is made of a root partition and an ext4 data partition holding
the update's record and backup, as the board leaves them when power is lost
while the new root is written (code audit of 2026-10-20, AUD-01). The tool
must write the backup back byte for byte, and must refuse a backup the record
does not name or a record with no backup to write.
"""

from __future__ import annotations

import errno
import hashlib
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

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

    def run_tool(self, card: Path, env: dict | None = None) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, "-I", str(TOOL), str(card), "--yes"],
            capture_output=True,
            text=True,
            env=env,
        )

    def test_an_interrupted_write_is_undone_from_the_backup(self) -> None:
        card = self.card("writing")
        result = self.run_tool(card)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.root_of(card), self.original)

    def test_a_work_directory_with_a_space_still_restores(self) -> None:
        # Code audit of 2026-11-06, AUD-14: debugfs read the dump's paths
        # unquoted, so a temporary directory with a space split them.
        card = self.card("writing")
        spaced = self.work / "work dir"
        spaced.mkdir()
        result = self.run_tool(card, env={**os.environ, "TMPDIR": str(spaced)})
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


class ClaimedRootTests(unittest.TestCase):
    """A root partition in use is never written (code audit of 2026-11-06, AUD-14)."""

    def setUp(self) -> None:
        spec = importlib.util.spec_from_file_location("restore_raw_backup", TOOL)
        self.tool = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.tool)
        self.work = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.work)

    @unittest.skipUnless(shutil.which("zstd"), "needs zstd")
    def test_a_root_partition_in_use_is_refused_and_left_unchanged(self) -> None:
        original = os.urandom(64 * 1024)
        (self.work / "original").write_bytes(original)
        backup = self.work / "root.backup.zst"
        subprocess.run(["zstd", "-q", "-o", str(backup), str(self.work / "original")], check=True)
        partition = self.work / "partition"
        partition.write_bytes(b"the mounted root" * 4096)
        before = partition.read_bytes()
        real_open = os.open

        def claimed(path, flags, *args, **kwargs):
            if flags & os.O_EXCL:
                raise OSError(errno.EBUSY, os.strerror(errno.EBUSY), str(path))
            return real_open(path, flags, *args, **kwargs)

        with (
            mock.patch.object(self.tool, "is_block", return_value=True),
            mock.patch.object(self.tool.os, "open", side_effect=claimed),
        ):
            with self.assertRaises(self.tool.Refused) as refused:
                self.tool.write_back(
                    self.work / "card",
                    {"node": str(partition), "start": 0},
                    backup,
                    len(original),
                    hashlib.sha256(original).hexdigest(),
                )
        self.assertIn("the root partition is mounted or in use", str(refused.exception))
        self.assertEqual(partition.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
