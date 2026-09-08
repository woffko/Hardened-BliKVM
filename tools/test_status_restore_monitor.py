import json
import os
import signal
import subprocess
import sys
import time
import unittest
from unittest.mock import MagicMock, patch

from tools import status_restore_monitor as monitor
from tools.test_status_restore_event import fixture, OPERATION


class MonitorTests(unittest.TestCase):
    def test_real_child_exits_on_close_and_independent_hard_timeout(self) -> None:
        code = "import sys,time; sys.stderr.write('Monitoring bus message stream.\\n'); sys.stderr.flush(); time.sleep(30)"
        for hard_timeout in (False, True):
            with patch.object(monitor, "monitor_command", return_value=[sys.executable, "-B", "-c", code]), \
                    patch.object(monitor, "LIFETIME_SECONDS", 1):
                with monitor.RestoreMonitor(OPERATION) as observer:
                    if hard_timeout:
                        self.assertEqual(observer.process.wait(timeout=4), 124)
                self.assertIsNotNone(observer.process.poll())

    def test_bounded_record_reads_eof_oversize_and_timeout(self) -> None:
        for data, limit in ((b"", 10), (b"truncated", 10), (b"oversize\n", 3)):
            read, write = os.pipe()
            with os.fdopen(read, "rb", buffering=0) as stream:
                os.write(write, data)
                os.close(write)
                with self.assertRaises(ValueError):
                    monitor.read_record(stream, time.monotonic() + 1, limit)
        read, write = os.pipe()
        try:
            with os.fdopen(read, "rb", buffering=0) as stream:
                with self.assertRaises(TimeoutError):
                    monitor.read_record(stream, time.monotonic() - 1, 10)
        finally:
            os.close(write)

    def test_exact_command_minimal_environment_and_successful_event(self) -> None:
        child = MagicMock()
        child.poll.return_value = None
        raw = json.dumps(fixture()).encode() + b"\n"
        with patch.object(monitor.subprocess, "Popen", return_value=child) as spawn, \
                patch.object(monitor, "read_record", side_effect=[monitor.READY, raw]), \
                patch.object(monitor.time, "monotonic_ns", side_effect=[100_000_000_000, 121_000_000_000]), \
                patch.object(monitor.os, "killpg") as kill:
            with monitor.RestoreMonitor(OPERATION) as observer:
                value = observer.completed(":1.0", 120_000_000)
                self.assertEqual(value["jobId"], 123)
                self.assertEqual(value["receivedMonotonicUsec"], 121_000_000)
            kill.assert_called_once_with(child.pid, signal.SIGTERM)
        argv = spawn.call_args.args[0]
        self.assertEqual(argv[:4], ["/usr/bin/timeout", "--signal=TERM", "--kill-after=2s", "150s"])
        self.assertEqual(argv[-1], "monitor")
        self.assertEqual(sum(argument.startswith("--match=") for argument in argv), 1)
        self.assertTrue(spawn.call_args.kwargs["start_new_session"])
        self.assertNotIn("HOME", spawn.call_args.kwargs["env"])
        child.stdout.close.assert_called_once()
        child.stderr.close.assert_called_once()

    def test_registration_failure_closes_and_escalates_only_owned_group(self) -> None:
        child = MagicMock()
        child.poll.return_value = None
        child.wait.side_effect = [subprocess.TimeoutExpired("synthetic", 3), 0]
        with patch.object(monitor.subprocess, "Popen", return_value=child), \
                patch.object(monitor, "read_record", return_value=b"unexpected\n"), \
                patch.object(monitor.os, "killpg") as kill, self.assertRaises(ValueError):
            monitor.RestoreMonitor(OPERATION)
        self.assertEqual([call.args for call in kill.call_args_list],
                         [(child.pid, signal.SIGTERM), (child.pid, signal.SIGKILL)])
        child.stdout.close.assert_called_once()
        child.stderr.close.assert_called_once()

    def test_early_job_is_not_accepted_as_timer_execution(self) -> None:
        child = MagicMock()
        child.poll.return_value = None
        with patch.object(monitor.subprocess, "Popen", return_value=child), \
                patch.object(monitor, "read_record", side_effect=[monitor.READY, json.dumps(fixture()).encode() + b"\n"]), \
                patch.object(monitor.time, "monotonic_ns", side_effect=[100_000_000_000, 110_000_000_000]), \
                patch.object(monitor.os, "killpg"):
            with monitor.RestoreMonitor(OPERATION) as observer, self.assertRaises(ValueError):
                observer.completed(":1.0", 120_000_000)


if __name__ == "__main__":
    unittest.main()
