from collections.abc import Callable
import unittest
from typing import cast

from tools import observe_retained_application_status as observer

SLOT = "app-" + "0" * 20 + "-" + "a" * 64


def status() -> dict[str, object]:
    return {"status": {"activeSlotId": SLOT, "confirmedSlotId": SLOT, "floorSequence": 10,
                       "generation": 46, "pending": None, "lastRollback": None},
            "bootIdSha256": "b" * 64}


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


class RetainedStatusTests(unittest.TestCase):
    def test_valid_status_is_accepted(self) -> None:
        self.assertEqual(observer.validate_status(status()), status())

    def test_mismatched_or_invalid_status_is_rejected(self) -> None:
        def mismatched(value: dict[str, object]) -> None:
            section(value, "status")["confirmedSlotId"] = "app-" + "0" * 20 + "-" + "c" * 64

        def zero_generation(value: dict[str, object]) -> None:
            section(value, "status")["generation"] = 0

        def missing_pending(value: dict[str, object]) -> None:
            del section(value, "status")["pending"]

        def bad_boot(value: dict[str, object]) -> None:
            value["bootIdSha256"] = "short"

        mutations: tuple[Callable[[dict[str, object]], None], ...] = (
            mismatched, zero_generation, missing_pending, bad_boot)
        for mutation in mutations:
            changed = status()
            mutation(changed)
            with self.subTest(mutation=mutation.__name__), self.assertRaises(ValueError):
                observer.validate_status(changed)


if __name__ == "__main__":
    unittest.main()
