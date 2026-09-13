import unittest

from tools import capture_persistent_write_gui_gate as gate
from tools import persistent_write_contract as contract

SLOT9 = "app-00000000000000000009-" + "c" * 64
SLOT10 = "app-00000000000000000010-" + "c" * 64


def binding(slot: str, floor: int, generation: int) -> contract.EvidenceBinding:
    return contract.EvidenceBinding("e" * 24, "f" * 64, contract.CONFIG_SHA256, slot, floor, generation)


class GuiGateCaptureTests(unittest.TestCase):
    def test_profile_follows_the_reviewed_baseline(self) -> None:
        self.assertEqual(gate.profile_for(binding(SLOT10, 10, 46)).name, "seq10-retry")
        self.assertEqual(gate.profile_for(binding(SLOT9, 9, 43)).name, "seq9-retry")

    def test_unreviewed_baseline_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            gate.profile_for(binding("app-00000000000000000008-" + "c" * 64, 8, 40))


if __name__ == "__main__":
    unittest.main()
