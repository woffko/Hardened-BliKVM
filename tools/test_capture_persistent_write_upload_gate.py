import unittest
from pathlib import Path

from tools import capture_persistent_write_upload_gate as gate
from tools import persistent_write_contract as contract

ROOT = Path("/var/lib/hardened-blikvm-write-retain-" + "e" * 24)


class UploadGateCaptureTests(unittest.TestCase):
    def test_mode_command_uses_the_update_identity_except_for_the_root_probe(self) -> None:
        command = gate.mode_command("retry", ROOT, "/x.tar", "seq10-retry", 123, 456)
        self.assertEqual(command[:4], ["setpriv", "--reuid=123", "--regid=456", "--clear-groups"])
        self.assertIn("retry", command)
        unauthorized = gate.mode_command("unauthorized", ROOT, "/x.tar", "seq10-retry", 123, 456)
        self.assertEqual(unauthorized[0], "python3")
        with self.assertRaises(ValueError):
            gate.mode_command("publish", ROOT, "/x.tar", "seq10-retry", 1, 2)

    def test_mode_output_requires_an_exact_passed_vector(self) -> None:
        gate.validate_mode_output(b'{"mode": "retry", "result": "passed"}', "retry")
        for raw in (b'{"mode": "retry", "result": "failed"}', b'{"mode": "short", "result": "passed"}'):
            with self.assertRaises(ValueError):
                gate.validate_mode_output(raw, "retry")

    def test_profile_requires_a_reviewed_baseline(self) -> None:
        slot = "app-00000000000000000010-" + "c" * 64
        binding = contract.EvidenceBinding("e" * 24, "f" * 64, contract.CONFIG_SHA256, slot, 10, 46)
        self.assertEqual(gate.profile_for(binding).name, "seq10-retry")
        old = contract.EvidenceBinding("e" * 24, "f" * 64, contract.CONFIG_SHA256,
                                       "app-00000000000000000008-" + "c" * 64, 8, 40)
        with self.assertRaises(ValueError):
            gate.profile_for(old)


if __name__ == "__main__":
    unittest.main()
