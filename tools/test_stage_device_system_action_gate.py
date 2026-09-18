import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_system_action_gate as gate


class SystemActionGateTests(unittest.TestCase):
    def test_plan_script_is_read_only(self) -> None:
        script = gate.preflight_script()
        for forbidden in (
            "systemctl restart",
            "systemctl start",
            "systemctl enable",
            "install -m",
            "unlink",
            "rm -",
        ):
            self.assertNotIn(forbidden, script)
        for required in (
            "owner_unit=",
            "control_socket=present",
            "journal_a=present",
            "armed=yes",
            "reboot_command=set",
            "boot_id=",
        ):
            self.assertIn(required, script)

    def test_scripts_never_attest_guards_and_restart_only_the_owner(self) -> None:
        for script in (
            gate.preflight_script(),
            gate.restart_script(),
            gate.post_boot_script(),
            gate.accept_script(),
        ):
            # The reviewed acceptance runs in the owner, which holds the
            # journal; the tool never shells out to the maintenance binary.
            self.assertNotIn("accept-post-boot", script)
            self.assertNotIn("systemctl start --wait", script)
        restart = gate.restart_script()
        self.assertIn(f"systemctl restart {gate.UNIT}", restart)
        self.assertIn("observe_before=", restart)
        self.assertIn("observe_after=", restart)
        self.assertIn('"guardsReady": False', restart)
        accept = gate.accept_script()
        self.assertIn('"type": "accept_post_boot"', accept)
        self.assertIn('"guardsReady": True', accept)
        self.assertIn("store_observation=", accept)

    def test_facts_parse_ignores_unexpected_lines(self) -> None:
        facts = gate.parse_facts("  indented=1\nowner_unit=active\nbad key=1\n")
        self.assertEqual(facts, {"owner_unit": "active"})

    def test_outcome_table(self) -> None:
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=False,
                confirm_reboot=False,
                confirm_observed=False,
                preflight={},
                phase={},
            ),
            "plan",
        )
        persisted = {
            "observe_before": "pending",
            "observe_after": "pending",
            "owner_unit": "active",
        }
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=True,
                confirm_reboot=False,
                confirm_observed=False,
                preflight={},
                phase=persisted,
            ),
            "restart-persisted",
        )
        lost = dict(persisted, observe_after="none")
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=True,
                confirm_reboot=False,
                confirm_observed=False,
                preflight={},
                phase=lost,
            ),
            "inspect",
        )
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=False,
                confirm_reboot=True,
                confirm_observed=False,
                preflight={},
                phase={},
            ),
            "dispatching",
        )
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=False,
                confirm_reboot=True,
                confirm_observed=True,
                preflight={},
                phase={"store_observation": "accepted"},
            ),
            "passed",
        )
        self.assertEqual(
            gate.decide_outcome(
                confirm_restart=False,
                confirm_reboot=True,
                confirm_observed=True,
                preflight={},
                phase={"store_observation": "waiting"},
            ),
            "inspect",
        )

    def test_dispatch_rejects_plain_http_for_a_remote_board(self) -> None:
        with self.assertRaises(ValueError):
            gate.dispatch_reboot(
                "http://192.0.2.10", "admin", "password", None, None
            )

    def _run(self, extra: list[str], outputs: list[str], expected_code: int = 0) -> dict:
        directory = Path(tempfile.mkdtemp())
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence"
            / "private"
            / f"system-action-gate-test-{uuid.uuid4().hex}.json"
        )
        self.addCleanup(lambda: output.unlink(missing_ok=True))
        argv = [
            "stage_device_system_action_gate.py",
            "--endpoint",
            "ssh://127.0.0.1:22/",
            "--username",
            "root",
            "--known-hosts",
            str(directory / "known-hosts"),
            "--output",
            str(output),
        ] + extra
        results = [
            mock.Mock(stdout=text.encode(), stderr=b"") for text in outputs
        ]
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=results),
            mock.patch.object(
                sys,
                "stdin",
                io.BytesIO(b"ssh-secret\noperator-password\n"),
            ),
        ):
            code = gate.main()
        self.assertEqual(code, expected_code)
        return json.loads(output.read_text(encoding="utf-8"))

    def test_plan_mode_records_the_device_facts(self) -> None:
        receipt = self._run(
            [],
            ["owner_unit=active\narmed=no\nboot_id=abc\n"],
        )
        self.assertEqual(receipt["mode"], "plan")
        self.assertEqual(receipt["result"], "plan")
        self.assertIs(receipt["policy"]["mutationsPerformed"], False)
        self.assertEqual(receipt["preflight"]["owner_unit"], "active")

    def test_restart_mode_proves_persistence(self) -> None:
        facts = "\n".join(
            [
                "observe_before=pending",

                "restart_started=yes",
                "owner_unit=active",
                "observe_after=pending",

                "boot_id=abc",
            ]
        )
        receipt = self._run(["--confirm-restart"], [facts])
        self.assertEqual(receipt["mode"], "restart")
        self.assertEqual(receipt["result"], "restart-persisted")
        self.assertEqual(receipt["result"], "restart-persisted")
        self.assertIs(receipt["policy"]["mutationsPerformed"], True)

    def test_accept_mode_passes_only_when_the_store_reports_closure(self) -> None:
        preflight = "owner_unit=active\nboot_id=old\n"
        boot = (
            "boot_id=new\nowner_unit=active\nserver_unit=active\nnginx_unit=active\n"
            "boardd_unit=active\nhealth_code=200\n"
        )
        accept = (
            "accept_response=applied\nstore_observation=accepted\n"
            "replay_observation=accepted\n"
        )
        receipt = self._run(
            ["--confirm-observed", "--expect-operation", "c" * 32],
            [preflight, boot, accept],
        )
        self.assertEqual(receipt["mode"], "accept")
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["bootIdBefore"], "old")
        self.assertEqual(receipt["bootIdAfter"], "new")
        self.assertIs(receipt["operatorObserved"], True)

        waiting = "accept_response=rejected\nstore_observation=rejected\nreplay_observation=rejected\n"
        receipt = self._run(
            ["--confirm-observed", "--expect-operation", "c" * 32],
            [preflight, boot, waiting],
            expected_code=1,
        )
        self.assertEqual(receipt["result"], "inspect")

    def test_operation_id_is_validated(self) -> None:
        with self.assertRaises(SystemExit):
            self._run(["--confirm-observed", "--expect-operation", "not-hex"], [])


if __name__ == "__main__":
    unittest.main()
