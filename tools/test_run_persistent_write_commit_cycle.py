import io
import unittest

from tools import persistent_write_contract as contract
from tools import run_persistent_write_commit_cycle as cycle
from tools.secret_pair_stdin import secret_pair_stdin


class CommitCycleTests(unittest.TestCase):
    def test_pair_envelope_round_trips_through_the_reviewed_reader(self) -> None:
        pair = {"ssh": bytearray(b"ssh-secret\n"), "gui": bytearray(b"gui-secret")}
        raw = cycle.pair_envelope(pair)
        with secret_pair_stdin(io.BytesIO(raw)) as decoded:
            self.assertEqual(decoded["ssh"], pair["ssh"])
            self.assertEqual(decoded["gui"], pair["gui"])
        self.assertNotIn(b"ssh-secret", raw)
        self.assertNotIn(b"gui-secret", raw)

    def test_commit_result_requires_the_exact_receipt_map(self) -> None:
        digests = {gate: "a" * 64 for gate in contract.REQUIRED_GATES}
        good = {"result": "committed", "retentionApproved": True, "gateReceiptHashes": digests}
        self.assertEqual(cycle.validate_commit_result(good, digests), good)
        for changed in ({**good, "result": "failed"}, {**good, "retentionApproved": False},
                        {**good, "gateReceiptHashes": {}}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                cycle.validate_commit_result(changed, digests)

    def test_fresh_gate_files_cover_the_reviewed_gates(self) -> None:
        self.assertEqual(set(cycle.FRESH_GATE_FILES),
                         {"host", "native_upload", "gui_failure_retry", "gui_cancel", "gui_logout"})


if __name__ == "__main__":
    unittest.main()
