from dataclasses import replace
import unittest

from tools import application_write_pipeline as pipeline
from tools.test_application_write_contract import status


class PipelineTests(unittest.TestCase):
    def test_jobs_filter_only_application_work_and_reject_partial_output(self) -> None:
        raw = ("1 unrelated.service start running\n"
               "2 hardened-blikvm-update-watchdog.service start running\n"
               "3 hardened-blikvm-update-apply-confirmed.service start waiting\n"
               "4 hardened-blikvm-update-activation@1-2.service start running\n"
               "5 hardened-blikvm-update-confirm@3.service start running\n"
               "6 hardened-blikvm-update-activate@app-8.service start waiting\n")
        self.assertEqual(pipeline.queued_pipeline_jobs(raw), 4)
        for value in ("garbage", "1 unit start", "1 unit start mystery", "1 unit start running extra",
                      "1 hardened-blikvm-update-ap… start running", "1 hardened-blikvm-update-ap... start running"):
            with self.assertRaises(ValueError):
                pipeline.queued_pipeline_jobs(value)

    def run_observations(self, values: list[pipeline.Observation], saved: list[int]) -> pipeline.contract.TerminalState:
        now = [0.0]
        iterator = iter(values)
        def sleep(seconds: float) -> None:
            now[0] += seconds
        return pipeline.await_terminal(lambda: next(iterator), saved.append, timeout=1,
                                       clock=lambda: now[0], sleep=sleep)

    def test_floor_is_saved_before_apply_settles(self) -> None:
        clean = pipeline.Observation(status(8), 0, 0, False, False, False)
        saved: list[int] = []
        result = self.run_observations([replace(clean, confirmed_apply_present=True), clean, clean], saved)
        self.assertEqual(saved, [8])
        self.assertEqual(result.floor, 8)

    def test_activity_between_terminal_reads_resets_confirmation(self) -> None:
        clean = pipeline.Observation(status(), 0, 0, False, False, False)
        with self.assertRaises(TimeoutError):
            self.run_observations([clean, replace(clean, live_workers=1), clean,
                                   replace(clean, queued_jobs=1)], [])

    def test_floor_regression_is_fatal_even_before_terminal_state(self) -> None:
        eight = pipeline.Observation(status(8), 1, 0, False, False, True)
        seven = replace(eight, status=status())
        saved: list[int] = []
        with self.assertRaisesRegex(ValueError, "floor regressed"):
            self.run_observations([eight, seven], saved)
        self.assertEqual(saved, [8])

    def test_observation_failure_does_not_retry_or_mutate(self) -> None:
        calls: list[int] = []
        def read() -> pipeline.Observation:
            calls.append(1)
            raise OSError("unreadable state")
        with self.assertRaises(OSError):
            pipeline.await_terminal(read, lambda _: self.fail("unexpected persistence"))
        self.assertEqual(calls, [1])


if __name__ == "__main__":
    unittest.main()
