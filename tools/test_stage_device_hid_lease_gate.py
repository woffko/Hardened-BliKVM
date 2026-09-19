import collections.abc
import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_hid_lease_gate as gate


def lease(applied: bool, lease_id: str = "", rejection: str = "") -> dict[str, object]:
    payload: dict[str, object] = {"applied": applied}
    if lease_id:
        payload["leaseId"] = lease_id
    if rejection:
        payload["rejection"] = rejection
    return payload


class HidLeaseGateTests(unittest.TestCase):
    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-hid-lease-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_hid_lease_gate.py",
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
        first_leases: list[dict[str, object]] | None = None,
        second_acquire: dict[str, object] | None = None,
        reports: collections.abc.Sequence[object] | None = None,
    ) -> dict[str, object]:
        output, known_hosts = self._paths()
        first = mock.Mock()
        first.platform.return_value = "bli-h616-pikvm"
        first.lease.side_effect = first_leases or [
            lease(True, "aaaa"),
            lease(True, "bbbb"),
            lease(True),
        ]
        first.report.side_effect = reports or [{"applied": True}] * 5
        second = mock.Mock()
        second.lease.return_value = second_acquire or lease(False, rejection="lease_conflict")
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardSession", side_effect=[first, second]),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    def test_plan_mode_observes_without_leasing(self) -> None:
        output, known_hosts = self._paths()
        first = mock.Mock()
        first.platform.return_value = "bli-h616-pikvm"
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, [])),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(gate, "BoardSession", return_value=first),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        first.lease.assert_not_called()

    def test_both_sessions_receive_the_operator_password(self) -> None:
        output, known_hosts = self._paths()
        created: list[str] = []

        def make_session(_options: object, password: str) -> mock.Mock:
            second_session = bool(created)
            created.append(password)
            session = mock.Mock()
            session.platform.return_value = "bli-h616-pikvm"
            if second_session:
                session.lease.return_value = lease(
                    False, rejection="lease_conflict"
                )
            else:
                session.lease.side_effect = [
                    lease(True, "aaaa"),
                    lease(True, "bbbb"),
                    lease(True),
                ]
                session.report.return_value = {"applied": True}
            return session

        with (
            mock.patch.object(
                sys, "argv", self._argv(output, known_hosts, ["--confirm"])
            ),
            mock.patch.object(
                sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")
            ),
            mock.patch.object(gate, "BoardSession", side_effect=make_session),
        ):
            code = gate.main()
        self.assertEqual(code, 0)
        self.assertEqual(created, ["operator-password", "operator-password"])

    def test_own_lease_is_superseded_and_a_second_session_is_refused(self) -> None:
        receipt = self._run(["--confirm"])
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["probe"], "hardened-blikvm-hid-lease-gate")
        self.assertEqual((receipt["firstLeaseId"], receipt["secondLeaseId"]), ("aaaa", "bbbb"))
        self.assertIs(receipt["superseded"], True)
        self.assertEqual(receipt["reportsApplied"], 5)
        self.assertIs(receipt["secondSessionRefused"], True)
        self.assertEqual(receipt["secondSessionRejection"], "lease_conflict")

    def test_a_repeated_lease_id_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            first_leases=[lease(True, "aaaa"), lease(True, "aaaa")],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureStage"], "supersede")
        self.assertEqual(receipt["failureType"], "lease_id_not_rotated")

    def test_a_refused_second_acquire_for_the_same_session_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            first_leases=[lease(True, "aaaa"), lease(False, rejection="lease_conflict")],
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "supersede_lease_conflict")

    def test_a_rejected_report_fails_closed(self) -> None:
        receipt = self._run(
            ["--confirm"],
            reports=[{"applied": True}] + [
                gate.GateError("report", "report_failed_3")
            ] * 5,
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "report_rejected")

    def test_an_admitted_second_session_fails_closed(self) -> None:
        receipt = self._run(["--confirm"], second_acquire=lease(True, "cccc"))
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "second_session_admitted")


if __name__ == "__main__":
    unittest.main()
