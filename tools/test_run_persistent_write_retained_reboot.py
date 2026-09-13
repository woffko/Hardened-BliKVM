import unittest

from tools import run_persistent_write_retained_reboot as driver


class RetainedRebootTests(unittest.TestCase):
    def test_validate_retained_requires_a_committed_policy(self) -> None:
        good = {"result": "retained", "retentionApproved": True, "bootIdSha256": "a" * 64}
        self.assertEqual(driver.validate_retained(good), good)
        for changed in ({"result": "restored", "retentionApproved": True, "bootIdSha256": "a" * 64},
                        {"result": "retained", "retentionApproved": False, "bootIdSha256": "a" * 64},
                        {"result": "retained", "retentionApproved": True, "bootIdSha256": "short"}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                driver.validate_retained(changed)

    def test_survival_requires_a_new_boot_and_retained_policy(self) -> None:
        before: dict[str, object] = {"result": "retained", "retentionApproved": True,
                                     "bootIdSha256": "a" * 64}
        self.assertTrue(driver.survived(before, {**before, "bootIdSha256": "b" * 64}))
        self.assertFalse(driver.survived(before, before))
        self.assertFalse(driver.survived(before, {**before, "bootIdSha256": "b" * 64, "result": "restored"}))


if __name__ == "__main__":
    unittest.main()
