from dataclasses import replace
import unittest

from tools import persistent_write_pipeline as pipeline


def observation(floor: int = 8, generation: int = 40) -> pipeline.observations.Observation:
    slot = f"app-{floor:020d}-" + "a" * 64
    return pipeline.observations.Observation({"floorSequence": floor, "generation": generation,
        "activeSlotId": slot, "confirmedSlotId": slot, "pending": None}, 0, 0, False, False, False)


class PipelineTests(unittest.TestCase):
    def run_values(self, values: list[pipeline.observations.Observation], saved: list[pipeline.Counters]) -> pipeline.observations.contract.TerminalState:
        now = [0.0]
        iterator = iter(values)
        def sleep(seconds: float) -> None:
            now[0] += seconds
        return pipeline.await_terminal(lambda: next(iterator), saved.append, observed=pipeline.Counters(8, 40),
            timeout=1, clock=lambda: now[0], sleep=sleep)

    def test_newer_confirmed_slot_is_preserved_not_reverted_to_eight(self) -> None:
        newer = observation(12, 56)
        saved: list[pipeline.Counters] = []
        result = self.run_values([replace(newer, confirmed_apply_present=True), newer, newer], saved)
        self.assertEqual(saved, [pipeline.Counters(12, 56)])
        self.assertEqual((result.floor, result.generation), (12, 56))

    def test_floor_or_generation_regression_is_fatal_while_busy(self) -> None:
        for regressed in (observation(8, 43), observation(9, 41)):
            with self.subTest(status=regressed.status):
                with self.assertRaisesRegex(ValueError, "regressed"):
                    self.run_values([replace(observation(9, 42), live_workers=1), regressed], [])

    def test_activity_resets_two_read_settlement(self) -> None:
        clean = observation()
        with self.assertRaises(TimeoutError):
            self.run_values([clean, replace(clean, live_workers=1), clean, replace(clean, queued_jobs=1)], [])

    def test_bad_types_and_oversize_counters_fail_closed(self) -> None:
        for field in ("floorSequence", "generation"):
            for value in (True, 0, -1, "8", 1 << 64):
                clean = observation()
                clean.status[field] = value
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    self.run_values([clean], [])

    def test_settled_pointer_mismatch_is_not_accepted(self) -> None:
        clean = observation()
        clean.status["activeSlotId"] = observation(9).status["activeSlotId"]
        with self.assertRaisesRegex(ValueError, "disagree"):
            self.run_values([clean], [])

    def test_persistence_failure_stops_observation(self) -> None:
        calls: list[int] = []
        def read() -> pipeline.observations.Observation:
            calls.append(1)
            return observation(9, 43)
        def fail(_: pipeline.Counters) -> None:
            raise OSError("durable marker unavailable")
        with self.assertRaises(OSError):
            pipeline.await_terminal(read, fail, observed=pipeline.Counters(8, 40))
        self.assertEqual(calls, [1])
