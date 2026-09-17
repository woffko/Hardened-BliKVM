import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_boardd_gate as gate


class BoarddGateTests(unittest.TestCase):
    def test_plan_script_never_mutates(self) -> None:
        for subsystem in gate.SUBSYSTEMS:
            script = gate.build_preflight_script(subsystem)
            for forbidden in ("unexport", "install -m", "systemctl restart", "base64 -d"):
                self.assertNotIn(forbidden, script, subsystem)

    def test_apply_script_backs_up_and_hands_over_one_subsystem(self) -> None:
        script = gate.build_apply_script("fan", "a" * 64, {"socket": "/run/x.sock"}, True)
        self.assertIn("exports_backed_up=yes", script)
        self.assertIn("> /sys/class/gpio/unexport", script)
        self.assertIn("install -m 0755", script)
        self.assertIn(gate.CONFIG_PATH, script)
        self.assertIn("systemctl restart", script)
        self.assertIn("fan_duty_set", script)
        self.assertIn("status_code=", script)
        # Only the subsystem's own lines are released.
        self.assertIn("for n in 269", script)
        self.assertNotIn("for n in 269 261", script)

    def test_teardown_restores_the_recorded_exports(self) -> None:
        script = gate.build_teardown_script("fan")
        self.assertIn("systemctl stop", script)
        self.assertIn("systemctl disable", script)
        self.assertIn("/sys/class/gpio/export", script)
        self.assertIn("exports_restored=yes", script)

    def test_the_atx_leds_entry_is_read_only_and_reports_both_inputs(self) -> None:
        spec = gate.SUBSYSTEMS["atx-leds"]
        self.assertEqual(spec["lines"], (234, 233))
        self.assertEqual(spec["requests"], ())
        self.assertIn("atxPowerLedOn", spec["status_fields"])
        self.assertIn("atxHddLedOn", spec["status_fields"])

    def test_facts_parse_ignores_indented_output(self) -> None:
        facts = gate.parse_facts("  line269 present\nunit_active=active\nbad key=1\n")
        self.assertEqual(facts, {"unit_active": "active"})

    def _run(self, extra: list[str], stdout: str, expected_code: int = 0) -> dict:
        directory = Path(tempfile.mkdtemp())
        bundle = directory / "bundle.tar.gz"
        bundle.write_bytes(b"bundle")
        config = directory / "boardd.json"
        config.write_text('{"socket": "/run/boardd.sock"}', encoding="utf-8")
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence"
            / "private"
            / f"boardd-gate-test-{uuid.uuid4().hex}.json"
        )
        self.addCleanup(lambda: output.unlink(missing_ok=True))
        argv = [
            "stage_device_boardd_gate.py",
            "--endpoint",
            "ssh://127.0.0.1:22/",
            "--username",
            "root",
            "--known-hosts",
            str(directory / "known-hosts"),
            "--output",
            str(output),
            "--bundle",
            str(bundle),
            "--expected-sha256",
            "a" * 64,
            "--config",
            str(config),
        ] + extra
        fake = mock.Mock(stdout=stdout.encode(), stderr=b"")
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(gate.prototype, "run_ssh", return_value=fake),
            mock.patch.object(gate.prototype, "read_secret_stdin", return_value=b"x"),
        ):
            code = gate.main()
        self.assertEqual(code, expected_code)
        return json.loads(output.read_text(encoding="utf-8"))

    def test_handoff_needs_the_operator_observation_to_pass(self) -> None:
        facts = "\n".join(
            [
                "exports_backed_up=yes",
                "config_installed=yes",
                "unit_active=active",
                "status_code=0",
            ]
        )
        pending = self._run(
            ["--subsystem", "led", "--confirm-handoff"], facts
        )
        self.assertEqual(pending["mode"], "handoff")
        self.assertEqual(pending["result"], "observed-pending")
        self.assertIs(pending["policy"]["mutationsPerformed"], True)

        passed = self._run(
            ["--subsystem", "led", "--confirm-handoff", "--confirm-observed"], facts
        )
        self.assertEqual(passed["result"], "passed")
        self.assertIs(passed["operatorObserved"], True)

    def test_missing_facts_stay_inspect(self) -> None:
        receipt = self._run(
            ["--subsystem", "fan", "--confirm-handoff"], "unexpected=1", expected_code=1
        )
        self.assertEqual(receipt["result"], "inspect")

    def test_plan_mode_reports_plan(self) -> None:
        receipt = self._run(["--subsystem", "panel"], "unit_active=inactive")
        self.assertEqual(receipt["mode"], "plan")
        self.assertEqual(receipt["result"], "plan")
        self.assertIs(receipt["policy"]["mutationsPerformed"], False)


if __name__ == "__main__":
    unittest.main()
