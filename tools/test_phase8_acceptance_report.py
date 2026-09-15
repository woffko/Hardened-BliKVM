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

    def test_missing_receipts_are_unrun_and_never_inferred(self) -> None:
        root, evidence = self.fixture()
        built = report.build_report(root, evidence)
        self.assertEqual(built["overall"], "incomplete")
        for phase in built["phases"]:
            for item in phase["device"]:
                if item["name"] != "composite gadget acceptance":
                    self.assertEqual(item["status"], "unrun", item)
        phase8 = next(phase for phase in built["phases"] if phase["phase"] == "8")
        virtual_media = next(
            item for item in phase8["device"] if item["name"] == "virtual media acceptance"
        )
        self.assertEqual(virtual_media["status"], "unrun")
        self.assertIs(built["policy"]["inferenceFromCode"], False)

    def test_recorded_receipts_with_markers_complete_a_phase(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "board-handoff-gate-20260915-e.json",
            {
                "gate": "hardened-blikvm-board-profile-handoff",
                "applyFacts": {"socket_present": "yes", "owner_started": "yes"},
            },
        )
        built = report.build_report(root, evidence)
        phase3 = next(phase for phase in built["phases"] if phase["phase"] == "3")
        handoff = next(
            item for item in phase3["device"] if item["name"] == "board-owner handoff gate"
        )
        self.assertEqual(handoff["status"], "recorded")
        self.assertEqual(handoff["receipt"], "board-handoff-gate-20260915-e.json")
        acceptance = next(
            item for item in phase3["device"] if item["name"] == "composite gadget acceptance"
        )
        self.assertEqual(acceptance["status"], "unrun")
        self.assertEqual(phase3["status"], "incomplete")

    def test_wrong_marker_or_probe_stays_unrun_with_a_detail(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "board-handoff-gate-20260915-e.json",
            {
                "gate": "hardened-blikvm-board-profile-handoff",
                "applyFacts": {"socket_present": "no"},
            },
        )
        write_json(
            evidence / "video-soak-probe-20260913-a.json",
            {"probe": "something-else", "result": "passed"},
        )
        built = report.build_report(root, evidence)
        phase2 = next(phase for phase in built["phases"] if phase["phase"] == "2")
        self.assertEqual(phase2["device"][0]["status"], "unrun")
        phase4 = next(phase for phase in built["phases"] if phase["phase"] == "4")
        soak = next(item for item in phase4["device"] if item["name"] == "video soak")
        self.assertEqual(soak["status"], "unrun")
        self.assertIn("durationSeconds", soak["detail"])

    def test_markerless_receipts_need_their_required_fields(self) -> None:
        root, evidence = self.fixture()
        write_json(
            evidence / "video-soak-probe-20260913-a.json",
            {"result": "passed", "durationSeconds": 60, "samples": [1, 2, 3]},
        )
        built = report.build_report(root, evidence)
        phase4 = next(phase for phase in built["phases"] if phase["phase"] == "4")
        soak = next(item for item in phase4["device"] if item["name"] == "video soak")
        self.assertEqual(soak["status"], "recorded")
        two_client = next(
            item for item in phase4["device"] if item["name"] == "two-client video"
        )
        self.assertEqual(two_client["status"], "unrun")

    def test_missing_host_artifact_is_reported(self) -> None:
        root, evidence = self.fixture()
        (root / "crates/bli-kvm-usb-gadget/src/hid_descriptors.rs").unlink()
        built = report.build_report(root, evidence)
        phase3 = next(phase for phase in built["phases"] if phase["phase"] == "3")
        self.assertIn(
            "crates/bli-kvm-usb-gadget/src/hid_descriptors.rs",
            phase3["host"]["missing"],
        )


if __name__ == "__main__":
    unittest.main()
