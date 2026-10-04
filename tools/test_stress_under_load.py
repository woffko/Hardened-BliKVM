"""The load tool counts the runs of a command beside busy loops, and leaves no
loop behind: not after its runs, not on SIGTERM or SIGINT, not on SIGKILL
(scripts/stress-under-load.sh, A1 of docs/baselines/work-plan-20261004-review-fixes.md)."""

import os
import shutil
import signal
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / "scripts/stress-under-load.sh"
CORES = len(os.sched_getaffinity(0))
# A command that runs long enough to be stopped, and tells itself from every
# other `sleep`: its seconds end in this process's number.
LONG = f"11.{os.getpid()}"


def run(arguments: list[str], **env: str) -> subprocess.CompletedProcess[str]:
    environment = dict(os.environ, STRESS_WARMUP="0", **env)
    return subprocess.run(
        ["sh", str(TOOL), *arguments],
        capture_output=True,
        text=True,
        env=environment,
        timeout=120,
    )


def last_line(result: subprocess.CompletedProcess[str]) -> str:
    return result.stdout.strip().splitlines()[-1]


def counted(text: str) -> int:
    """How many processes have `text` in their command line."""
    found = subprocess.run(["pgrep", "-f", text], capture_output=True, text=True)
    return len(found.stdout.split())


@unittest.skipUnless(shutil.which("pgrep"), "pgrep is needed to count the loops")
class StressUnderLoadTests(unittest.TestCase):
    def test_runs_that_pass_are_counted(self) -> None:
        result = run(["3", "1", "--", "true"])
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(last_line(result), "stress: 3 runs, 3 passed, 0 failed (1 busy loops)")

    def test_a_failing_run_is_counted_and_its_output_shown(self) -> None:
        result = run(["2", "1", "--", "sh", "-c", "echo the-reason; exit 3"])
        self.assertEqual(result.returncode, 1)
        self.assertEqual(last_line(result), "stress: 2 runs, 0 passed, 2 failed (1 busy loops)")
        self.assertIn("run 1: failed (status 3)", result.stdout)
        self.assertIn("the-reason", result.stdout)

    def test_one_run_of_a_series_may_fail(self) -> None:
        script = 'n=$(cat "$1" 2>/dev/null || echo 0); echo $((n + 1)) > "$1"; [ "$n" -ne 1 ]'
        with tempfile.TemporaryDirectory() as tmp:
            result = run(["3", "1", "--", "sh", "-c", script, "sh", str(Path(tmp) / "count")])
        self.assertEqual(result.returncode, 1)
        self.assertEqual(last_line(result), "stress: 3 runs, 2 passed, 1 failed (1 busy loops)")
        self.assertIn("run 2: failed", result.stdout)
        self.assertIn("run 3: passed", result.stdout)

    @unittest.skipUnless(shutil.which("timeout"), "timeout is needed for the time limit")
    def test_a_run_over_its_time_limit_fails(self) -> None:
        began = time.monotonic()
        result = run(["1", "1", "--", "sleep", "20"], STRESS_TIMEOUT="1")
        self.assertLess(time.monotonic() - began, 15)
        self.assertEqual(result.returncode, 1)
        self.assertIn("run 1: failed (status 124)", result.stdout)
        self.assertEqual(last_line(result), "stress: 1 runs, 0 passed, 1 failed (1 busy loops)")

    def test_the_whole_output_of_every_run_can_be_kept(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            result = run(["2", "1", "--", "echo", "kept"], STRESS_LOGS=str(Path(tmp) / "logs"))
            self.assertEqual(result.returncode, 0, result.stderr)
            kept = sorted(path.name for path in (Path(tmp) / "logs").iterdir())
            self.assertEqual(kept, ["run-1.log", "run-2.log"])
            self.assertEqual((Path(tmp) / "logs/run-2.log").read_text(), "kept\n")

    def test_the_load_is_one_and_a_half_times_the_cores_where_it_is_left_out(self) -> None:
        result = run(["1", "--", "true"])
        loops = max(1, CORES * 3 // 2)
        self.assertEqual(last_line(result), f"stress: 1 runs, 1 passed, 0 failed ({loops} busy loops)")

    def test_the_arguments_are_checked_before_anything_runs(self) -> None:
        for arguments in (
            [],
            ["3"],
            ["3", "--"],
            ["0", "--", "true"],
            ["x", "1", "--", "true"],
            ["3", "y", "--", "true"],
            ["3", "1", "true"],
            ["10001", "1", "--", "true"],
        ):
            with self.subTest(arguments=arguments):
                self.assertEqual(run(arguments).returncode, 2)
        result = run(["1", str(CORES * 4 + 1), "--", "true"])
        self.assertEqual(result.returncode, 2)
        self.assertIn("more than four times", result.stderr)

    def start(self) -> subprocess.Popen[bytes]:
        """The tool, running a long command beside three loops."""
        environment = dict(os.environ, STRESS_WARMUP="0")
        # SIGINT may have been ignored for this test's own process, as it is
        # for a job started in the background; the tool is to see it.
        tool = subprocess.Popen(
            ["sh", str(TOOL), "3", "3", "--", "sleep", LONG],
            env=environment,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            preexec_fn=lambda: signal.signal(signal.SIGINT, signal.SIG_DFL),
        )
        self.addCleanup(subprocess.run, ["pkill", "-f", f"sleep {LONG}"])
        self.addCleanup(tool.wait)
        self.addCleanup(tool.kill)
        self.until(lambda: counted(self.marker(tool)) == 3, "the three loops did not start")
        return tool

    @staticmethod
    def marker(tool: subprocess.Popen[bytes]) -> str:
        return f"stress-under-load-loop-{tool.pid}"

    def until(self, condition, what: str, seconds: float = 15) -> None:
        deadline = time.monotonic() + seconds
        while not condition():
            self.assertLess(time.monotonic(), deadline, what)
            time.sleep(0.05)

    def test_the_loops_and_the_run_end_on_sigterm_and_on_sigint(self) -> None:
        for signum, status in ((signal.SIGTERM, 143), (signal.SIGINT, 130)):
            with self.subTest(signal=signum.name):
                tool = self.start()
                tool.send_signal(signum)
                self.assertEqual(tool.wait(timeout=15), status)
                self.until(
                    lambda: counted(self.marker(tool)) == 0,
                    "a loop is left behind",
                )
                self.until(lambda: counted(f"sleep {LONG}") == 0, "the run was left running")

    def test_the_loops_end_by_themselves_when_the_tool_is_killed(self) -> None:
        tool = self.start()
        tool.kill()
        tool.wait()
        self.until(lambda: counted(self.marker(tool)) == 0, "a loop outlived its tool")


if __name__ == "__main__":
    unittest.main()
