import itertools
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_boot_gate as gate


def service_facts(token: str) -> dict[str, str]:
    facts = {key: "active" for key in gate.SERVICE_FACTS if key.startswith("unit_")}
    facts.update(
        {
            "units_active": str(len(gate.PRODUCT_UNITS)),
            "failed_units": "0",
            "https_status": "200",
            "health_status": "200",
            "journal_first_line": "Sep_18_13:32:34_kernel:_start",
            "kernel_records": "120",
            "boot_token": token,
        }
    )
    return facts


def facts_output(facts: dict[str, str]) -> str:
    return "".join(f"{key}={value}\n" for key, value in facts.items())


class BootGateTests(unittest.TestCase):
    def test_service_script_is_read_only_and_covers_every_product_unit(self) -> None:
        script = gate.build_service_script()
        for forbidden in (
            "systemctl restart",
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "reboot",
            "install -m",
            "rm -",
        ):
            self.assertNotIn(forbidden, script)
        for unit in gate.PRODUCT_UNITS:
            self.assertIn(unit, script)
        for required in ("units_active=", "failed_units=", "https_status=", "health_status="):
            self.assertIn(required, script)
        self.assertIn("journalctl -b", script)

    def test_reboot_script_uses_only_the_reviewed_wrapper(self) -> None:
        script = gate.build_reboot_script()
        self.assertIn("/usr/local/sbin/hardened-blikvm-reboot", script)
        self.assertNotIn("systemctl reboot", script)
        self.assertIn("emit reboot_requested yes", script)

    def test_validate_service_facts_fails_closed(self) -> None:
        gate.validate_service_facts(service_facts("a" * 64))

        for override, failure in (
            ({"units_active": "5"}, "product_unit_inactive"),
            ({"failed_units": "1"}, "failed_unit_present"),
            ({"https_status": "502"}, "https_unhealthy"),
            ({"health_status": "503"}, "health_unhealthy"),
            ({"journal_first_line": ""}, "journal_missing"),
            ({"kernel_records": "0"}, "kernel_records_missing"),
        ):
            with self.assertRaises(gate.GateError) as error:
                gate.validate_service_facts(service_facts("a" * 64) | override)
            self.assertEqual(error.exception.failure_type, failure)

    def _run(
        self,
        extra: list[str],
        outputs: list[str],
        *,
        unreachable_after: int | None = None,
    ) -> dict[str, object]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-boot-gate-{uuid.uuid4().hex}.json"
        )
        argv = [
            "stage_device_boot_gate.py",
            "--endpoint",
            "ssh://192.0.2.10",
            "--username",
            "root",
            "--known-hosts",
            str(directory / "known-hosts"),
            "--output",
            str(output),
            "--timeout-seconds",
            "10",
        ] + extra
        replies: list[object] = [
            mock.Mock(stdout=text.encode(), stderr=b"", returncode=0) for text in outputs
        ]
        if unreachable_after is not None:
            replies = replies[:unreachable_after] + [OSError("unreachable")] * 1000
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(
                gate.prototype, "read_secret_stdin", return_value=bytearray(b"ssh-secret")
            ),
            mock.patch.object(gate.time, "sleep", self._no_sleep),
            mock.patch.object(gate.time, "monotonic", side_effect=itertools.count()),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    @staticmethod
    def _no_sleep(_: float) -> None:
        return None

    def test_reboot_gate_passes_when_the_board_returns_healthy(self) -> None:
        receipt = self._run(
            ["--confirm-reboot"],
            [
                facts_output(service_facts("old")),
                "reboot_command=present\nreboot_requested=yes\n",
                facts_output(service_facts("new")),
                "cleanup_confirmed=yes\n",
            ],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertIs(receipt["bootIdentityChanged"], True)
        self.assertIs(receipt["deviceReachableAfterReboot"], True)
        self.assertIs(receipt["unitsActive"], True)
        self.assertIs(receipt["failedUnitsEmpty"], True)
        self.assertIs(receipt["earlyJournalPresent"], True)

    def test_reboot_gate_fails_when_the_board_never_returns(self) -> None:
        receipt = self._run(
            ["--confirm-reboot"],
            [facts_output(service_facts("old")), "reboot_command=present\nreboot_requested=yes\n"],
            unreachable_after=2,
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["result"], "failed")
        self.assertEqual(receipt["failureType"], "device_unreachable_after_reboot")

    def test_reboot_gate_refuses_without_approval(self) -> None:
        receipt = self._run([], [facts_output(service_facts("old"))])
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "reboot_not_approved")

    def test_verify_only_records_the_current_state(self) -> None:
        receipt = self._run(
            ["--verify-only"],
            [facts_output(service_facts("now")), "cleanup_confirmed=yes\n"],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["deviceReachableAfterReboot"], True)
        self.assertIs(receipt["bootIdentityChanged"], False)


if __name__ == "__main__":
    unittest.main()
