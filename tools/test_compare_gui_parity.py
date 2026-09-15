import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import tools.compare_gui_parity as comparator


def receipt(labels: list[str], tags: dict[str, int], errors: int = 0) -> dict:
    return {
        "schemaVersion": 1,
        "probe": comparator.PROBE,
        "route": "/#/desktop",
        "title": "Hardened BLi-KVM",
        "menuLabels": labels,
        "tagCounts": tags,
        "consoleErrors": errors,
    }


def write(directory: Path, name: str, value: dict) -> Path:
    path = directory / name
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


class CompareGuiParityTests(unittest.TestCase):
    def test_identical_receipts_have_no_differences(self) -> None:
        value = receipt(["Images", "Power"], {"div": 10, "nav": 1}, 0)
        result = comparator.compare(value, value)
        self.assertEqual(result["labelsOnlyInOurs"], [])
        self.assertEqual(result["labelsOnlyInReference"], [])
        self.assertEqual(result["tagCountDeltas"], {})
        self.assertEqual(result["consoleErrorDelta"], 0)
        self.assertEqual(result["sharedLabelCount"], 2)

    def test_label_tag_and_error_differences_are_reported(self) -> None:
        ours = receipt(["Images", "Power", "Settings"], {"div": 12, "nav": 1}, 2)
        reference = receipt(["Images", "Power"], {"div": 10, "nav": 1, "main": 1}, 0)
        result = comparator.compare(ours, reference)
        self.assertEqual(result["labelsOnlyInOurs"], ["Settings"])
        self.assertEqual(result["labelsOnlyInReference"], [])
        self.assertEqual(result["tagCountDeltas"], {"div": 2, "main": -1})
        self.assertEqual(result["consoleErrorDelta"], 2)

    def test_invalid_receipts_are_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            wrong = write(path, "wrong.json", {"probe": "other", "menuLabels": []})
            with self.assertRaises(ValueError):
                comparator.load_receipt(wrong)
            bad_labels = write(
                path, "bad.json", {"probe": comparator.PROBE, "menuLabels": "nope"}
            )
            with self.assertRaises(ValueError):
                comparator.load_receipt(bad_labels)
            bad_tags = receipt([], [])  # type: ignore[arg-type]
            bad_tags["tagCounts"] = {"div": "many"}
            with self.assertRaises(ValueError):
                comparator._tag_counts(bad_tags)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            ours = write(path, "ours.json", receipt(["A"], {"div": 1}))
            reference = write(path, "reference.json", receipt(["A"], {"div": 1}))
            output = Path("evidence/private/parity-compare-selftest.json").resolve()
            result = subprocess.run(
                [
                    sys.executable,
                    "tools/compare_gui_parity.py",
                    "--ours",
                    str(ours),
                    "--reference",
                    str(reference),
                    "--output",
                    str(output),
                ],
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            try:
                self.assertEqual(result.returncode, 0, result.stderr.decode())
                self.assertIn(b"onlyOurs=0", result.stdout)
            finally:
                output.unlink(missing_ok=True)
        help_result = subprocess.run(
            [sys.executable, "tools/compare_gui_parity.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(help_result.returncode, 0, help_result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
