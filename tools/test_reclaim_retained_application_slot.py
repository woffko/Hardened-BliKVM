import unittest

from tools import reclaim_retained_application_slot as reclaim

SLOTS = [f"app-{n:020d}-" + "a" * 64 for n in range(3, 11)]
STATUS: dict[str, object] = {"activeSlotId": SLOTS[-1], "confirmedSlotId": SLOTS[-1], "previousSlotId": SLOTS[-2]}


class RetainedReclaimTests(unittest.TestCase):
    def test_select_oldest_targets_only_the_unprotected_oldest(self) -> None:
        self.assertEqual(reclaim.select_oldest(SLOTS, STATUS, "app-00000000000000000003-"), [SLOTS[0]])
        with self.assertRaises(ValueError):
            reclaim.select_oldest(SLOTS[:-1], STATUS, "app-00000000000000000003-")
        with self.assertRaises(ValueError):
            reclaim.select_oldest(SLOTS, STATUS, "app-00000000000000000004-")

    def test_validate_result_requires_full_retained_evidence(self) -> None:
        good: dict[str, object] = {"result": "passed", "deletedSlotIds": [SLOTS[0]],
            "retainedTreesUnchanged": True, "statusUnchanged": True, "admissionRetained": True,
            "decisionSha256": "d" * 64, "planSha256": "e" * 64}
        self.assertEqual(reclaim.validate_result(good, [SLOTS[0]]), good)
        mutations: list[dict[str, object]] = [{**good, "deletedSlotIds": []}, {**good, "statusUnchanged": False},
            {**good, "admissionRetained": False}, {**good, "decisionSha256": "short"}]
        for changed in mutations:
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                reclaim.validate_result(changed, [SLOTS[0]])


if __name__ == "__main__":
    unittest.main()
