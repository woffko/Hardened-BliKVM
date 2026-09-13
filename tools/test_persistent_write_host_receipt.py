import copy
from collections.abc import Callable
import hashlib
import json
import unittest
from typing import cast

from tools import persistent_write_contract as contract
from tools import persistent_write_host_receipt as host

TOOL = "a" * 64
SLOT = "app-00000000000000000010-" + "c" * 64
BINDING = {"transaction_id": "e" * 24, "device_fingerprint": "f" * 64,
           "configuration_sha256": contract.CONFIG_SHA256, "baseline_slot": SLOT,
           "baseline_floor": 10, "baseline_generation": 46}
SOURCES = {path: "d" * 64 for path in host.source_paths()}


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


def document() -> dict[str, object]:
    return {"probe": "persistent-write-host-gate", "schemaVersion": 1, "result": "passed",
            "binding": BINDING, "toolSha256": TOOL,
            "suite": {"command": host.COMMAND, "tests": 123, "result": "passed", "sourceHashes": SOURCES}}


def raw(value: dict[str, object]) -> bytes:
    return json.dumps(value).encode()


class HostReceiptTests(unittest.TestCase):
    def parse(self, value: dict[str, object]) -> host.Contents:
        return host.parse(raw(value), expected_python_sources=SOURCES, expected_tool=TOOL)

    def test_completed_host_gate_is_bound(self) -> None:
        contents = self.parse(document())
        self.assertEqual(contents.binding.baseline_slot, SLOT)
        self.assertEqual(contents.receipt_sha256, hashlib.sha256(raw(document())).hexdigest())

    def test_unreviewed_tool_and_sources_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unreviewed host gate tool"):
            host.parse(raw(document()), expected_python_sources=SOURCES, expected_tool="0" * 64)
        sources = dict(SOURCES)
        sources[next(iter(sources))] = "0" * 64
        with self.assertRaisesRegex(ValueError, "unreviewed host sources"):
            host.parse(raw(document()), expected_python_sources=sources, expected_tool=TOOL)

    def test_failed_or_shallow_suite_is_rejected(self) -> None:
        def failed(value: dict[str, object]) -> None:
            value["result"] = "failed"

        def suite_failed(value: dict[str, object]) -> None:
            section(value, "suite")["result"] = "failed"

        def shallow(value: dict[str, object]) -> None:
            section(value, "suite")["tests"] = 99

        def foreign_command(value: dict[str, object]) -> None:
            section(value, "suite")["command"] = ["python3", "-m", "unittest"]

        mutations: tuple[tuple[Callable[[dict[str, object]], None], str], ...] = (
            (failed, "not a completed host gate"),
            (suite_failed, "host suite command/result differs"),
            (shallow, "coverage below bound"),
            (foreign_command, "command/result differs"),
        )
        for mutation, message in mutations:
            changed = copy.deepcopy(document())
            mutation(changed)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.parse(changed)


if __name__ == "__main__":
    unittest.main()
