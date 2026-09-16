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
        "kvmd_udc_unbound": "yes",
        "kvmd_gadget": "absent",
        "udc_bound": "no",
        "udc_bound_by": "none",
    }


def teardown_facts() -> dict[str, str]:
    return {
        "owner_stopped": "yes",
        "hardened_gadget_removed": "yes",
        "runtime_removed": "yes",
        "kvmd_restarted": "yes",
        "kvmd_udc_bound": "yes",
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

    def acceptance_facts(self, **overrides: str) -> dict[str, str]:
        base = {
            "gadget_present": "yes",
            "gadget_name": "hardened",
            "udc_bound": "yes",
            "functions": "hid.usb0,hid.usb1,hid.usb2,mass_storage.usb0",
            "msd_file": "kubuntu.img",
            "hidg_devices": "/dev/hidg0,/dev/hidg1,/dev/hidg2",
            "hid_write": "ok",
        }
        base.update(overrides)
        return base

    def test_acceptance_script_pins_the_timeout_and_image(self) -> None:
        script = gate.build_acceptance_script(120, "kubuntu.img")
        self.assertIn("while [ \"$waited\" -lt 120 ]", script)
        self.assertIn("expected=kubuntu.img", script)
        self.assertIn("/dev/hidg0", script)
        with self.assertRaises(ValueError):
            gate.build_acceptance_script(0, "kubuntu.img")
        with self.assertRaises(ValueError):
            gate.build_acceptance_script(601, "kubuntu.img")
        with self.assertRaises(ValueError):
            gate.build_acceptance_script(60, "bad name.img")

    def test_acceptance_validation_accepts_the_composite_gadget(self) -> None:
        gate.validate_acceptance(self.acceptance_facts(), "kubuntu.img")
        gate.validate_acceptance(
            self.acceptance_facts(hid_write="skipped"), "kubuntu.img"
        )

    def test_acceptance_validation_fails_closed(self) -> None:
        cases = (
            {"gadget_present": "no"},
            {"gadget_name": "kvmd"},
            {"udc_bound": "no"},
            {"functions": "mass_storage.usb0"},
            {"functions": "hid.usb0,hid.usb1,hid.usb2,mass_storage.usb0,hid.usb9"},
            {"msd_file": "other.img"},
            {"hidg_devices": "none"},
            {"hidg_devices": "/dev/ttyGS0"},
            {"hid_write": "failed"},
        )
        for override in cases:
            with self.subTest(override=override):
                with self.assertRaises(ValueError):
                    gate.validate_acceptance(
                        self.acceptance_facts(**override), "kubuntu.img"
                    )

    def atx_bench_facts(self, **overrides: str) -> dict[str, str]:
        base = {"run_rc": "0"}
        for action, expected in (
            ("power_short", "500"),
            ("power_long", "5000"),
            ("reset", "500"),
        ):
            base[f"{action}_expected_ms"] = expected
            base[f"{action}_elapsed_ms"] = expected
            base[f"{action}_within_tolerance"] = "yes"
            base[action] = "ok"
        base.update(overrides)
        return base

    def test_udc_takeover_is_optional_and_verified_both_ways(self) -> None:
        environment = {
            "ownerEnvironment": {
                "BLI_KVM_CONTROL_SOCKET": "/run/x.sock",
                "BLI_KVM_GADGET_UDC": "musb-hdrc.2.auto",
            },
            "atxEnvironmentFile": {},
        }
        plain = gate.build_apply_script(environment, "a" * 64)
        takeover = gate.build_apply_script(environment, "a" * 64, takeover_udc=True)
        self.assertNotIn("kvmd_udc_unbound=yes", plain)
        self.assertIn("kvmd_udc_unbound=absent", plain)
        self.assertIn("gdir0=/sys/kernel/config/usb_gadget", takeover)
        self.assertLess(
            takeover.index("gdir0=/sys/kernel/config/usb_gadget"),
            takeover.index('"$gdir0/kvmd/UDC"'),
            "the takeover must define its configfs root before using it",
        )
        self.assertIn('printf \'\' > "$gdir0/kvmd/UDC"', takeover)
        self.assertIn("kvmd_udc_unbound=yes", takeover)

        for script in (plain, takeover, gate.build_teardown_script()):
            syntax = subprocess.run(
                ["sh", "-n"],
                input=script.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())

        gate.validate_apply(
            {"socket_present": "yes", "owner_started": "yes", "kvmd_udc_unbound": "yes"},
            takeover_udc=True,
        )
        with self.assertRaises(ValueError):
            gate.validate_apply(
                {"socket_present": "yes", "owner_started": "yes", "kvmd_udc_unbound": "no"},
                takeover_udc=True,
            )
        self.assertIn("kvmd_udc_bound=yes", gate.build_teardown_script())

    def test_atx_bench_fact_keys_match_what_the_binary_prints(self) -> None:
        for action in gate.ATX_BENCH_ACTIONS:
            self.assertIn(action, gate.ATX_BENCH_FACTS)
            self.assertIn(f"{action}_elapsed_ms", gate.ATX_BENCH_FACTS)
            self.assertIn(f"{action}_expected_ms", gate.ATX_BENCH_FACTS)
            self.assertIn(f"{action}_within_tolerance", gate.ATX_BENCH_FACTS)
            self.assertNotIn(f"{action}_", gate.ATX_BENCH_FACTS)
        self.assertIn("run_rc", gate.ATX_BENCH_FACTS)

    def test_atx_bench_script_pins_the_binary_and_timing(self) -> None:
        script = gate.build_atx_bench_script("/dev/gpiochip0", (500, 5000, 500))
        self.assertIn("BLI_KVM_ATX_BENCH_CONFIRM=1", script)
        self.assertIn("BLI_KVM_ATX_POWER_SHORT_MS=500", script)
        self.assertIn("BLI_KVM_ATX_POWER_LONG_MS=5000", script)
        self.assertIn("bli-kvm-atx-gpio-bench-gate", script)
        with self.assertRaises(ValueError):
            gate.build_atx_bench_script("/dev/gpiochip7; rm -rf /", (500, 5000, 500))
        with self.assertRaises(ValueError):
            gate.build_atx_bench_script("/dev/gpiochip0", (0, 5000, 500))

    def test_atx_bench_validation_accepts_matching_pulses_and_rejects_drift(self) -> None:
        gate.validate_atx_bench(self.atx_bench_facts())
        with self.assertRaises(ValueError):
            gate.validate_atx_bench(
                self.atx_bench_facts(power_long="drift", power_long_within_tolerance="no")
            )
        with self.assertRaises(ValueError):
            gate.validate_atx_bench(self.atx_bench_facts(run_rc="1"))

    def test_generated_scripts_are_valid_shell(self) -> None:
        apply, teardown = render()
        acceptance = gate.build_acceptance_script(
            gate.DEFAULT_ACCEPT_TIMEOUT_SECONDS, "kubuntu.img"
        )
        bench = gate.build_atx_bench_script("/dev/gpiochip0", (500, 5000, 500))
        for script in (apply, teardown, acceptance, bench):
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
        self.assertIn(
            "systemctl start kvmd-main.service kvmd-otg.service kvmd.service", teardown
        )
        self.assertIn("kvmd_udc_bound=yes", teardown)
        self.assertIn('rm -rf -- "$root"', teardown)
        self.assertIn("$gdir/hardened", teardown)
        # The teardown only reads the kvmd gadget to prove it is bound again; it
        # must never write to it or remove anything of KVMD's.
        self.assertNotIn('> "$gdir/kvmd', teardown)
        self.assertNotIn("rm -f -- \"$gdir/kvmd", teardown)
        self.assertIn('$gdir/kvmd/UDC', teardown)
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
