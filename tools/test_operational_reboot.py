import copy
import inspect
import unittest

from tools import stage_operational_reboot as gate


class OperationalRebootTests(unittest.TestCase):
    def fixture(self) -> dict[str, object]:
        return {"result": "ready", "bootIdSha256": "a" * 64, "status": {"floorSequence": 2, "generation": 7},
                "snapshot": {"state": {"signed-file": "hash"}, "nginxUnit": "hash",
                             "runtime": {"account": "hash", "nginxConfig": "hash", "certificate": "hash", "tlsKey": "hash",
                                         "trust": "hash", "brokerPid": "100", "brokerRestarts": "0", "videoRestarts": "0"}},
                "recoveryOrderVerified": True, "rootfsMode": "rw"}

    def test_comparison_accepts_new_boot_with_preserved_state(self) -> None:
        before = self.fixture()
        after = copy.deepcopy(before)
        after["bootIdSha256"] = "b" * 64
        gate.compare(before, after)
        after["status"] = {"floorSequence": 1, "generation": 1}
        with self.assertRaises(ValueError):
            gate.compare(before, after)

    def test_unchanged_or_missing_boot_identity_is_not_reboot_proof(self) -> None:
        before = self.fixture()
        for value in (None, "", "a" * 64):
            after = copy.deepcopy(before)
            after["bootIdSha256"] = value
            with self.assertRaises(ValueError):
                gate.compare(before, after)

    def test_verified_order_and_requested_rw_mode_are_required(self) -> None:
        before = self.fixture()
        for field, value in (("rootfsMode", "ro"), ("recoveryOrderVerified", False)):
            after = copy.deepcopy(before)
            after["bootIdSha256"] = "b" * 64
            after[field] = value
            with self.assertRaises(ValueError):
                gate.compare(before, after)

    def test_single_nonforced_software_reboot_request(self) -> None:
        self.assertEqual(gate.REBOOT_COMMAND, "systemctl reboot --no-block")
        source = inspect.getsource(gate.main)
        self.assertEqual(source.count("transport.run_ssh(target, secret, REBOOT_COMMAND"), 1)
        self.assertIn("except subprocess.TimeoutExpired:", source)
        self.assertIn("if not args.reboot:", source)
        self.assertNotIn("--force", source)


if __name__ == "__main__":
    unittest.main()
