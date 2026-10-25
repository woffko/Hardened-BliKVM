"""The third-party build scripts never `rm -rf` a path they did not make."""

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GUARD = ROOT / "scripts/safe-output.sh"


def refuses(path: str, home: str | None = None) -> bool:
    env = dict(os.environ)
    if home is not None:
        env["HOME"] = home
    result = subprocess.run(
        ["sh", "-c", f'. "{GUARD}"; refuse_unsafe_output "$1"', "sh", path],
        env=env,
        capture_output=True,
        text=True,
    )
    return result.returncode != 0


class SafeOutputTests(unittest.TestCase):
    def test_dangerous_paths_are_refused(self) -> None:
        for path in ("/", "//", "/usr", "/home/user", os.environ["HOME"], "relative/dir"):
            self.assertTrue(refuses(path), path)

    def test_only_directories_a_previous_run_made_are_replaced(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp) / "a" / "b"
            base.mkdir(parents=True)
            fresh = base / "out"
            self.assertFalse(refuses(str(fresh)), "an absent directory is fine")
            fresh.mkdir()
            self.assertFalse(refuses(str(fresh)), "an empty one too")
            (fresh / "SOURCE").write_text("x264 abc\n")
            (fresh / "lib").mkdir()
            self.assertFalse(refuses(str(fresh)), "one an earlier run made")
            other = base / "precious"
            other.mkdir()
            (other / "photo.jpg").write_text("keep")
            self.assertTrue(refuses(str(other)), "a directory with foreign files")
            self.assertTrue(other.exists())
            link = base / "link"
            link.symlink_to(fresh)
            self.assertTrue(refuses(str(link)), "a symlink")

    def test_both_scripts_call_the_guard_before_deleting(self) -> None:
        for name in ("build-x264.sh", "build-libvpx.sh", "build-libjpeg-turbo.sh"):
            text = (ROOT / "scripts" / name).read_text()
            self.assertIn("safe-output.sh", text, name)
            self.assertLess(
                text.index("refuse_unsafe_output"), text.index('rm -rf -- "$output"'), name
            )


if __name__ == "__main__":
    unittest.main()
