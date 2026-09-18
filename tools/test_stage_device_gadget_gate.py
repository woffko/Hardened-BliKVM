import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_gadget_gate as gate


GOOD_FACTS = {
    "gadget_present": "yes",
    "gadget_name": "hardened",
    "udc_bound": "yes",
    "functions": "hid.usb0,hid.usb1,hid.usb2",
    "msd_file": "none",
    "mass_storage_present": "no",
    "hidg_devices": "/dev/hidg0,/dev/hidg1,/dev/hidg2",
    "hid_write": "ok",
}
PREFLIGHT_FACTS = GOOD_FACTS | {
    "owner_active": "active",
    "server_active": "active",
    "broker_sha256": "b" * 64,
}
STARTUP_FACTS = GOOD_FACTS | {
    "owner_active": "active",
    "socket_present": "yes",
    "server_active": "active",
}
ATTACHED_FACTS = GOOD_FACTS | {"msd_file": "kubuntu.img", "mass_storage_present": "yes"}


def facts_output(facts: dict[str, str]) -> str:
    return "".join(f"{key}={value}\n" for key, value in facts.items())


class GadgetGateTests(unittest.TestCase):
    def test_preflight_is_read_only_and_restart_only_restarts_the_owner(self) -> None:
        preflight = gate.preflight_script()
        for forbidden in (
            "systemctl restart",
            "systemctl start",
            "systemctl enable",
            "install -m",
            "unlink",
            "rm -",
        ):
            self.assertNotIn(forbidden, preflight)
        for required in ("owner_active=", "broker_sha256=", "hidg_devices=", "msd_file="):
            self.assertIn(required, preflight)
        self.assertIn("mass_storage_present=", preflight)

        restart = gate.restart_script(60)
        self.assertIn(f"systemctl restart {gate.UNIT}", restart)
        self.assertIn("socket_present=", restart)
        self.assertIn("owner_active=", restart)
        self.assertNotIn("reboot", restart)
        self.assertNotIn("systemctl enable", restart)

    def test_facts_parse_ignores_unexpected_lines(self) -> None:
        facts = gate.parse_facts("  indented=1\nowner_active=active\nbad key=1\n")
        self.assertEqual(facts, {"owner_active": "active"})

    def test_readback_requires_hid_and_storage_as_expected(self) -> None:
        gate.validate_readback(dict(GOOD_FACTS), stage="startup", expect_storage=False)
        gate.validate_readback(ATTACHED_FACTS, stage="attach", expect_storage=True)

        with self.assertRaises(gate.GateError) as absent:
            gate.validate_readback(
                GOOD_FACTS | {"gadget_present": "no"},
                stage="startup",
                expect_storage=False,
            )
        self.assertEqual(absent.exception.failure_type, "gadget_absent")

        with self.assertRaises(gate.GateError) as unbound:
            gate.validate_readback(
                GOOD_FACTS | {"udc_bound": "no"}, stage="startup", expect_storage=False
            )
        self.assertEqual(unbound.exception.failure_type, "gadget_unbound")

        with self.assertRaises(gate.GateError) as no_hid:
            gate.validate_readback(
                GOOD_FACTS | {"functions": "mass_storage.usb0"},
                stage="startup",
                expect_storage=False,
            )
        self.assertEqual(no_hid.exception.failure_type, "hid_function_missing")

        with self.assertRaises(gate.GateError) as leftover:
            gate.validate_readback(ATTACHED_FACTS, stage="eject", expect_storage=False)
        self.assertEqual(leftover.exception.failure_type, "mass_storage_unexpected")

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_gadget_gate.py",
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

    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"gadget-lifecycle-gate-test-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def test_plan_mode_records_observed_without_mutations(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=facts_output(PREFLIGHT_FACTS).encode(), stderr=b"", returncode=0)
        ]
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, [])),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertEqual(receipt["mode"], "plan")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertIs(receipt["startupBindPassed"], False)
        self.assertEqual(receipt["brokerSha256"], "b" * 64)

    def test_restart_and_media_cycle_pass_when_readbacks_match(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=facts_output(PREFLIGHT_FACTS).encode(), stderr=b"", returncode=0),
            mock.Mock(stdout=facts_output(STARTUP_FACTS).encode(), stderr=b"", returncode=0),
            mock.Mock(stdout=facts_output(ATTACHED_FACTS).encode(), stderr=b"", returncode=0),
            mock.Mock(stdout=facts_output(GOOD_FACTS).encode(), stderr=b"", returncode=0),
        ]
        api = mock.Mock()
        api.catalog.return_value = [
            {"imageId": 7, "imageName": "kubuntu.img", "removableAllowed": True}
        ]
        api.command.return_value = (True, "")
        with (
            mock.patch.object(
                sys,
                "argv",
                self._argv(
                    output,
                    known_hosts,
                    [
                        "--confirm-restart",
                        "--confirm-media-cycle",
                        "--confirm-hid-observed",
                        "--image",
                        "kubuntu.img",
                        "--base-url",
                        "https://192.0.2.10",
                        "--api-username",
                        "admin",
                    ],
                ),
            ),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(receipt["mode"], "restart+media")
        self.assertIs(receipt["startupBindPassed"], True)
        self.assertIs(receipt["attachKeptHid"], True)
        self.assertIs(receipt["ejectKeptHid"], True)
        self.assertIs(receipt["hidObservedOnTarget"], True)
        self.assertIs(receipt["mutationsPerformed"], True)
        actions = [call.args[0]["action"] for call in api.command.call_args_list]
        self.assertEqual(actions, ["select", "set_connected", "set_connected"])
        self.assertEqual(api.command.call_args_list[0].args[0]["mode"], "removable_disk")

    def test_media_cycle_waits_out_the_mutation_cooldown(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=facts_output(PREFLIGHT_FACTS).encode(), stderr=b"", returncode=0),
            mock.Mock(stdout=facts_output(ATTACHED_FACTS).encode(), stderr=b"", returncode=0),
            mock.Mock(stdout=facts_output(GOOD_FACTS).encode(), stderr=b"", returncode=0),
        ]
        api = mock.Mock()
        api.catalog.return_value = [
            {"imageId": 7, "imageName": "kubuntu.img", "removableAllowed": True}
        ]
        api.command.side_effect = [(True, ""), (False, "cooldown_active"), (True, ""), (True, "")]
        with (
            mock.patch.object(
                sys,
                "argv",
                self._argv(
                    output,
                    known_hosts,
                    [
                        "--confirm-media-cycle",
                        "--image",
                        "kubuntu.img",
                        "--base-url",
                        "https://192.0.2.10",
                        "--api-username",
                        "admin",
                    ],
                ),
            ),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
            mock.patch.object(gate.time, "sleep", self._no_sleep),
            mock.patch.object(gate.time, "monotonic", side_effect=iter(range(0, 1000, 2))),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertEqual(api.command.call_count, 4)

    def test_media_cycle_fails_closed_on_a_non_cooldown_rejection(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=facts_output(PREFLIGHT_FACTS).encode(), stderr=b"", returncode=0)
        ]
        api = mock.Mock()
        api.catalog.return_value = [
            {"imageId": 7, "imageName": "kubuntu.img", "removableAllowed": True}
        ]
        api.command.return_value = (False, "conflict")
        with (
            mock.patch.object(
                sys,
                "argv",
                self._argv(
                    output,
                    known_hosts,
                    [
                        "--confirm-media-cycle",
                        "--image",
                        "kubuntu.img",
                        "--base-url",
                        "https://192.0.2.10",
                        "--api-username",
                        "admin",
                    ],
                ),
            ),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 1)
        self.assertEqual(receipt["failureType"], "select_conflict")

    @staticmethod
    def _no_sleep(_: float) -> None:
        return None

    def test_media_cycle_fails_when_the_image_is_not_catalogued(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=facts_output(PREFLIGHT_FACTS).encode(), stderr=b"", returncode=0)
        ]
        api = mock.Mock()
        api.catalog.return_value = []
        with (
            mock.patch.object(
                sys,
                "argv",
                self._argv(
                    output,
                    known_hosts,
                    [
                        "--confirm-media-cycle",
                        "--image",
                        "missing.img",
                        "--base-url",
                        "https://192.0.2.10",
                        "--api-username",
                        "admin",
                    ],
                ),
            ),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 1)
        self.assertEqual(receipt["result"], "failed")
        self.assertEqual(receipt["failureType"], "image_not_catalogued")

    def test_scripts_do_not_carry_credentials(self) -> None:
        for script in (
            gate.preflight_script(),
            gate.restart_script(30),
            gate.facts_script(),
        ):
            self.assertNotIn("password", script.lower())
            self.assertNotIn("secret", script.lower())

if __name__ == "__main__":
    unittest.main()
