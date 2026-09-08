import copy
from typing import Any
import unittest

from tools import status_restore_properties as guard
from tools import status_outage_contract as policy
from tools.systemd_property import decode_property

OPERATION = "a" * 24


def fixture() -> tuple[dict[str, Any], dict[str, Any]]:
    common: dict[str, Any] = {"LoadState": "loaded", "Transient": True, "DropInPaths": []}
    name = policy.guard_name(OPERATION)
    timer_values = {**common, "Id": name + ".timer", "ActiveState": "active", "SubState": "waiting",
                    "Unit": name + ".service", "WakeSystem": False, "AccuracyUSec": 1_000_000,
                    "RandomizedDelayUSec": 0, "TimersCalendar": [], "NextElapseUSecRealtime": 0,
                    "TimersMonotonic": [["OnActiveUSec", 120_000_000, 200_000_000]],
                    "NextElapseUSecMonotonic": 200_000_000}
    service_values = {**common, "Id": name + ".service", "ActiveState": "inactive", "SubState": "dead",
                      "Type": "oneshot", "User": "", "Group": "", "Restart": "no", "RemainAfterExit": False,
                      "TimeoutStartUSec": 20_000_000,
                      "ExecStart": [["/usr/bin/systemctl", ["/usr/bin/systemctl", "start", policy.SOCKET],
                                     False, 0, 0, 0, 0, 0, 0, 0]],
                      **{key: [] for key in guard.EXTRA_COMMANDS}}
    timer_signatures = {**guard.UNIT_PROPERTIES, **guard.TIMER_PROPERTIES}
    service_signatures = {**guard.UNIT_PROPERTIES, **guard.SERVICE_PROPERTIES}
    return ({key: {"type": timer_signatures[key], "data": value} for key, value in timer_values.items()},
            {key: {"type": service_signatures[key], "data": value} for key, value in service_values.items()})


class GuardPropertyTests(unittest.TestCase):
    def test_exact_unrun_transient_pair(self) -> None:
        timer, service = fixture()
        self.assertEqual(guard.verify_guard_properties(OPERATION, timer, service, now_usec=100_000_000), 100)
        service["User"]["data"] = service["Group"]["data"] = "root"
        self.assertEqual(guard.verify_guard_properties(OPERATION, timer, service, now_usec=140_000_001), 59)

    def test_foreign_modified_or_expired_timer_is_not_authority_to_stop(self) -> None:
        changes: dict[str, object] = {"Id": "foreign.timer", "Transient": False,
                                      "DropInPaths": ["/run/systemd/system/foreign.conf"],
                                      "ActiveState": "inactive", "SubState": "elapsed", "LoadState": "not-found",
                                      "Unit": "foreign.service", "AccuracyUSec": 60_000_000,
                                      "RandomizedDelayUSec": 1, "NextElapseUSecRealtime": 1,
                                      "WakeSystem": True, "NextElapseUSecMonotonic": 1,
                                      "TimersCalendar": [["daily", "daily", 1]]}
        for key, value in changes.items():
            timer, service = fixture()
            timer[key]["data"] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                guard.verify_guard_properties(OPERATION, timer, service, now_usec=100_000_000)

    def test_alternate_commands_privilege_settings_and_prior_execution_rejected(self) -> None:
        changes: dict[str, object] = {"Type": "simple", "User": "foreign", "Group": "foreign",
                                      "Restart": "always", "RemainAfterExit": True, "TimeoutStartUSec": 0,
                                      "Transient": False, "Id": "foreign.service", "SubState": "running"}
        for key, value in changes.items():
            timer, service = fixture()
            service[key]["data"] = value
            with self.subTest(key=key), self.assertRaises(ValueError):
                guard.verify_guard_properties(OPERATION, timer, service, now_usec=100_000_000)
        for index, value in ((0, "/bin/sh"), (1, ["systemctl", "restart", policy.SOCKET]),
                             (2, True), (3, 1), (7, 123), (8, 1), (9, 1)):
            timer, service = fixture()
            service["ExecStart"]["data"][0][index] = value
            with self.subTest(index=index), self.assertRaises(ValueError):
                guard.verify_guard_properties(OPERATION, timer, service, now_usec=100_000_000)
        for key in guard.EXTRA_COMMANDS:
            timer, service = fixture()
            service[key] = copy.deepcopy(service["ExecStart"])
            with self.subTest(extra=key), self.assertRaises(ValueError):
                guard.verify_guard_properties(OPERATION, timer, service, now_usec=100_000_000)

    def test_missing_extra_and_malformed_command_properties_fail_closed(self) -> None:
        timer, service = fixture()
        for change in ("missing", "extra", "boolean-counter", "too-many-fields", "wrong-signature"):
            altered = copy.deepcopy(service)
            if change == "missing":
                del altered["ExecCondition"]
            elif change == "extra":
                altered["extra"] = {"type": "s", "data": "unknown"}
            elif change == "boolean-counter":
                altered["ExecStart"]["data"][0][3] = False
            elif change == "too-many-fields":
                altered["ExecStart"]["data"][0].append(0)
            else:
                altered["ExecStart"]["type"] = "s"
            with self.subTest(change=change), self.assertRaises(ValueError):
                guard.verify_guard_properties(OPERATION, timer, altered, now_usec=100_000_000)
        with self.assertRaises(ValueError):
            decode_property({"type": "as", "data": ["valid", 5]}, "as")


if __name__ == "__main__":
    unittest.main()
