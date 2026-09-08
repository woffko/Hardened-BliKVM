from dataclasses import replace
import unittest

from tools import status_outage_contract as policy

OPERATION = "a" * 24
BASELINE = policy.OutageObservation(True, True, 0, False, 0)
ARMED = policy.OutageObservation(True, True, 0, True, 100)


class OutagePolicyTests(unittest.TestCase):
    def test_fixed_guard_and_single_socket_commands(self) -> None:
        command = policy.arm_command(OPERATION, namespace_unused=True, observation=BASELINE)
        self.assertEqual(command, ["systemd-run", "--quiet", "--collect",
                                  "--unit=" + policy.PREFIX + OPERATION, "--on-active=120s",
                                  "--timer-property=AccuracySec=1s", "--property=Type=oneshot",
                                  "--property=TimeoutStartSec=20s", "/usr/bin/systemctl", "start", policy.SOCKET])
        self.assertEqual(policy.stop_command(ARMED), ["systemctl", "stop", policy.SOCKET])
        self.assertEqual(policy.restore_command(), ["systemctl", "start", policy.SOCKET])
        self.assertEqual(policy.disarm_command(OPERATION, observation=ARMED, restored_status_verified=True),
                         ["systemctl", "stop", policy.PREFIX + OPERATION + ".timer"])

    def test_no_arm_on_foreign_namespace_or_unstable_baseline(self) -> None:
        with self.assertRaises(ValueError):
            policy.arm_command(OPERATION, namespace_unused=False, observation=BASELINE)
        for value in (replace(BASELINE, baseline_matches=False), replace(BASELINE, socket_active=False),
                      replace(BASELINE, active_workers=1), replace(BASELINE, guard_verified=True),
                      replace(BASELINE, guard_remaining_seconds=1)):
            with self.assertRaises(ValueError):
                policy.arm_command(OPERATION, namespace_unused=True, observation=value)

    def test_no_outage_without_fresh_verified_guard_and_idle_workers(self) -> None:
        for value in (BASELINE, replace(ARMED, baseline_matches=False), replace(ARMED, socket_active=False),
                      replace(ARMED, active_workers=1), replace(ARMED, guard_remaining_seconds=59)):
            with self.assertRaises(ValueError):
                policy.stop_command(value)
        self.assertEqual(policy.stop_command(replace(ARMED, guard_remaining_seconds=60)),
                         ["systemctl", "stop", policy.SOCKET])

    def test_no_disarm_before_real_status_restoration(self) -> None:
        with self.assertRaises(ValueError):
            policy.disarm_command(OPERATION, observation=ARMED, restored_status_verified=False)
        for value in (BASELINE, replace(ARMED, baseline_matches=False), replace(ARMED, socket_active=False),
                      replace(ARMED, active_workers=1)):
            with self.assertRaises(ValueError):
                policy.disarm_command(OPERATION, observation=value, restored_status_verified=True)

    def test_invalid_names_and_counters_are_rejected(self) -> None:
        for name in ("", "a" * 23, "A" * 24, "../foreign", "a" * 24 + "\n"):
            with self.assertRaises(ValueError):
                policy.guard_name(name)
        for value in (replace(ARMED, active_workers=True), replace(ARMED, active_workers=-1),
                      replace(ARMED, active_workers=5), replace(ARMED, guard_remaining_seconds=True),
                      replace(ARMED, guard_remaining_seconds=121)):
            with self.assertRaises(ValueError):
                policy.validate_observation(value)


if __name__ == "__main__":
    unittest.main()
