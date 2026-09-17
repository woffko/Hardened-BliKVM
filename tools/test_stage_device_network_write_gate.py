import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_network_write_gate as gate


class NetworkWriteGateTests(unittest.TestCase):
    def test_preflight_script_is_read_only(self) -> None:
        script = gate.preflight_script("wlan0")
        for forbidden in ("systemctl restart", "systemctl start", "tee ", " > /", "install -m"):
            self.assertNotIn(forbidden, script)
        for required in (
            "server_unit=",
            "writes_enabled=",
            "resolver=",
            "ipv6_disable_before=",
            "boot_id=",
        ):
            self.assertIn(required, script)

    def test_facts_parse_ignores_unexpected_lines(self) -> None:
        facts = gate.parse_facts(" indented=1\nserver_unit=active\nbad key=1\n")
        self.assertEqual(facts, {"server_unit": "active"})

    def test_the_plan_restores_what_it_observed(self) -> None:
        plan = gate.plan_writes(["9.9.9.9"], "wlan0", "0", None)
        self.assertEqual(plan["dns"]["servers"], [gate.REVIEWED_RESOLVER])
        self.assertEqual(plan["dns"]["restore"], ["9.9.9.9"])
        self.assertIs(plan["dns"]["changed"], True)
        self.assertEqual(plan["ipv6"]["interface"], "wlan0")
        self.assertIs(plan["ipv6"]["disabled"], True)
        self.assertIs(plan["ipv6"]["restore_disabled"], False)
        unchanged = gate.plan_writes([gate.REVIEWED_RESOLVER], "wlan0", "1", None)
        self.assertIs(unchanged["dns"]["changed"], False)
        self.assertIs(unchanged["ipv6"]["disabled"], False)
        self.assertIs(unchanged["ipv6"]["restore_disabled"], True)

    def test_the_plan_records_the_optional_wake_on_lan_target(self) -> None:
        without = gate.plan_writes(["9.9.9.9"], "wlan0", "0", None)
        self.assertIs(without["wol"]["requested"], False)
        with_mac = gate.plan_writes(["9.9.9.9"], "wlan0", "0", "00:11:22:aa:bb:cc")
        self.assertIs(with_mac["wol"]["requested"], True)
        self.assertEqual(with_mac["wol"]["mac"], "00:11:22:aa:bb:cc")

    def test_outcome_table(self) -> None:
        self.assertEqual(
            gate.decide_outcome(confirm_write=False, preflight={}, write={}), "plan"
        )
        self.assertEqual(
            gate.decide_outcome(confirm_write=True, preflight={}, write={}), "preflight"
        )
        complete = {
            "dns_applied": True,
            "dns_restored": True,
            "ipv6_applied": True,
            "ipv6_restored": True,
            "wol_applied": None,
        }
        self.assertEqual(
            gate.decide_outcome(confirm_write=True, preflight={}, write=complete), "passed"
        )
        partial = dict(complete, ipv6_restored=False)
        self.assertEqual(
            gate.decide_outcome(confirm_write=True, preflight={}, write=partial), "partial"
        )
        nothing = dict.fromkeys(complete, False)
        self.assertEqual(
            gate.decide_outcome(confirm_write=True, preflight={}, write=nothing), "inspect"
        )

    def test_a_write_run_needs_both_stdin_lines(self) -> None:
        with mock.patch.object(sys, "stdin", io.BytesIO(b"only-the-ssh-secret\n")):
            with self.assertRaises(SystemExit):
                gate.read_credentials(confirm_write=True)
        with mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")):
            secret, password = gate.read_credentials(confirm_write=False)
        self.assertEqual(bytes(secret), b"ssh-secret")
        self.assertEqual(password, "")
        with mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")):
            with self.assertRaises(SystemExit):
                gate.read_credentials(confirm_write=True)

    def test_a_reviewed_wol_target_must_be_a_mac(self) -> None:
        with self.assertRaises(SystemExit):
            self._run(["--wol-mac", "not-a-mac"], "writes_enabled=no")

    def test_plain_http_is_refused_for_a_remote_board(self) -> None:
        with self.assertRaises(ValueError):
            gate.api_write(
                "http://10.0.77.118",
                "/api/network/dns",
                {"servers": [], "confirm": True},
                "password",
                None,
                None,
            )

    def _run(
        self,
        extra: list[str],
        stdout: str,
        applied: bool = True,
        expected_code: int = 0,
    ) -> dict:
        directory = Path(tempfile.mkdtemp())
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence"
            / "private"
            / f"network-write-gate-test-{uuid.uuid4().hex}.json"
        )
        self.addCleanup(lambda: output.unlink(missing_ok=True))
        argv = [
            "stage_device_network_write_gate.py",
            "--endpoint",
            "ssh://127.0.0.1:22/",
            "--username",
            "root",
            "--known-hosts",
            str(directory / "known-hosts"),
            "--output",
            str(output),
        ] + extra
        fake = mock.Mock(stdout=stdout.encode(), stderr=b"")
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(gate.prototype, "run_ssh", return_value=fake),
            mock.patch.object(
                sys,
                "stdin",
                io.BytesIO(b"ssh-secret\noperator-password\n"),
            ),
            mock.patch.object(
                gate, "api_write", return_value={"applied": applied, "status": 200}
            ),
        ):
            code = gate.main()
        self.assertEqual(code, expected_code)
        return json.loads(output.read_text(encoding="utf-8"))

    def test_plan_mode_records_the_device_facts(self) -> None:
        facts = "\n".join(
            [
                "server_unit=active",
                "owner_unit=active",
                "writes_enabled=yes",
                "resolver=9.9.9.9",
                "ipv6_interface=wlan0",
                "ipv6_present=yes",
                "ipv6_disable_before=0",
            ]
        )
        receipt = self._run([], facts)
        self.assertEqual(receipt["mode"], "plan")
        self.assertEqual(receipt["result"], "plan")
        self.assertIs(receipt["policy"]["mutationsPerformed"], False)
        self.assertEqual(receipt["plan"]["dns"]["restore"], ["9.9.9.9"])

    def test_write_mode_passes_when_every_write_and_restore_applies(self) -> None:
        facts = "\n".join(
            [
                "server_unit=active",
                "owner_unit=active",
                "writes_enabled=yes",
                "resolver=9.9.9.9",
                "ipv6_interface=wlan0",
                "ipv6_present=yes",
                "ipv6_disable_before=0",
            ]
        )
        receipt = self._run(
            ["--base-url", "https://10.0.77.118", "--confirm-write"], facts
        )
        self.assertEqual(receipt["mode"], "write")
        self.assertEqual(receipt["result"], "passed")
        self.assertIs(receipt["policy"]["mutationsPerformed"], True)

        failed = self._run(
            ["--base-url", "https://10.0.77.118", "--confirm-write"],
            facts,
            applied=False,
            expected_code=1,
        )
        self.assertEqual(failed["result"], "inspect")

    def test_write_mode_requires_the_reviewed_prerequisites(self) -> None:
        disabled = "\n".join(["writes_enabled=no", "ipv6_present=yes"])
        with self.assertRaises(SystemExit):
            self._run(["--base-url", "https://10.0.77.118", "--confirm-write"], disabled)
        missing_ipv6 = "\n".join(["writes_enabled=yes", "ipv6_present=no"])
        with self.assertRaises(SystemExit):
            self._run(
                ["--base-url", "https://10.0.77.118", "--confirm-write"], missing_ipv6
            )


if __name__ == "__main__":
    unittest.main()
