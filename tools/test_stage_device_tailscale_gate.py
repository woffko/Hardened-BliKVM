import collections.abc
import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_tailscale_gate as gate


STATUS = {
    "installed": False,
    "running": False,
    "enabled": False,
    "message": "the tailscale package is not installed",
}


def refusal() -> dict[str, object]:
    return {"applied": False, "rejection": "capability_disabled"}


class TailscaleGateTests(unittest.TestCase):
    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-tailscale-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_tailscale_gate.py",
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
        action_results: collections.abc.Sequence[collections.abc.Mapping[str, object]] | None = None,
    ) -> dict[str, object]:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.status.return_value = STATUS
        api.action.side_effect = action_results or [refusal()] * len(gate.ACTIONS)
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "TailscaleApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    def test_plan_mode_observes_the_state_without_acting(self) -> None:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.status.return_value = STATUS
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, [])),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(gate, "TailscaleApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["installed"], False)
        self.assertIs(receipt["mutationsPerformed"], False)
        api.action.assert_not_called()

    def test_every_reviewed_action_is_refused(self) -> None:
        receipt = self._run(["--confirm"])
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["probe"], "hardened-blikvm-tailscale-gate")
        self.assertEqual(receipt["actionsAttempted"], len(gate.ACTIONS))
        self.assertEqual(receipt["actionsRefused"], len(gate.ACTIONS))
        self.assertEqual(receipt["firstRejection"], "capability_disabled")
        self.assertIs(receipt["mutationsPerformed"], False)

    def test_an_admitted_action_fails_closed(self) -> None:
        results = [refusal()] * 3 + [{"applied": True, "rejection": "x"}]
        receipt = self._run(["--confirm"], action_results=results)
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureStage"], "actions")
        self.assertEqual(receipt["failureType"], "restart_admitted")

    def test_a_missing_rejection_fails_closed(self) -> None:
        results = [{"applied": False}] + [refusal()] * len(gate.ACTIONS)
        receipt = self._run(["--confirm"], action_results=results)
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "install_no_rejection")


if __name__ == "__main__":
    unittest.main()
