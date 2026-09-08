import contextlib
import copy
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from tools import status_timer_firing as firing
from tools.test_retained_resource_baseline import fixture as resources
from tools.test_retained_backend_load import status

OPERATION = "a" * 24


def handoff() -> dict[str, object]:
    return {"result": "armed", "operation": OPERATION, "baseline": resources()[0],
            "status": status(), "deadlineUsec": 200_000_000}


class FiringTests(unittest.TestCase):
    def test_handoff_binds_operation_status_and_real_numeric_identities(self) -> None:
        self.assertEqual(firing.validate_handoff(handoff(), OPERATION), handoff())
        invalid_values: tuple[tuple[str, object], ...] = (("operation", "foreign"), ("deadlineUsec", True),
                                                         ("result", "passed"), ("baseline", {}), ("status", {}))
        for key, value in invalid_values:
            invalid = handoff()
            invalid[key] = value
            with self.assertRaises(ValueError):
                firing.validate_handoff(invalid, OPERATION)

    def test_arm_returns_live_handoff_without_disarming(self) -> None:
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(firing.os, "geteuid", return_value=0))
            stack.enter_context(patch.object(firing.os, "uname", return_value=SimpleNamespace(machine="aarch64")))
            stack.enter_context(patch.object(firing.resources, "snapshot", return_value=resources()[0]))
            stack.enter_context(patch.object(firing.probe, "status_read", return_value=status()))
            stack.enter_context(patch.object(firing.probe, "quiet_socket"))
            stack.enter_context(patch.object(firing.probe, "namespace_pristine"))
            stack.enter_context(patch.object(firing.probe, "unit_absent", return_value=True))
            command = stack.enter_context(patch.object(firing.probe, "command", return_value=b""))
            stack.enter_context(patch.object(firing, "guard", return_value=(200_000_000, 100)))
            cleanup = stack.enter_context(patch.object(firing, "cleanup", return_value=True))
            result = firing.arm(OPERATION, "synthetic status reader")
        firing.validate_handoff(result, OPERATION)
        cleanup.assert_not_called()
        self.assertEqual(command.call_count, 1)
        self.assertEqual(command.call_args.args[0][0], "systemd-run")

    def test_observer_never_starts_restorer_and_cleans_up_on_failed_observation(self) -> None:
        for failure in ("none", "event", "deadline", "handoff"):
            subscription, monitor = MagicMock(), MagicMock()
            subscription.__enter__.return_value = subscription
            subscription.process.returncode = 0
            monitor.__enter__.return_value = monitor
            monitor.completed.return_value = {"result": "done", "jobId": 123}
            if failure == "event":
                monitor.completed.side_effect = ValueError("synthetic failed event")
            state = copy.deepcopy(handoff())
            if failure == "handoff":
                state["operation"] = "foreign"
            with contextlib.ExitStack() as stack:
                stack.enter_context(patch.object(firing.os, "geteuid", return_value=0))
                stack.enter_context(patch.object(firing.os, "uname", return_value=SimpleNamespace(machine="aarch64")))
                stack.enter_context(patch.object(firing.resources, "snapshot", return_value=resources()[0]))
                stack.enter_context(patch.object(firing.probe, "status_read", return_value=status()))
                stack.enter_context(patch.object(firing.probe, "quiet_socket"))
                command = stack.enter_context(patch.object(firing.probe, "command"))
                stack.enter_context(patch.object(firing, "SubscriptionProcess", return_value=subscription))
                stack.enter_context(patch.object(firing, "RestoreMonitor", return_value=monitor))
                stack.enter_context(patch.object(firing, "manager_owner", return_value=":1.0"))
                stack.enter_context(patch.object(firing, "guard", return_value=(201_000_000 if failure == "deadline" else 200_000_000, 100)))
                cleanup = stack.enter_context(patch.object(firing, "cleanup", return_value=True))
                result = firing.observe(OPERATION, state, "synthetic status reader", "synthetic subscriber")
            command.assert_not_called()
            self.assertEqual(result["result"], "passed" if failure == "none" else "failed")
            if failure == "handoff":
                cleanup.assert_not_called()
                self.assertTrue(result["guardMayRemain"])
            else:
                cleanup.assert_called_once_with(OPERATION)
                self.assertFalse(result["guardMayRemain"])


if __name__ == "__main__":
    unittest.main()
