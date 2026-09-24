import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

import tools.compare_gui_parity as comparator


CONTEXT = {
    "acceptanceRunId": "gui-parity-test-0001",
    "reviewedSourceCommit": "a" * 40,
}


def receipt(
    labels: list[str],
    tags: dict[str, int],
    errors: int = 0,
    capabilities: dict[str, str] | None = None,
    markers: dict[str, int] | None = None,
    failed_requests: list[dict[str, object]] | None = None,
    settings_tabs: list[str] | None = None,
    role: str = "ours",
    capability_status: int = 200,
) -> dict[str, object]:
    return {
        "schemaVersion": 2,
        "probe": comparator.PROBE,
        "result": "passed",
        "targetRole": role,
        "evidenceContext": CONTEXT,
        "coverage": {name: "complete" for name in comparator.COVERAGE_FIELDS},
        "capabilitiesRequest": {
            "method": "GET",
            "route": "/api/capabilities",
            "outcome": "http",
            "status": capability_status,
        },
        "route": "/#/desktop",
        "title": "Hardened BLi-KVM",
        "menuLabels": labels,
        "tagCounts": tags,
        "consoleErrors": errors,
        "capabilities": capabilities or {},
        "markers": markers or {},
        "failedRequests": failed_requests or [],
        "settingsTabs": settings_tabs or [],
    }


def write(directory: Path, name: str, value: dict[str, object]) -> Path:
    path = directory / name
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


class CompareGuiParityTests(unittest.TestCase):
    def test_malformed_root_and_non_string_enums_are_controlled_errors(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            malformed: list[object] = [None, [], 1, {**receipt([], {}), "targetRole": []},
                         {**receipt([], {}), "result": {}}]
            for value in malformed:
                with self.subTest(value=value):
                    path.write_text(json.dumps(value), encoding="utf-8")
                    with self.assertRaises(ValueError):
                        comparator.load_receipt(path)

    def test_identical_receipts_have_no_differences(self) -> None:
        value = receipt(["Images", "Power"], {"div": 10, "nav": 1}, 0)
        result = comparator.compare(value, {**value, "targetRole": "reference"})
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
            role="reference",
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

    def test_empty_marker_set_is_still_compared(self) -> None:
        ours = receipt(["Images"], {"div": 1}, markers={"mobileKeyboardInput": 1})
        reference = receipt(["Images"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertIs(result["markersCompared"], True)
        self.assertEqual(
            result["markerDiffs"],
            {"mobileKeyboardInput": {"ours": 1, "reference": None}},
        )

        reference = receipt(["Images"], {"div": 1}, markers={"mobileKeyboardInput": 0})
        result = comparator.compare(ours, reference)
        self.assertIs(result["markersCompared"], True)
        self.assertEqual(
            result["markerDiffs"],
            {"mobileKeyboardInput": {"ours": 1, "reference": 0}},
        )

    def test_reference_capability_404_is_an_explicit_asymmetry(self) -> None:
        ours = receipt(
            ["Images"], {"div": 1}, capabilities={"hid.status.availability": "read_write"}
        )
        reference = receipt(
            ["Images"], {"div": 1}, role="reference", capability_status=404
        )
        result = comparator.compare(ours, reference)
        self.assertIs(result["capabilitiesCompared"], False)
        self.assertEqual(result["capabilityDiffs"], {})
        self.assertEqual(result["expectedAsymmetries"], [{
            "side": "reference", "method": "GET", "route": "/api/capabilities",
            "oursStatus": 200, "referenceStatus": 404,
        }])

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

    def test_empty_failed_request_set_is_still_compared(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[
                {"method": "GET", "route": "/api/download/image/status", "status": 404}
            ],
        )
        reference = receipt(["Images"], {"div": 1})
        result = comparator.compare(ours, reference)
        self.assertIs(result["failedRequestsCompared"], True)
        self.assertEqual(
            result["failedRequestsOnlyInOurs"],
            ["GET /api/download/image/status:404"],
        )
        self.assertEqual(result["failedRequestsOnlyInReference"], [])

    def test_invalid_failed_requests_are_rejected(self) -> None:
        ours = receipt(
            ["Images"],
            {"div": 1},
            failed_requests=[{"method": "GET", "route": "no-slash", "status": 404}],
        )
        with self.assertRaises(ValueError):
            comparator.compare(ours, ours)

    def test_settings_tabs_are_compared_even_when_one_side_is_empty(self) -> None:
        ours = receipt(
            ["Images"], {"div": 1}, settings_tabs=["About", "Device", "System"]
        )
        reference = receipt(["Images"], {"div": 1}, settings_tabs=["About", "System"])
        result = comparator.compare(ours, reference)
        self.assertIs(result["settingsTabsCompared"], True)
        self.assertEqual(result["settingsTabsOnlyInOurs"], ["Device"])
        self.assertEqual(result["settingsTabsOnlyInReference"], [])

        result = comparator.compare(ours, receipt(["Images"], {"div": 1}))
        self.assertIs(result["settingsTabsCompared"], True)
        self.assertEqual(
            result["settingsTabsOnlyInOurs"], ["About", "Device", "System"]
        )

    def test_incomplete_coverage_fails_the_comparison(self) -> None:
        ours = receipt(["Images"], {"div": 1})
        ours["coverage"] = {
            **{name: "complete" for name in comparator.COVERAGE_FIELDS},
            "settingsTabs": "failed",
        }
        reference = receipt(["Images"], {"div": 1}, role="reference")
        result = comparator.compare(ours, reference)
        self.assertEqual(result["result"], "failed")
        self.assertEqual(result["coverageGaps"], ["ours.coverage.settingsTabs"])

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
            bad_tags = receipt([], {})
            bad_tags["tagCounts"] = {"div": "many"}
            with self.assertRaises(ValueError):
                comparator.compare(bad_tags, receipt([], {}))

            bad_coverage = receipt([], {})
            bad_coverage["coverage"] = {
                **{name: "complete" for name in comparator.COVERAGE_FIELDS},
                "settingsTabs": [],
            }
            with self.assertRaises(ValueError):
                comparator.load_receipt(write(path, "bad-coverage.json", bad_coverage))

            bad_errors = receipt([], {})
            bad_errors["consoleErrors"] = True
            with self.assertRaises(ValueError):
                comparator.load_receipt(write(path, "bad-errors.json", bad_errors))

    def test_direct_entrypoint_loads_project_package(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory)
            ours = write(path, "ours.json", receipt(["A"], {"div": 1}))
            reference = write(
                path,
                "reference.json",
                receipt(["A"], {"div": 1}, role="reference"),
            )
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
