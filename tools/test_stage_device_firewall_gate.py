import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import Any
from unittest import mock

from tools import stage_device_firewall_gate as gate


STATUS = {
    "backendAvailable": True,
    "nftInstalled": True,
    "mode": "disabled",
    "baselineActive": False,
}


class FirewallGateTests(unittest.TestCase):
    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-firewall-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_firewall_gate.py",
            "--endpoint",
            "ssh://192.0.2.10",
            "--username",
            "root",
            "--known-hosts",
            str(known_hosts),
            "--output",
            str(output),
            "--base-url",
            "https://192.0.2.10",
            "--api-username",
            "admin",
            "--timeout-seconds",
            "30",
        ] + extra

    def _run(
        self,
        extra: list[str],
        *,
        statuses: list[dict[str, Any]],
        set_results: list[dict[str, Any]] | None = None,
    ) -> dict[str, Any]:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.status.side_effect = statuses + [statuses[-1]] * 8
        api.set_mode.side_effect = set_results or [{"applied": True}]
        api.confirm.return_value = {"applied": True}
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "FirewallApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    def test_plan_mode_observes_without_applying(self) -> None:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.status.return_value = STATUS
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, [])),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(gate, "FirewallApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        api.set_mode.assert_not_called()

    def test_the_baseline_is_applied_and_restored(self) -> None:
        receipt = self._run(
            ["--confirm"],
            statuses=[
                STATUS,
                {"backendAvailable": True, "nftInstalled": True, "baselineActive": True},
                {"backendAvailable": True, "nftInstalled": True, "baselineActive": False},
            ],
            set_results=[{"applied": True}, {"applied": True}],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["probe"], "hardened-blikvm-firewall-gate")
        self.assertEqual(receipt["result"], "passed")
        self.assertIs(receipt["applied"], True)
        self.assertIs(receipt["confirmed"], True)
        self.assertEqual(receipt["initialMode"], "disabled")
        self.assertEqual(receipt["restoredMode"], "disabled")

    def test_a_rejected_apply_fails_closed_and_restores(self) -> None:
        receipt = self._run(
            ["--confirm"],
            statuses=[STATUS],
            set_results=[{"applied": False, "rejection": "capability_disabled"}],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureStage"], "apply")
        self.assertEqual(receipt["failureType"], "capability_disabled")

    def test_a_baseline_that_does_not_activate_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            statuses=[STATUS, STATUS],
            set_results=[{"applied": True}],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "baseline_not_active")

    def test_an_already_active_baseline_is_left_active(self) -> None:
        active = {"backendAvailable": True, "nftInstalled": True, "mode": "baseline", "baselineActive": True}
        receipt = self._run(
            ["--confirm"],
            statuses=[active, active],
            set_results=[{"applied": True}],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["restoredMode"], "baseline")
        self.assertEqual(receipt["initialMode"], "baseline")


if __name__ == "__main__":
    unittest.main()
