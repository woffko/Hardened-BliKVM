import copy
from collections.abc import Callable
import json
import unittest
from typing import cast

from tools import persistent_write_contract as contract
from tools import persistent_write_gui_gate_receipt as gate
from tools import test_persistent_write_gui_failure_receipt as gui_fixture

TOOL = "a" * 64
SLOT = "app-00000000000000000010-" + "c" * 64
BINDING = {"transaction_id": "e" * 24, "device_fingerprint": "f" * 64,
           "configuration_sha256": contract.CONFIG_SHA256, "baseline_slot": SLOT,
           "baseline_floor": 10, "baseline_generation": 46}
SOURCES = {name: "d" * 64 for name in gate.gui_failure.NEGATIVE_BROWSER_SOURCES}
ROOT = "/var/lib/" + contract.PREFIX + "e" * 24


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


def document() -> dict[str, object]:
    return {"probe": "persistent-write-gui-gate", "schemaVersion": 1, "result": "passed", "binding": BINDING,
            "guardRoot": ROOT, "profile": "seq10-retry", "selection": "signed-seq10",
            "browserSha256": "b" * 64, "browser": gui_fixture.browser(), "sourceHashes": SOURCES,
            "toolSha256": TOOL}


class GuiGateReceiptTests(unittest.TestCase):
    def parse(self, value: dict[str, object]) -> gate.Contents:
        return gate.parse(json.dumps(value).encode(), expected_sources=SOURCES, expected_tool=TOOL)

    def test_completed_gate_is_bound_and_gates_validate(self) -> None:
        contents = self.parse(document())
        self.assertEqual(contents.binding.baseline_slot, SLOT)
        for name in ("gui_failure_retry", "gui_cancel", "gui_logout"):
            gate.validate_gate(name, contents)

    def test_unreviewed_producer_is_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unreviewed GUI gate producer"):
            gate.parse(json.dumps(document()).encode(), expected_sources=SOURCES, expected_tool="0" * 64)
        sources = dict(SOURCES)
        sources["upload_termination.mjs"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "unreviewed GUI gate producer"):
            gate.parse(json.dumps(document()).encode(), expected_sources=sources, expected_tool=TOOL)

    def test_wrong_profile_binding_and_wire_fields_are_rejected(self) -> None:
        def wrong_profile(value: dict[str, object]) -> None:
            value["profile"] = "seq9-retry"

        def wrong_selection(value: dict[str, object]) -> None:
            value["selection"] = "signed-seq9"

        def wrong_binding(value: dict[str, object]) -> None:
            section(value, "binding")["transaction_id"] = "1" * 24

        def wrong_guard_root(value: dict[str, object]) -> None:
            value["guardRoot"] = "/var/lib/hardened-blikvm-write-retain-" + "1" * 24

        def wrong_browser(value: dict[str, object]) -> None:
            section(value, "browser")["cancelProgress"] = 0

        mutations: tuple[tuple[Callable[[dict[str, object]], None], str], ...] = (
            (wrong_profile, "GUI gate profile differs"),
            (wrong_selection, "GUI gate profile differs"),
            (wrong_binding, "GUI gate guard root differs"),
            (wrong_guard_root, "GUI gate guard root differs"),
            (wrong_browser, "negative browser counter differs"),
        )
        for mutation, message in mutations:
            changed = copy.deepcopy(document())
            mutation(changed)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.parse(changed)

    def test_unknown_gate_is_rejected(self) -> None:
        contents = self.parse(document())
        with self.assertRaisesRegex(ValueError, "unknown negative browser gate"):
            gate.validate_gate("native_upload", contents)


if __name__ == "__main__":
    unittest.main()
