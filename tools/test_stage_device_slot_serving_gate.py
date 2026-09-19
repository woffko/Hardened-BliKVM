import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import Any
from unittest import mock

from tools import stage_device_slot_serving_gate as gate


INSTALLED = "/usr/libexec/hardened-blikvm/bli-kvm-server"
SLOT_ROOT = "/var/lib/hardened-blikvm-slots-v2"
SLOT_EXE = SLOT_ROOT + "/current/bin/bli-kvm-server"
SLOT_ID = "app-00000000000000000013-deadbeef"
SLOT_EXE_REAL = SLOT_ROOT + "/" + SLOT_ID + "/bin/bli-kvm-server"


def facts(phase: str, **overrides: str) -> str:
    base = {
        "slotRoot": SLOT_ROOT,
        "slotExists": "True",
        "slotBinarySha": "abc",
        "installedBinarySha": "abc",
        "slotWebSha": "web",
        "installedWebSha": "web",
        "slotExeReal": SLOT_EXE_REAL,
        "installedExeReal": INSTALLED,
        "serverActive": "active",
        "serverEnabled": "enabled",
        "serverPid": "1234",
        "serverExe": INSTALLED,
        "dropInPresent": "False",
        "confirmedSlot": SLOT_ID,
        "currentTarget": SLOT_ID,
        "healthOk": "True",
    }
    base.update(overrides)
    return "".join(f"{phase}_{key}={value}\n" for key, value in base.items())


class SlotServingGateTests(unittest.TestCase):
    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-slot-serving-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_slot_serving_gate.py",
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

    def test_plan_mode_observes_without_mutating(self) -> None:
        receipt = self._run([], [facts("before")])
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertEqual(receipt["confirmedSlot"], SLOT_ID)

    def test_confirm_switches_to_the_confirmed_slot(self) -> None:
        receipt = self._run(
            ["--confirm"],
            [
                facts("before"),
                facts("after", serverExe=SLOT_EXE_REAL, dropInPresent="True", serverPid="4321"),
            ],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["probe"], "hardened-blikvm-slot-serving-gate")
        self.assertEqual(receipt["serverExeAfter"], SLOT_EXE_REAL)
        self.assertIs(receipt["serverRestarted"], True)
        self.assertIs(receipt["dropInPresentAfter"], True)

    def test_a_slot_that_differs_from_the_installed_binary_is_refused(self) -> None:
        receipt = self._run([], [facts("before", slotBinarySha="other")])
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "slot_binary_differs")

    def test_a_current_that_is_not_the_confirmed_slot_is_refused(self) -> None:
        receipt = self._run([], [facts("before", currentTarget="app-old")])
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "current_is_not_confirmed")

    def test_rollback_restores_the_installed_server(self) -> None:
        receipt = self._run(
            ["--rollback"],
            [
                facts("before", serverExe=SLOT_EXE_REAL, dropInPresent="True"),
                facts("after", serverExe=INSTALLED, dropInPresent="False"),
            ],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "rolled_back")
        self.assertIs(receipt["dropInPresentAfter"], False)

    def test_health_failure_after_switch_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            [
                facts("before"),
                facts("after", serverExe=SLOT_EXE_REAL, dropInPresent="True", healthOk="False"),
            ],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "health_unavailable_after")


if __name__ == "__main__":
    unittest.main()
