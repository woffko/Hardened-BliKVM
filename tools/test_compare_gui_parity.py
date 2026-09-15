import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import tools.compare_gui_parity as comparator


def receipt(
    labels: list[str],
    tags: dict[str, int],
    errors: int = 0,
    capabilities: dict[str, str] | None = None,
    markers: dict[str, int] | None = None,
    failed_requests: list[dict[str, object]] | None = None,
) -> dict:
    return {
        "schemaVersion": 1,
        "probe": comparator.PROBE,
        "route": "/#/desktop",
        "title": "Hardened BLi-KVM",
        "menuLabels": labels,
        "tagCounts": tags,
        "consoleErrors": errors,
        "capabilities": capabilities or {},
        "markers": markers or {},
        "failedRequests": failed_requests or [],
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

    def test_capability_differences_explain_hidden_labels(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            capabilities={"hid.status.availability": "unavailable"},
            markers={"mobileKeyboardInput": 0},
        )
        reference = receipt(
            ["Images", "Mobile local keyboard input", "Terminal"],
            {"div": 1},
            capabilities={"hid.status.availability": "read_write"},
            markers={"mobileKeyboardInput": 1},
        )
        result = comparator.compare(ours, reference)
        self.assertEqual(
            result["capabilityDiffs"],
            {"hid.status.availability": {"ours": "unavailable", "reference": "read_write"}},
        )
        self.assertEqual(
            result["explainedLabelsOnlyInReference"],
            {"Mobile local keyboard input": "hid.status.availability"},
        )
        self.assertEqual(result["unexplainedLabelsOnlyInReference"], ["Terminal"])
        self.assertEqual(
            result["markerDiffs"],
            {"mobileKeyboardInput": {"ours": 0, "reference": 1}},
        )

    def test_unexplained_labels_are_reported_when_no_capability_differs(self) -> None:
        ours = receipt(["Images"], {"div": 1})
        reference = receipt(["Images", "Power", "Diagnostics"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertEqual(result["capabilityDiffs"], {})
        self.assertEqual(
            result["unexplainedLabelsOnlyInReference"], ["Diagnostics", "Power"]
        )

    def test_branding_labels_are_expected_not_gaps(self) -> None:
        ours = receipt(["Hardened BliKVM", "Mobile local keyboard input"], {"div": 1})
        reference = receipt(["Hardened NanoKVM", "Mobile local keyboard input"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertEqual(result["expectedLabelsOnlyInOurs"], ["Hardened BliKVM"])
        self.assertEqual(result["expectedLabelsOnlyInReference"], ["Hardened NanoKVM"])
        self.assertEqual(result["unexplainedLabelsOnlyInOurs"], [])
        self.assertEqual(result["unexplainedLabelsOnlyInReference"], [])

    def test_markers_are_compared_only_when_both_receipts_carry_them(self) -> None:
        ours = receipt(["Images"], {"div": 1}, markers={"mobileKeyboardInput": 1})
        reference = receipt(["Images"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertIs(result["markersCompared"], False)
        self.assertEqual(result["markerDiffs"], {})

        reference = receipt(["Images"], {"div": 1}, markers={"mobileKeyboardInput": 0})
        result = comparator.compare(ours, reference)
        self.assertIs(result["markersCompared"], True)
        self.assertEqual(
            result["markerDiffs"],
            {"mobileKeyboardInput": {"ours": 1, "reference": 0}},
        )

    def test_capabilities_are_compared_only_when_both_receipts_carry_them(self) -> None:
        ours = receipt(
            ["Images"], {"div": 1}, capabilities={"hid.status.availability": "read_write"}
        )
        reference = receipt(["Images"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertIs(result["capabilitiesCompared"], False)
        self.assertEqual(result["capabilityDiffs"], {})

    def test_failed_requests_are_reported_per_side(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[
                {"method": "GET", "route": "/api/download/image/status", "status": 404},
                {"method": "GET", "route": "/api/inventory", "status": 503},
            ],
        )
        reference = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[
                {"method": "GET", "route": "/api/inventory", "status": 503},
            ],
        )
        result = comparator.compare(ours, reference)
        self.assertIs(result["failedRequestsCompared"], True)
        self.assertEqual(
            result["failedRequestsOnlyInOurs"],
            ["GET /api/download/image/status:404"],
        )
        self.assertEqual(result["failedRequestsOnlyInReference"], [])

    def test_failed_requests_are_not_compared_when_one_side_lacks_them(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[
                {"method": "GET", "route": "/api/download/image/status", "status": 404}
            ],
        )
        reference = receipt(["Images"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertIs(result["failedRequestsCompared"], False)
        self.assertEqual(result["failedRequestsOnlyInOurs"], [])
        self.assertEqual(result["failedRequestsOnlyInReference"], [])

    def test_invalid_failed_requests_are_rejected(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[{"method": "GET", "route": "no-slash", "status": 404}],
        )
        with self.assertRaises(ValueError):
            comparator.compare(ours, ours)

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
