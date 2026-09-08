import unittest
from unittest.mock import MagicMock, patch

from tools import status_socket_outage as outage


class StatusSocketOutageTests(unittest.TestCase):
    handoff: MagicMock
    verified_serving: MagicMock
    stopped_and_drained: MagicMock

    def setUp(self):
        self.operation = "a" * 24
        self.state: dict[str, object] = {"deadlineUsec": 120000000, "baseline": {}, "status": {}}
        self.events: list[list[str]] = []
        for name, value in (("handoff", self.state), ("verified_serving", None),
                            ("stopped_and_drained", None)):
            mock = patch.object(outage, name, return_value=value).start()
            setattr(self, name, mock)
        self.guard = patch.object(outage.timer, "guard", return_value=(120000000, 90)).start()
        self.cleanup = patch.object(outage.timer, "cleanup", return_value=True).start()
        def command(argv: list[str], **_kw: object) -> bytes:
            self.events.append(argv)
            return b""
        self.command = patch.object(outage.probe, "command", side_effect=command).start()
        patch.object(outage.probe, "same_identity").start()
        patch.object(outage.resources, "snapshot", return_value={}).start()
        self.addCleanup(patch.stopall)

    def test_stop_is_only_fixed_socket_and_guard_stays(self):
        result = outage.stop(self.operation, self.state, "source")
        self.assertEqual(result["result"], "stopped")
        self.assertIs(result["guardMayRemain"], True)
        self.assertEqual(self.events, [["systemctl", "stop", outage.policy.SOCKET]])
        self.stopped_and_drained.assert_called_once()
        self.cleanup.assert_not_called()

    def test_short_or_changed_guard_never_stops(self):
        for observation in ((120000000, 59), (120000001, 90)):
            self.guard.return_value = observation
            with self.assertRaises(ValueError):
                outage.stop(self.operation, self.state, "source")
        self.command.assert_not_called()

    def test_drain_failure_restores_but_does_not_remove_guard(self):
        self.stopped_and_drained.side_effect = ValueError("worker remains")
        with self.assertRaisesRegex(ValueError, "worker remains"):
            outage.stop(self.operation, self.state, "source")
        self.assertEqual(self.events, [["systemctl", "stop", outage.policy.SOCKET],
                                       ["systemctl", "start", outage.policy.SOCKET]])
        self.cleanup.assert_not_called()

    def test_lost_stop_reply_still_restores(self):
        self.command.side_effect = [outage.subprocess.TimeoutExpired("fixed", 15), b""]
        with self.assertRaises(outage.subprocess.TimeoutExpired):
            outage.stop(self.operation, self.state, "source")
        self.assertEqual(self.command.call_args_list[-1].args[0], outage.policy.restore_command())
        self.cleanup.assert_not_called()

    def test_restore_is_idempotent_and_retains_guard(self):
        for _ in range(2):
            self.assertEqual(outage.restore(self.operation, self.state, "source")["result"], "restored")
        self.assertEqual(self.events, [outage.policy.restore_command()] * 2)
        self.cleanup.assert_not_called()

    def test_failed_verification_prevents_disarm(self):
        self.verified_serving.side_effect = ValueError("identity changed")
        with self.assertRaises(ValueError):
            outage.finish(self.operation, self.state, "source")
        self.assertEqual(self.events, [outage.policy.restore_command()])
        self.cleanup.assert_not_called()

    def test_finish_verifies_before_and_after_cleanup(self):
        result = outage.finish(self.operation, self.state, "source")
        self.assertEqual(result["result"], "finished")
        self.assertIs(result["guardMayRemain"], False)
        self.assertEqual(self.verified_serving.call_count, 2)
        self.cleanup.assert_called_once_with(self.operation)


class StatusWorkerDrainTests(unittest.TestCase):
    def test_pre_arm_quiet_gate_also_waits_for_worker_collection(self):
        values: dict[str, object] = {"ActiveState": "active", "SubState": "listening",
                                    "Accept": True, "MaxConnections": 4, "NConnections": 0}
        def property_value(_unit: str, _interface: str, name: str, sig: str) -> dict[str, object]:
            return {"type": sig, "data": values[name]}
        with patch.object(outage.probe, "property_value", side_effect=property_value), \
             patch.object(outage.probe, "command", side_effect=[b"retained worker\n", b""]) as command, \
             patch.object(outage.time, "sleep") as sleep:
            outage.probe.quiet_socket()
        self.assertEqual(command.call_count, 2)
        sleep.assert_called_once_with(0.25)

    def test_cleanup_allows_restorer_timeout_without_stopping_service(self):
        name = outage.policy.guard_name("a" * 24)
        def absent(unit: str) -> bool:
            return unit == name + ".timer"
        with patch.object(outage.probe, "unit_absent", side_effect=absent), \
             patch.object(outage.probe, "command") as command, \
             patch.object(outage.time, "monotonic", side_effect=[0, 9, 24, 26]), \
             patch.object(outage.time, "sleep") as sleep:
            self.assertFalse(outage.timer.cleanup("a" * 24))
        command.assert_not_called()
        self.assertEqual(sleep.call_count, 2)

    def test_listener_counter_alone_cannot_prove_worker_exit(self):
        replies = {"ActiveState": ("s", "inactive"), "SubState": ("s", "dead"), "NConnections": ("u", 0)}
        def property_value(_unit: str, _interface: str, name: str, _sig: str) -> dict[str, object]:
            signature, value = replies[name]
            return {"type": signature, "data": value}
        with patch.object(outage.probe, "property_value", side_effect=property_value), \
             patch.object(outage.probe, "command", side_effect=[b"retained instance\n", b""]) as command, \
             patch.object(outage.time, "sleep") as sleep:
            outage.stopped_and_drained()
        self.assertEqual(command.call_count, 2)
        sleep.assert_called_once_with(0.25)
        self.assertEqual(command.call_args.args[0][-1], outage.WORKERS)

    def test_live_listener_is_rejected_even_with_no_workers(self):
        values: dict[str, object] = {"ActiveState": "active", "SubState": "listening", "NConnections": 0}
        def property_value(_unit: str, _interface: str, name: str, sig: str) -> dict[str, object]:
            return {"type": sig, "data": values[name]}
        with patch.object(outage.probe, "property_value", side_effect=property_value), \
             patch.object(outage.probe, "command", return_value=b""):
            with self.assertRaisesRegex(ValueError, "status socket did not stop"):
                outage.stopped_and_drained()


if __name__ == "__main__":
    unittest.main()
