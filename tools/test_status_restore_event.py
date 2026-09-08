import copy
import json
from typing import Any
import unittest

from tools import status_restore_event as event
from tools.status_outage_contract import guard_name

OPERATION = "a" * 24


def fixture() -> dict[str, Any]:
    return {"type": "signal", "endian": "l", "flags": 1, "version": 1, "cookie": 55,
            "timestamp-realtime": 1000000, "sender": ":1.0", "path": event.MANAGER_PATH,
            "interface": event.MANAGER_INTERFACE, "member": "JobRemoved",
            "payload": {"type": "uoss", "data": [123, event.MANAGER_PATH + "/job/123",
                                                    guard_name(OPERATION) + ".service", "done"]}}


class EventTests(unittest.TestCase):
    def test_one_exact_match_and_no_broad_service_operand(self) -> None:
        argv = event.monitor_command(OPERATION)
        self.assertEqual(argv[-1], "monitor")
        self.assertEqual(argv[:5], ["/usr/bin/busctl", "--system", "--address=" + event.SYSTEM_BUS_ADDRESS,
                                   "--no-pager", "--json=short"])
        self.assertEqual(len(argv), 7)
        self.assertEqual(argv[5], "--match=type='signal',sender='org.freedesktop.systemd1',"
                         "path='/org/freedesktop/systemd1',interface='org.freedesktop.systemd1.Manager',"
                         "member='JobRemoved',arg2='" + guard_name(OPERATION) + ".service'")
        with self.assertRaises(ValueError):
            event.monitor_command("a" * 24 + "',arg3='done")

    def test_matching_success_returns_only_bounded_job_identity(self) -> None:
        value = fixture()
        self.assertEqual(event.parse_completed_job(json.dumps(value).encode(), OPERATION, ":1.0"),
                         {"jobId": 123, "unit": guard_name(OPERATION) + ".service", "result": "done"})
        value.update({"monotonic": 250000, "realtime": 1000000, "seqnum": 1})
        self.assertEqual(event.parse_completed_job(json.dumps(value).encode(), OPERATION, ":1.0")["jobId"], 123)

    def test_wrong_sender_unit_result_path_signature_and_numeric_types_rejected(self) -> None:
        changes: dict[str, object] = {"sender": ":1.2", "member": "JobNew", "path": "/foreign",
                                      "interface": "foreign", "type": "method_call", "cookie": True,
                                      "version": True, "timestamp-realtime": -1, "foreign": "field"}
        for key, wrong in changes.items():
            value = fixture()
            value[key] = wrong
            with self.subTest(key=key), self.assertRaises(ValueError):
                event.parse_completed_job(json.dumps(value).encode(), OPERATION, ":1.0")
        for index, wrong in ((0, True), (0, 2**32), (1, "/foreign"), (2, "foreign.service"),
                             (3, "failed"), (3, "canceled"), (3, "timeout")):
            value = fixture()
            value["payload"]["data"][index] = wrong
            with self.subTest(index=index), self.assertRaises(ValueError):
                event.parse_completed_job(json.dumps(value).encode(), OPERATION, ":1.0")
        value = copy.deepcopy(fixture())
        value["payload"]["type"] = "ssss"
        with self.assertRaises(ValueError):
            event.parse_completed_job(json.dumps(value).encode(), OPERATION, ":1.0")

    def test_oversize_duplicate_and_missing_event_rejected(self) -> None:
        for raw in (b"", b"x" * (event.MAX_EVENT_BYTES + 1), b'{}', b'{"type":"signal","type":"signal"}'):
            with self.assertRaises(ValueError):
                event.parse_completed_job(raw, OPERATION, ":1.0")


if __name__ == "__main__":
    unittest.main()
