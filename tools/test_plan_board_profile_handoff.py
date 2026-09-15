import argparse
import subprocess
import sys
import tempfile
import unittest
from typing import Any
from unittest import mock
from pathlib import Path

import tools.plan_board_profile_handoff as planner
from tools.test_verify_board_profile_bundle import build_bundle


def options(**overrides: object) -> argparse.Namespace:
    base: dict[str, object] = {
        "bundle": "",
        "output": "",
        "udc": "musb-hdrc.2.auto",
        "power_short_ms": 500,
        "power_long_ms": 4_000,
        "reset_ms": 200,
        "configfs_root": planner.CONFIGFS_ROOT,
        "images_dir": f"{planner.GADGET_STATE}/images",
        "catalog": f"{planner.GADGET_ETC}/catalog.json",
        "serial_file": f"{planner.GADGET_ETC}/serial",
        "atx_journal": f"{planner.GADGET_STATE}/atx-journal.json",
        "hid_ledger": f"{planner.GADGET_STATE}/hid-ledger.json",
        "control_socket": f"{planner.GADGET_RUN}/control.sock",
        "atx_chip": "/dev/gpiochip0",
        "hidg_dir": planner.HIDG_DIR,
        "reconciliations": "",
    }
    base.update(overrides)
    return argparse.Namespace(**base)


class PlanBoardProfileHandoffTests(unittest.TestCase):
    def test_valid_plan_pins_environment_and_has_no_mutations(self) -> None:
        plan: dict[str, Any] = planner.build_plan(options(), "a" * 64)
        self.assertEqual(plan["plan"], "hardened-blikvm-board-profile-handoff")
        self.assertIs(plan["policy"]["mutationsPerformed"], False)
        environment = plan["ownerEnvironment"]
        self.assertEqual(environment["BLI_KVM_BOARD_PROFILE"], "1")
        self.assertEqual(environment["BLI_KVM_ATX_POWER_SHORT_MS"], "500")
        self.assertEqual(environment["BLI_KVM_ATX_POWER_LONG_MS"], "4000")
        self.assertEqual(environment["BLI_KVM_ATX_RESET_MS"], "200")
        self.assertEqual(plan["atxEnvironmentFile"]["BLI_KVM_ATX_RESET_MS"], "200")
        self.assertNotIn("BLI_KVM_ATX_POWER_SHORT_MS", str(plan["steps"]))
        device = plan["device"]
        self.assertEqual(device["configfsRoot"], planner.CONFIGFS_ROOT)
        self.assertEqual(device["udc"], "musb-hdrc.2.auto")

    def test_parser_defaults_use_the_vendor_pinned_timings(self) -> None:
        argv = ["plan_board_profile_handoff", "--bundle", "b", "--output", "o", "--udc", "u"]
        with mock.patch.object(sys, "argv", argv):
            parsed = planner.parse_args()
        self.assertEqual(parsed.power_short_ms, planner.VENDOR_ATX_POWER_SHORT_MS)
        self.assertEqual(parsed.power_long_ms, planner.VENDOR_ATX_POWER_LONG_MS)
        self.assertEqual(parsed.reset_ms, planner.VENDOR_ATX_RESET_MS)
        self.assertEqual(parsed.power_short_ms, 500)
        self.assertEqual(parsed.power_long_ms, 5_000)
        self.assertEqual(parsed.reset_ms, 500)

    def test_plan_steps_include_the_media_and_atx_bench_gates(self) -> None:
        plan = planner.build_plan(options(), "a" * 64)
        steps = " ".join(plan["steps"])
        self.assertIn("--accept-media-hid", steps)
        self.assertIn("--accept-atx-bench", steps)
        self.assertIn("--confirm-physical-disconnect", steps)

    def test_timings_are_bounded(self) -> None:
        for field, value in (
            ("power_short_ms", 0),
            ("power_long_ms", 10_001),
            ("reset_ms", -1),
        ):
            with self.subTest(field=field):
                with self.assertRaises(ValueError):
                    planner.build_plan(options(**{field: value}), "a" * 64)

    def test_paths_must_stay_in_the_reviewed_locations(self) -> None:
        for field, value in (
            ("images_dir", "/tmp/images"),
            ("catalog", "/var/lib/hardened-blikvm-gadget/catalog.json"),
            ("serial_file", "/run/serial"),
            ("control_socket", "/tmp/control.sock"),
            ("atx_journal", "/etc/hardened-blikvm-gadget/atx.json"),
        ):
            with self.subTest(field=field):
                with self.assertRaises(ValueError):
                    planner.build_plan(options(**{field: value}), "a" * 64)

    def test_identifiers_are_conservative(self) -> None:
        for overrides in (
            {"udc": "bad udc"},
            {"udc": ""},
            {"configfs_root": "/tmp/configfs"},
            {"hidg_dir": "/tmp"},
            {"atx_chip": "/dev/gpiochip0/../x"},
            {"atx_chip": "/dev/ttyS0"},
        ):
            with self.subTest(overrides=overrides):
                with self.assertRaises(ValueError):
                    planner.build_plan(options(**overrides), "a" * 64)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = build_bundle(Path(directory))
            output = Path("evidence/private/plan-selftest.json").resolve()
            result = subprocess.run(
                [
                    sys.executable,
                    "tools/plan_board_profile_handoff.py",
                    "--bundle",
                    str(bundle),
                    "--output",
                    str(output),
                    "--udc",
                    "musb-hdrc.2.auto",
                    "--power-short-ms",
                    "500",
                    "--power-long-ms",
                    "4000",
                    "--reset-ms",
                    "200",
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            try:
                self.assertEqual(result.returncode, 0, result.stderr.decode())
            finally:
                output.unlink(missing_ok=True)


if __name__ == "__main__":
    _ = unittest.main()
