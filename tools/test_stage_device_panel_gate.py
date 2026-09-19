import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import Any
from unittest import mock

from tools import stage_device_panel_gate as gate


def facts(phase: str, present: str = "True", initialized: str = "True", backlight: str = "True") -> str:
    return (
        f"{phase}_present={present}\n{phase}_initialized={initialized}\n"
        f"{phase}_backlight={backlight}\n"
    )


class PanelGateTests(unittest.TestCase):
    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-panel-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_panel_gate.py",
            "--endpoint",
            "ssh://192.0.2.10",
            "--username",
            "root",
            "--known-hosts",
            str(known_hosts),
            "--output",
            str(output),
            "--timeout-seconds",
            "30",
        ] + extra

    def _run(self, extra: list[str], replies: list[str]) -> dict[str, Any]:
        output, known_hosts = self._paths()
        mocks = [
            mock.Mock(stdout=reply.encode(), stderr=b"", returncode=0) for reply in replies
        ]
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=mocks),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    def test_plan_mode_observes_without_restarting(self) -> None:
        receipt = self._run([], [facts("before")])
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertIs(receipt["initializedBefore"], True)
        self.assertIs(receipt["daemonRestarted"], False)

    def test_the_panel_is_repainted_after_a_daemon_restart(self) -> None:
        receipt = self._run(
            ["--confirm"],
            [facts("before"), "test_code=0\n", "", facts("after")],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["probe"], "hardened-blikvm-panel-status-gate")
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["testPatternCode"], 0)
        self.assertIs(receipt["daemonRestarted"], True)
        self.assertIs(receipt["initializedAfter"], True)

    def test_an_uninitialized_panel_is_refused(self) -> None:
        receipt = self._run([], [facts("before", initialized="False")])
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "panel_not_initialized")

    def test_a_panel_that_stays_blank_after_restart_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            [facts("before"), "test_code=0\n", "", facts("after", backlight="False")],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "panel_not_repainted_after_restart")


if __name__ == "__main__":
    unittest.main()
