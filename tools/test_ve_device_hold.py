"""The busy-engine holder opens the device it is given, holds it for the time it
was told or until it is ended, asks it for nothing, and says when it lets go
(tools/device/ve_device_hold.py, F3 of
docs/baselines/work-plan-20261004-review-fixes.md)."""

import os
import signal
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "tools/device/ve_device_hold.py"


def start(*arguments: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [sys.executable, str(TOOL), *arguments],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def has_open(pid: int, path: Path) -> bool:
    """Whether the process has `path` open."""
    for descriptor in Path(f"/proc/{pid}/fd").iterdir():
        try:
            if os.readlink(descriptor) == str(path):
                return True
        except OSError:
            continue
    return False


class DeviceHoldTests(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.device = Path(directory.name) / "device"
        self.device.write_bytes(b"")

    def test_it_wants_a_number_of_seconds_in_range(self) -> None:
        for arguments in ([], ["soon"], ["0"], ["601"], ["-5"], ["1", "a", "b"]):
            with self.subTest(arguments=arguments):
                process = start(*arguments)
                process.communicate(timeout=10)
                self.assertEqual(process.returncode, 2)

    def test_a_device_that_cannot_be_opened_is_said_and_not_held(self) -> None:
        process = start("1", str(self.device.parent / "missing"))
        out, _ = process.communicate(timeout=10)
        self.assertEqual(process.returncode, 1)
        self.assertIn("could not open", out)

    def test_it_holds_the_device_for_the_time_and_asks_it_for_nothing(self) -> None:
        before = self.device.stat()
        process = start("2", str(self.device))
        self.assertIn("holding", process.stdout.readline())
        self.assertTrue(has_open(process.pid, self.device))
        out, _ = process.communicate(timeout=20)
        self.assertEqual(process.returncode, 0)
        self.assertIn("released", out)
        after = self.device.stat()
        self.assertEqual((after.st_size, after.st_mtime_ns), (0, before.st_mtime_ns))

    def test_it_lets_go_when_it_is_ended(self) -> None:
        for name in ("SIGTERM", "SIGINT"):
            with self.subTest(signal=name):
                process = start("300", str(self.device))
                self.assertIn("holding", process.stdout.readline())
                process.send_signal(getattr(signal, name))
                out, _ = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 0)
                self.assertIn(f"released {self.device} on {name}", out)


if __name__ == "__main__":
    unittest.main()
