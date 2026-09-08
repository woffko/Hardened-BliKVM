import unittest

from tools.systemd_property import decode_property, guard_remaining_seconds


class PropertyTests(unittest.TestCase):
    def test_actual_device_wire_shapes(self) -> None:
        self.assertEqual(decode_property({"type": "s", "data": "249.11-0ubuntu3.17"}, "s"), "249.11-0ubuntu3.17")
        self.assertIs(decode_property({"type": "b", "data": False}, "b"), False)
        self.assertEqual(decode_property({"type": "u", "data": 4}, "u"), 4)
        self.assertEqual(decode_property({"type": "t", "data": 260100855997}, "t"), 260100855997)
        entries = [["OnUnitActiveUSec", 86400000000, 260100855997], ["OnBootUSec", 900000000, 900000000]]
        self.assertEqual(decode_property({"type": "a(stt)", "data": entries}, "a(stt)"), entries)

    def test_wrong_scalar_types_bounds_and_envelopes_fail_closed(self) -> None:
        for signature, data in (("u", True), ("t", -1), ("t", 2**64), ("u", 2**32),
                                ("t", "123"), ("t", 1.5), ("b", 1), ("s", "bad\nstring"), ("s", "a" * 513)):
            with self.subTest(signature=signature), self.assertRaises(ValueError):
                decode_property({"type": signature, "data": data}, signature)
        invalid: list[object] = [[], {}, {"type": "t", "data": 1}, {"type": "u", "data": 1, "extra": 2}]
        for value in invalid:
            with self.assertRaises(ValueError):
                decode_property(value, "u")

    def test_monotonic_margin_never_uses_realtime_or_boottime(self) -> None:
        spec = {"type": "a(stt)", "data": [["OnActiveUSec", 120000000, 200000000]]}
        deadline = {"type": "t", "data": 200000000}
        wake = {"type": "b", "data": False}
        self.assertEqual(guard_remaining_seconds(specification=spec, deadline=deadline, wake_system=wake,
                                                 now_usec=140000001), 59)
        for now in (0, 79999999, 200000000, 210000000, True):
            with self.assertRaises(ValueError):
                guard_remaining_seconds(specification=spec, deadline=deadline, wake_system=wake, now_usec=now)
        with self.assertRaises(ValueError):
            guard_remaining_seconds(specification=spec, deadline=deadline,
                                    wake_system={"type": "b", "data": True}, now_usec=100000000)
        with self.assertRaises(ValueError):
            guard_remaining_seconds(specification=spec, deadline={"type": "t", "data": 0},
                                    wake_system=wake, now_usec=100000000)

    def test_extra_or_changed_timer_expressions_are_not_guard_evidence(self) -> None:
        good = ["OnActiveUSec", 120000000, 200000000]
        invalid: list[object] = [[], [good, good], [["OnBootUSec", 120000000, 200000000]],
                                [["OnActiveUSec", 119000000, 200000000]],
                                [["OnActiveUSec", 120000000, 200000001]], [["OnActiveUSec", True, 200000000]]]
        for data in invalid:
            with self.assertRaises(ValueError):
                guard_remaining_seconds(specification={"type": "a(stt)", "data": data},
                                        deadline={"type": "t", "data": 200000000},
                                        wake_system={"type": "b", "data": False}, now_usec=100000000)


if __name__ == "__main__":
    unittest.main()
