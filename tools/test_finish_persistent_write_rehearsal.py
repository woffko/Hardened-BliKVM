import unittest

from tools import finish_persistent_write_rehearsal as finish


class FinishTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = "/var/lib/" + finish.runner.native.contract.PREFIX + "a" * 24
        self.receipt: dict[str, object] = {"probe": "persistent-write-wiring-rehearsal", "guardRoot": self.root,
            "guardRetained": True, "runtimeWritesClosed": True, "cleanupSubmitted": False, "retentionApproved": False,
            "recovery": {"result": "restored", "guardRoot": self.root, "runtimeVerified": True, "boot": False, "retentionApproved": False}}
    def test_restored_receipt_selects_only_its_owned_root(self) -> None:
        self.assertEqual(finish.restored_root(self.receipt), self.root)
    def test_ambiguous_cleanup_or_unproven_closure_cannot_be_replayed(self) -> None:
        for key, value in (("cleanupSubmitted", True), ("runtimeWritesClosed", False), ("guardRetained", False),
                           ("retentionApproved", True), ("guardRoot", "/var/lib")):
            with self.subTest(key=key), self.assertRaises(ValueError):
                finish.restored_root({**self.receipt, key: value})
    def test_boot_file_restoration_is_not_runtime_acceptance(self) -> None:
        with self.assertRaises(ValueError):
            finish.restored_root({**self.receipt, "recovery": {"result": "restored", "guardRoot": self.root,
                "runtimeVerified": False, "boot": True, "retentionApproved": False}})
