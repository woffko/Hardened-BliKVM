import subprocess
from typing import Any, cast
import unittest
from unittest.mock import patch

from tools import subscription_process as worker

READY = "import sys; print('SUBSCRIBED',flush=True); sys.stdin.buffer.read(1)"


class ProcessTests(unittest.TestCase):
    def test_live_worker_closes_on_lease_eof(self) -> None:
        with worker.SubscriptionProcess(READY) as subscription:
            subscription.ensure_alive()
        self.assertEqual(subscription.process.returncode, 0)
        self.assertTrue(subscription.process.stdout is not None and subscription.process.stdout.closed)
        subscription.close()
        with self.assertRaises(RuntimeError):
            subscription.ensure_alive()

    def test_blocked_setup_is_killed_within_parent_deadline(self) -> None:
        created: list[subprocess.Popen[bytes]] = []
        original = subprocess.Popen
        def spawn(argv: list[str], **kwargs: Any) -> subprocess.Popen[bytes]:
            self.assertNotIn("text", kwargs)
            self.assertNotIn("encoding", kwargs)
            process = cast(subprocess.Popen[bytes], original(argv, **kwargs))
            created.append(process)
            return process
        with patch.object(worker, "STARTUP_SECONDS", 0.05), patch.object(worker.subprocess, "Popen", side_effect=spawn):
            with self.assertRaises(TimeoutError):
                worker.SubscriptionProcess("import time; time.sleep(30)")
        self.assertEqual(len(created), 1)
        self.assertIsNotNone(created[0].poll())

    def test_independent_lifetime_expiry_is_detected(self) -> None:
        with patch.object(worker, "LIFETIME_SECONDS", 1):
            with worker.SubscriptionProcess(READY) as subscription:
                self.assertEqual(subscription.process.wait(timeout=4), 124)
                with self.assertRaises(RuntimeError):
                    subscription.ensure_alive()

    def test_bad_ready_marker_is_rejected_and_worker_reaped(self) -> None:
        with self.assertRaises(RuntimeError):
            worker.SubscriptionProcess("print('WRONG',flush=True)")


if __name__ == "__main__":
    unittest.main()
