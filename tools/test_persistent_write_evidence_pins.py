import unittest

from tools import generate_persistent_write_evidence_pins as generator
from tools import persistent_write_evidence_pins as pins


class PinDriftTests(unittest.TestCase):
    def test_committed_pins_match_reviewed_sources(self) -> None:
        self.assertEqual(generator.TARGET.read_text(), generator.render(generator.build()))

    def test_current_pin_shape(self) -> None:
        current = pins.CURRENT_PINS
        self.assertEqual(set(current), {"runner", "browser_sources", "failure_browser_sources", "probe_sources"})
        runner = current["runner"]
        self.assertIsInstance(runner, str)
        self.assertRegex(str(runner), r"^[a-f0-9]{64}$")
        self.assertEqual(set(current["probe_sources"]), {"boot", "pending"})

    def test_historical_pins_are_commit_bound(self) -> None:
        historical = pins.HISTORICAL_PINS
        self.assertEqual(set(historical), {"gui_happy_path", "native_activation"})
        for entry in historical.values():
            self.assertRegex(str(entry["commit"]), r"^[a-f0-9]{40}$")
            self.assertRegex(str(entry["runner"]), r"^[a-f0-9]{64}$")


if __name__ == "__main__":
    unittest.main()
