import copy
from collections.abc import Callable
import json
import unittest
from typing import cast

from tools import persistent_write_contract as contract
from tools import persistent_write_upload_receipt as receipt

TOOL = "a" * 64
SLOT = "app-00000000000000000010-" + "c" * 64
BINDING = {"transaction_id": "e" * 24, "device_fingerprint": "f" * 64,
           "configuration_sha256": contract.CONFIG_SHA256, "baseline_slot": SLOT,
           "baseline_floor": 10, "baseline_generation": 46}
SOURCES = {name: "d" * 64 for name in receipt.SOURCES}
ROOT = "/var/lib/" + contract.PREFIX + "e" * 24


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


def document() -> dict[str, object]:
    return {"probe": "persistent-write-upload-gate", "schemaVersion": 1, "result": "passed", "binding": BINDING,
            "guardRoot": ROOT, "profile": "seq10-retry", "archiveSha256": receipt.PROFILES[10][1],
            "modes": {mode: "passed" for mode in receipt.MODES}, "spoolEmpty": True,
            "sourceHashes": SOURCES, "toolSha256": TOOL}


class UploadReceiptTests(unittest.TestCase):
    def parse(self, value: dict[str, object]) -> receipt.Contents:
        return receipt.parse(json.dumps(value).encode(), expected_sources=SOURCES, expected_tool=TOOL)

    def test_completed_upload_gate_is_bound(self) -> None:
        contents = self.parse(document())
        self.assertEqual(contents.binding.baseline_slot, SLOT)
        self.assertEqual(set(contents.modes), set(receipt.MODES))

    def test_unreviewed_tool_and_sources_are_rejected(self) -> None:
        with self.assertRaisesRegex(ValueError, "unreviewed upload gate tool"):
            receipt.parse(json.dumps(document()).encode(), expected_sources=SOURCES, expected_tool="0" * 64)
        sources = dict(SOURCES)
        sources["probe_installed_upload.py"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "unreviewed upload sources"):
            receipt.parse(json.dumps(document()).encode(), expected_sources=sources, expected_tool=TOOL)

    def test_incomplete_or_failed_vectors_are_rejected(self) -> None:
        def missing(value: dict[str, object]) -> None:
            del section(value, "modes")["malformed"]

        def failed(value: dict[str, object]) -> None:
            section(value, "modes")["unauthorized"] = "failed"

        def dirty_spool(value: dict[str, object]) -> None:
            value["spoolEmpty"] = False

        mutations: tuple[tuple[Callable[[dict[str, object]], None], str], ...] = (
            (missing, "upload vectors incomplete"),
            (failed, "upload vectors incomplete"),
            (dirty_spool, "upload spool is not empty"),
        )
        for mutation, message in mutations:
            changed = copy.deepcopy(document())
            mutation(changed)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.parse(changed)

    def test_seq11_profile_is_pinned(self) -> None:
        self.assertEqual(receipt.PROFILES[11][0], "seq11-retry")
        self.assertEqual(receipt.PROFILES[11][1],
                         "e113e4bc1d9091d294ea499e775a03e685f51f75f362c3cbb08c0fbaf005ec40")

    def test_unreviewed_baseline_is_rejected(self) -> None:
        changed = copy.deepcopy(document())
        slot = "app-00000000000000000008-" + "c" * 64
        section(changed, "binding").update(baseline_slot=slot, baseline_floor=8, baseline_generation=40)
        with self.assertRaisesRegex(ValueError, "no reviewed profile"):
            self.parse(changed)


if __name__ == "__main__":
    unittest.main()
