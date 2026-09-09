from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import application_write_guard as guard


def plan() -> dict[str, object]:
    return {"schemaVersion": 1, "phase": "prepared", "bootId": "a" * 36,
            "observedFloor": 7, "startedMonotonic": 10.0,
            "sourceSlot": guard.contract.SOURCE_SLOT, "targetSlot": guard.contract.TARGET_SLOT}


class GuardTests(unittest.TestCase):
    def test_marker_validation_is_strict_and_same_boot(self) -> None:
        guard.validate_plan(plan(), boot_id="a" * 36)
        invalid = ({"schemaVersion": True}, {"observedFloor": True}, {"observedFloor": 6},
                   {"startedMonotonic": float("nan")}, {"startedMonotonic": float("inf")},
                   {"startedMonotonic": True}, {"phase": "unknown"}, {"bootId": "b" * 36},
                   {"targetSlot": guard.contract.SOURCE_SLOT})
        for mutation in invalid:
            with self.assertRaises(ValueError):
                guard.validate_plan({**plan(), **mutation}, boot_id="a" * 36)

    def test_expired_or_revoked_guard_never_allows_open(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            boot = root / "boot"
            boot.write_text("a" * 36)
            with patch.object(guard, "BOOT_ID", boot):
                guard.require_open_budget(root, plan(), now=429.9)
                for now in (9, 430, 700):
                    with self.assertRaises(ValueError):
                        guard.require_open_budget(root, plan(), now=now)
                (root / "deadline-fired").touch()
                with self.assertRaises(ValueError):
                    guard.require_open_budget(root, plan(), now=20)

    def test_controller_lock_is_exclusive_and_bounded(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with guard.locked(root):
                with self.assertRaises(TimeoutError):
                    with guard.locked(root, timeout=0.01):
                        self.fail("duplicate controller acquired lock")
            with guard.locked(root):
                pass


if __name__ == "__main__":
    unittest.main()
