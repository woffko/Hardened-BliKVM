import json
import tempfile
import unittest
from pathlib import Path

import tools.phase8_acceptance_report as report


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


class Phase8AcceptanceReportTests(unittest.TestCase):
    def fixture(self) -> tuple[Path, Path]:
        root = Path(tempfile.mkdtemp())
        evidence = root / "evidence" / "private"
        evidence.mkdir(parents=True)
        for spec in report.PHASES:
            for relative in spec["host"]:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("fixture\n", encoding="utf-8")
        return root, evidence

    def phase(self, built: dict, name: str) -> dict:
        return next(phase for phase in built["phases"] if phase["phase"] == name)

    def test_missing_receipts_are_unrun_and_never_inferred(self) -> None:
        root, evidence = self.fixture()
        built = report.build_report(root, evidence)
        self.assertEqual(built["overall"], "incomplete")
        for phase in built["phases"]:
            for item in phase["device"]:
                self.assertEqual(item["status"], "unrun", item)
        storage = next(
            item
            for item in self.phase(built, "E1")["device"]
            if item["name"] == "virtual media GUI acceptance"
        )
        self.assertEqual(storage["status"], "unrun")
        self.assertIs(built["policy"]["inferenceFromCode"], False)

    def test_recorded_receipts_with_markers_complete_a_phase(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "storage-acceptance-20260916-x.json",
            {"probe": "hardened-blikvm-storage-acceptance", "result": "passed"},
        )
        built = report.build_report(root, evidence)
        phase = self.phase(built, "E1")
        gate = next(item for item in phase["device"] if item["name"] == "storage gate")
        self.assertEqual(gate["status"], "recorded")
        self.assertEqual(gate["receipt"], "storage-acceptance-20260916-x.json")
        gui = next(
            item for item in phase["device"] if item["name"] == "virtual media GUI acceptance"
        )
        self.assertEqual(gui["status"], "unrun")
        self.assertEqual(phase["status"], "incomplete")

    def test_wrong_marker_or_probe_stays_unrun_with_a_detail(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "video-frame-timing-20260916-x.json",
            {"probe": "something-else", "result": "passed"},
        )
        write_json(
            evidence / "video-two-client-probe-20260916-x.json",
            {"result": "passed"},
        )
        built = report.build_report(root, evidence)
        phase = self.phase(built, "E0")
        timing = next(item for item in phase["device"] if item["name"] == "frame timing")
        self.assertEqual(timing["status"], "unrun")
        two_client = next(
            item for item in phase["device"] if item["name"] == "two-client video"
        )
        self.assertEqual(two_client["status"], "unrun")
        self.assertIn("streamCount", two_client["detail"])

    def test_markerless_receipts_need_their_required_fields(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "video-two-client-probe-20260916-x.json",
            {"result": "passed", "streamCount": 2},
        )
        built = report.build_report(root, evidence)
        phase = self.phase(built, "E0")
        two_client = next(
            item for item in phase["device"] if item["name"] == "two-client video"
        )
        self.assertEqual(two_client["status"], "recorded")
        soak = next(item for item in phase["device"] if item["name"] == "video soak")
        self.assertEqual(soak["status"], "unrun")

    def test_missing_host_artifact_is_reported(self) -> None:
        root, evidence = self.fixture()
        (root / "crates/bli-kvm-boardd/src/fan.rs").unlink()
        built = report.build_report(root, evidence)
        phase = self.phase(built, "E2")
        self.assertIn("crates/bli-kvm-boardd/src/fan.rs", phase["host"]["missing"])


if __name__ == "__main__":
    unittest.main()
