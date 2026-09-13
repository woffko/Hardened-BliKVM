import io
from pathlib import Path
import tempfile
import unittest
from typing import cast
from unittest.mock import patch

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



    def test_historical_prerequisites_are_parsed_with_pinned_producers(self) -> None:
        def resolve(value: str) -> Path:
            return Path(value)

        probes = cast(dict[str, dict[str, str]], cycle.pins.CURRENT_PINS["probe_sources"])
        historical = cast(dict[str, dict[str, object]], cycle.pins.HISTORICAL_PINS)
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(cycle.common.transport, "validate_private_file", side_effect=resolve), \
                patch.object(cycle.activation_receipt, "parse") as activate, \
                patch.object(cycle.recovery_receipt, "parse") as recover:
            paths: list[Path] = []
            for name in ("activation.json", "boot.json", "pending.json"):
                path = Path(directory, name)
                path.write_bytes(b"{}")
                paths.append(path)
            cycle.validate_historical(paths[0], paths[1], paths[2])
        activate.assert_called_once()
        self.assertEqual(activate.call_args.kwargs["expected_runner"], historical["native_activation"]["runner"])
        self.assertEqual(recover.call_args_list[0].kwargs["expected_probe_sources"], probes["boot"])
        self.assertEqual(recover.call_args_list[1].kwargs["expected_probe_sources"], probes["pending"])


if __name__ == "__main__":
    unittest.main()
