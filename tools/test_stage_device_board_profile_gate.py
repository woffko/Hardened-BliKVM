import argparse
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools import plan_board_profile_handoff as planner
from tools import stage_device_board_profile_gate as gate
from tools.test_verify_board_profile_bundle import build_bundle


def plan_options() -> argparse.Namespace:
    return argparse.Namespace(
        udc="musb-hdrc.2.auto",
        power_short_ms=500,
        power_long_ms=4_000,
        reset_ms=200,
        configfs_root=planner.CONFIGFS_ROOT,
        images_dir=f"{planner.GADGET_STATE}/images",
        catalog=f"{planner.GADGET_ETC}/catalog.json",
        serial_file=f"{planner.GADGET_ETC}/serial",
        atx_journal=f"{planner.GADGET_STATE}/atx-journal.json",
        hid_ledger=f"{planner.GADGET_STATE}/hid-ledger.json",
        control_socket=f"{planner.GADGET_RUN}/control.sock",
        atx_chip="/dev/gpiochip0",
        hidg_dir=planner.HIDG_DIR,
        reconciliations="",
    )


def render() -> tuple[str, str]:
    plan = planner.build_plan(plan_options(), "a" * 64)
    return gate.build_apply_script(plan, "a" * 64), gate.build_teardown_script()


def apply_facts() -> dict[str, str]:
    return {
        "kvmd_was_active": "yes",
        "owner_started": "yes",
        "socket_present": "yes",
        "gadget_present": "yes",
        "udc_bound": "yes",
        "gadget_name": "hardened",
        "kvmd_stopped": "yes",
    }


def teardown_facts() -> dict[str, str]:
    return {
        "owner_stopped": "yes",
        "hardened_gadget_removed": "yes",
        "runtime_removed": "yes",
        "kvmd_restarted": "yes",
    }


class StageBoardProfileGateTests(unittest.TestCase):
    def test_parser_defaults_use_the_vendor_pinned_timings(self) -> None:
        argv = [
            "gate",
            "--endpoint",
            "ssh://10.0.77.118:22/",
            "--username",
            "root",
            "--known-hosts",
            "/tmp/known-hosts",
            "--bundle",
            "b",
            "--output",
            "o",
            "--udc",
            "u",
        ]
        with mock.patch.object(sys, "argv", argv):
            parsed = gate.parse_args()
        self.assertEqual(parsed.power_short_ms, planner.VENDOR_ATX_POWER_SHORT_MS)
        self.assertEqual(parsed.power_long_ms, planner.VENDOR_ATX_POWER_LONG_MS)
        self.assertEqual(parsed.reset_ms, planner.VENDOR_ATX_RESET_MS)

    def test_generated_scripts_are_valid_shell(self) -> None:
        apply, teardown = render()
        for script in (apply, teardown):
            syntax = subprocess.run(
                ["sh", "-n"],
                input=script.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())

    def test_apply_stops_kvmd_and_owns_only_the_hardened_gadget(self) -> None:
        apply, _ = render()
        self.assertIn("systemctl stop kvmd.service", apply)
        self.assertIn("BLI_KVM_BOARD_PROFILE=1", apply)
        self.assertIn("bundle sha mismatch", apply)
        self.assertIn("gadget_name", apply)
        self.assertIn(gate.MARKER, apply)
        self.assertIn('rm -f -- "$socket"', apply)
        self.assertNotIn("$gdir/kvmd", apply)
        self.assertEqual(apply.count("printf 'kvmd_stopped=yes"), 1)
        self.assertEqual(apply.count("printf 'kvmd_stopped=no"), 1)

    def test_teardown_restores_kvmd_and_removes_only_our_state(self) -> None:
        _, teardown = render()
        self.assertIn("systemctl start kvmd.service", teardown)
        self.assertIn('rm -rf -- "$root"', teardown)
        self.assertIn("$gdir/hardened", teardown)
        self.assertNotIn("$gdir/kvmd", teardown)
        self.assertIn(gate.MARKER, teardown)
        self.assertIn("rm -f -- /run/hardened-blikvm-gadget/control.sock", teardown)

    def test_fact_validation_fails_closed(self) -> None:
        gate.validate_apply(apply_facts())
        gate.validate_teardown(teardown_facts())
        for key, value in (
            ("socket_present", "no"),
            ("owner_started", "no"),
        ):
            facts = apply_facts()
            facts[key] = value
            with self.subTest(key=key):
                with self.assertRaises(ValueError):
                    gate.validate_apply(facts)
        for key in teardown_facts():
            facts = teardown_facts()
            facts[key] = "no"
            with self.subTest(key=key):
                with self.assertRaises(ValueError):
                    gate.validate_teardown(facts)

    def test_fact_parsers_accept_synthetic_output(self) -> None:
        apply, teardown = render()
        apply_raw = "".join(f"{k}={v}\n" for k, v in apply_facts().items()).encode()
        teardown_raw = "".join(f"{k}={v}\n" for k, v in teardown_facts().items()).encode()
        from tools import stage_device_prototype as prototype

        self.assertEqual(
            prototype.parse_facts(apply_raw, gate.APPLY_FACTS), dict(sorted(apply_facts().items()))
        )
        self.assertEqual(
            prototype.parse_facts(teardown_raw, gate.TEARDOWN_FACTS),
            dict(sorted(teardown_facts().items())),
        )
        self.assertTrue(apply.startswith("set -eu"))

    def test_plan_only_mode_does_not_require_a_device(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = build_bundle(Path(directory))
            output = Path("evidence/private/board-gate-selftest.json").resolve()
            result = subprocess.run(
                [
                    sys.executable,
                    "tools/stage_device_board_profile_gate.py",
                    "--endpoint",
                    "ssh://127.0.0.1:22",
                    "--username",
                    "root",
                    "--known-hosts",
                    "/dev/null",
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
                self.assertIn(b"plan_only", result.stdout)
            finally:
                output.unlink(missing_ok=True)


if __name__ == "__main__":
    _ = unittest.main()
