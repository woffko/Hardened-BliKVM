"""The Python tools made for the H.264 engine choice and the quality monitor
(plan of 2026-10-03 and the fixes of the review of 2026-10-04) carry the licence
line as their first or second line, after the interpreter's, as the tools of the
hardware H.264 plan do."""

import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOLS = (
    "tools/device/engine_switch_probe.py",
    "tools/device/frame_socket_tap.py",
    "tools/device/ve_device_hold.py",
    "tools/device/ve_engine_choose.py",
    "tools/device/ve_engine_switch.py",
    "tools/device/ve_engine_watch.py",
)
LICENCE = "# SPDX-License-Identifier: GPL-3.0-only"


class PlanToolHeaderTests(unittest.TestCase):
    def test_every_tool_names_its_licence_at_the_top(self) -> None:
        for name in TOOLS:
            with self.subTest(tool=name):
                lines = (ROOT / name).read_text(encoding="utf-8").splitlines()
                self.assertEqual(lines[0], "#!/usr/bin/env python3")
                self.assertEqual(lines[1], LICENCE)


if __name__ == "__main__":
    unittest.main()
