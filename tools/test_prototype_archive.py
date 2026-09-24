"""Exercise the bundle publisher with real tar/gzip and injected failures."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile
import unittest


BUILDER = Path(__file__).resolve().parents[1] / "scripts/build-prototype-bundle.sh"


class PrototypeArchiveTests(unittest.TestCase):
    builder = BUILDER
    staging_name = ".hardened-blikvm-bundle.fixture"
    def publish(
        self, root: Path, failure: str | None = None, mtime: int = 100
    ) -> subprocess.CompletedProcess[str]:
        # Run the actual publication and cleanup blocks, omitting cross-builds.
        source = self.builder.read_text(encoding="utf-8")
        prelude = source[:source.index("project_root=")]
        cleanup = source[source.index("cleanup() {"):source.index('\nstage=')]
        publication = source[source.index('archive="$output_dir/') :]
        stage_root = root / self.staging_name
        stage = stage_root / "fixture"
        stage.mkdir(parents=True)
        (stage / "payload").write_bytes(b"reviewed fixture\n")
        os.utime(stage / "payload", (mtime, mtime))
        os.utime(stage, (mtime, mtime))
        tools = root / "tools"
        tools.mkdir(exist_ok=True)
        if failure is not None:
            tool = tools / failure
            tool.write_text("#!/bin/sh\nprintf 'incomplete archive'\nexit 23\n", encoding="utf-8")
            tool.chmod(0o755)
        env = {
            **os.environ,
            "PATH": f"{tools}:{os.environ['PATH']}",
            "temp_root": str(stage_root),
            "output_dir": str(root),
            "bundle_name": "fixture",
        }
        return subprocess.run(
            [*shlex.split(source.splitlines()[0][2:]), "-c", prelude + cleanup + publication],
            env=env, capture_output=True, text=True, timeout=5, check=False,
        )

    def test_failed_producer_or_compressor_preserves_previous_release(self) -> None:
        for failure in ("tar", "gzip"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                archive = root / "fixture.tar.gz"
                checksum = root / "fixture.tar.gz.sha256"
                archive.write_bytes(b"previous release")
                checksum.write_bytes(b"previous checksum")
                result = self.publish(root, failure)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(archive.read_bytes(), b"previous release")
                self.assertEqual(checksum.read_bytes(), b"previous checksum")
                self.assertEqual(sorted(p.name for p in root.iterdir()),
                                 ["fixture.tar.gz", "fixture.tar.gz.sha256", "tools"])

    def test_real_archive_is_reproducible_and_checksum_matches(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            first = self.publish(root)
            self.assertEqual(first.returncode, 0, first.stderr)
            archive = root / "fixture.tar.gz"
            original = archive.read_bytes()
            second = self.publish(root, mtime=200)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertEqual(archive.read_bytes(), original)
            expected = hashlib.sha256(original).hexdigest()
            self.assertEqual((root / "fixture.tar.gz.sha256").read_text(),
                             f"{expected}  fixture.tar.gz\n")
            with tarfile.open(archive) as packaged:
                self.assertEqual(packaged.getnames(), ["fixture", "fixture/payload"])
                for member in packaged.getmembers():
                    self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))


class BoardArchiveTests(PrototypeArchiveTests):
    builder = BUILDER.with_name("build-board-profile.sh")
    staging_name = ".fixture.stage.fixture"


if __name__ == "__main__":
    unittest.main()
