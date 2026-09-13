import base64
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import stage_persistent_write_evidence as stage


def receipts() -> dict[str, bytes]:
    return {gate: ("receipt-" + gate).encode() for gate in stage.contract.REQUIRED_GATES}


class ReceiptLoadingTests(unittest.TestCase):
    def write_receipt(self, directory: str, data: bytes = b"{}") -> Path:
        path = Path(directory) / "receipt.json"
        path.write_bytes(data)
        return path

    def test_complete_gates_reject_missing_duplicate_and_unknown(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_receipt(directory)
            values = [f"{gate}=ignored" for gate in sorted(stage.contract.REQUIRED_GATES)]
            with patch.object(stage.common.transport, "validate_private_file", return_value=path):
                loaded = stage.load_receipts(values)
            self.assertEqual(set(loaded), set(stage.contract.REQUIRED_GATES))
            for broken in (values[:-1], [*values, values[0]], [*values[:-1], "unknown=ignored"]):
                with patch.object(stage.common.transport, "validate_private_file", return_value=path), \
                        self.assertRaises(ValueError):
                    stage.load_receipts(broken)

    def test_receipt_size_is_bounded(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = self.write_receipt(directory, b"x" * 4)
            values = [f"{gate}=ignored" for gate in sorted(stage.contract.REQUIRED_GATES)]
            with patch.object(stage.common.transport, "validate_private_file", return_value=path), \
                    patch.object(stage.storage, "MAX_BYTES", 3), self.assertRaises(ValueError):
                stage.load_receipts(values)

    def test_envelope_is_canonical_sorted_json(self) -> None:
        data = receipts()
        expected = json.dumps({"schemaVersion": 1, "gates": {
            gate: base64.b64encode(data[gate]).decode("ascii") for gate in sorted(data)}},
            sort_keys=True, separators=(",", ":")).encode()
        self.assertEqual(stage.build_envelope(data), expected)

    def test_device_result_must_match_receipts_and_transaction(self) -> None:
        data = receipts()
        transaction = "a" * 24
        expected = {gate: hashlib.sha256(value).hexdigest() for gate, value in data.items()}
        good = {"result": "sealed",
                "evidenceRoot": str(stage.storage.PARENT / (stage.storage.PREFIX + transaction)),
                "sealSha256": "b" * 64, "receipts": expected}
        stage.validate_device_result(good, data, transaction)
        def broken_result(value: dict[str, object]) -> None:
            value["result"] = "failed"
        def broken_root(value: dict[str, object]) -> None:
            value["evidenceRoot"] = "/tmp/foreign"
        def broken_seal(value: dict[str, object]) -> None:
            value["sealSha256"] = "short"
        def broken_receipts(value: dict[str, object]) -> None:
            receipts_value = value["receipts"]
            assert isinstance(receipts_value, dict)
            receipts_value["host"] = "c" * 64
        for mutation in (broken_result, broken_root, broken_seal, broken_receipts):
            changed = json.loads(json.dumps(good))
            mutation(changed)
            with self.subTest(mutation=mutation.__name__), self.assertRaises(ValueError):
                stage.validate_device_result(changed, data, transaction)


if __name__ == "__main__":
    unittest.main()
