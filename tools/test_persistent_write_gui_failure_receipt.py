import copy
from collections.abc import Callable
import unittest
from typing import cast

from tools import persistent_write_contract as contract
from tools import persistent_write_gui_failure_receipt as receipt

RUNNER = "a" * 64
PRODUCERS = {name: "d" * 64 for name in receipt.NEGATIVE_BROWSER_SOURCES}
SLOT = "app-00000000000000000010-" + "c" * 64
BINDING = {"transaction_id": "e" * 24, "device_fingerprint": "f" * 64,
           "configuration_sha256": contract.CONFIG_SHA256, "baseline_slot": SLOT,
           "baseline_floor": 10, "baseline_generation": 46}
ROOT = "/var/lib/" + contract.PREFIX + "e" * 24


def section(value: dict[str, object], key: str) -> dict[str, object]:
    return cast(dict[str, object], value[key])


def status() -> dict[str, object]:
    return {"activeSlotId": SLOT, "confirmedSlotId": SLOT, "floorSequence": 10, "generation": 46,
            "pending": None, "lastRollback": None, "previousSlotId": "app-00000000000000000009-" + "9" * 64}


def browser() -> dict[str, object]:
    return {"probe": "application-upload-failures", "profile": "seq10-retry", "result": "passed",
            "selectionVerified": "signed-seq10", "uploadRequests": 4, "activationRequests": 0,
            "loginRequests": 1, "pageErrors": 0, "blockedMutations": 0, "invalidArchiveRejected": True,
            "invalidCode": -1, "invalidHttpStatus": 200, "cancelled": True, "cancelProgress": 1,
            "retryAlreadyPresent": True, "logoutRequests": 1, "logoutHttpStatus": 200, "logoutProgress": 1,
            "throttleReleasedAfterLogout": True, "uploadHttpStatuses": [200, 200],
            "uploadTransportFailures": ["net::ERR_ABORTED", "net::ERR_ABORTED"],
            "twoPagesOneSession": True, "unauthenticatedStatus": 401,
            "uploadTermination": {"aborted": True, "afterLogoutRequested": True, "kind": "failed"}}


def document() -> dict[str, object]:
    return {"probe": "persistent-write-wiring-rehearsal", "action": "browser", "result": "passed",
            "applicationProfile": "seq10", "applicationUploads": 4, "applicationActivations": 0,
            "runnerSha256": RUNNER, "browserSourceHashes": PRODUCERS,
            "prepareSubmitted": True, "cleanupSubmitted": True, "runtimeWritesClosed": True,
            "controllerStagingRemoved": True, "browserSubmitted": True,
            "guardRetained": False, "retentionApproved": False, "targetPowerControl": False,
            "rebootRequested": False, "guardRoot": ROOT,
            "prepared": {"binding": BINDING, "guardRoot": ROOT, "result": "prepared", "phase": "wired",
                         "deadlineArmed": True, "retentionApproved": False},
            "before": {"bootIdSha256": "b" * 64, "runtimeWritesClosed": True, "status": status()},
            "after": {"bootIdSha256": "b" * 64, "runtimeWritesClosed": True, "status": status()},
            "restored": {"result": "restored", "guardRoot": ROOT, "runtimeVerified": True, "boot": False,
                         "retentionApproved": False},
            "cleaned": {"result": "cleaned", "guardRoot": ROOT,
                        "archiveRoot": "/var/lib/hardened-blikvm-write-restored-" + "e" * 24,
                        "runtimeWritesClosed": True, "floor": 10, "generation": 46},
            "browser": browser()}


def raw(value: dict[str, object]) -> bytes:
    import json
    return json.dumps(value).encode()


class NegativeBrowserReceiptTests(unittest.TestCase):
    def parse(self, value: dict[str, object]) -> receipt.Contents:
        return receipt.parse(raw(value), expected_runner=RUNNER, expected_browser_sources=PRODUCERS)

    def test_completed_negative_rehearsal_is_bound(self) -> None:
        contents = self.parse(document())
        self.assertEqual(contents.binding.baseline_slot, SLOT)
        self.assertEqual(contents.binding.baseline_floor, 10)
        for gate in ("gui_failure_retry", "gui_cancel", "gui_logout"):
            receipt.validate_gate(gate, contents)

    def test_unreviewed_producer_is_rejected(self) -> None:
        producers = dict(PRODUCERS)
        producers["upload_termination.mjs"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "unreviewed receipt producer"):
            receipt.parse(raw(document()), expected_runner=RUNNER, expected_browser_sources=producers)

    def test_failed_or_unfinished_lifecycle_is_rejected(self) -> None:
        def failed(value: dict[str, object]) -> None:
            value["result"] = "failed"

        def unfinished(value: dict[str, object]) -> None:
            value["cleanupSubmitted"] = False

        def retained(value: dict[str, object]) -> None:
            value["retentionApproved"] = True

        def unwired(value: dict[str, object]) -> None:
            section(value, "prepared")["phase"] = "prepared"

        mutations: tuple[Callable[[dict[str, object]], None], ...] = (failed, unfinished, retained, unwired)
        for mutation in mutations:
            changed = copy.deepcopy(document())
            mutation(changed)
            with self.subTest(mutation=mutation.__name__), self.assertRaises(ValueError):
                self.parse(changed)

    def test_gate_specific_fields_are_enforced(self) -> None:
        def rejected(value: dict[str, object]) -> None:
            section(value, "browser")["invalidArchiveRejected"] = False

        def cancelled(value: dict[str, object]) -> None:
            section(value, "browser")["cancelProgress"] = 0

        def termination(value: dict[str, object]) -> None:
            section(value, "browser")["uploadTermination"] = {"aborted": False,
                "afterLogoutRequested": True, "kind": "failed"}

        def aborted(value: dict[str, object]) -> None:
            section(value, "browser")["uploadTransportFailures"] = ["net::ERR_ABORTED"]

        mutations: tuple[tuple[Callable[[dict[str, object]], None], str], ...] = (
            (rejected, "negative browser assertion missing"),
            (cancelled, "negative browser counter differs"),
            (termination, "logout upload termination differs"),
            (aborted, "logout upload termination differs"),
        )
        for mutation, message in mutations:
            changed = copy.deepcopy(document())
            mutation(changed)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                self.parse(changed)

    def test_logout_response_termination_is_accepted(self) -> None:
        def response(value: dict[str, object]) -> None:
            section(value, "browser").update(uploadHttpStatuses=[200, 200, 401],
                uploadTransportFailures=["net::ERR_ABORTED"],
                uploadTermination={"kind": "response", "status": 401, "afterLogoutRequested": True})

        good = copy.deepcopy(document())
        response(good)
        contents = self.parse(good)
        receipt.validate_gate("gui_logout", contents)
        bad = copy.deepcopy(good)
        section(bad, "browser")["uploadHttpStatuses"] = [200, 200]
        with self.assertRaisesRegex(ValueError, "logout response termination differs"):
            self.parse(bad)

    def test_unreviewed_baseline_profile_is_rejected(self) -> None:
        changed = copy.deepcopy(document())
        slot = "app-00000000000000000008-" + "c" * 64
        section(section(changed, "prepared"), "binding").update(baseline_slot=slot, baseline_floor=8,
                                                                baseline_generation=40)
        for side in ("before", "after"):
            section(section(changed, side), "status").update(activeSlotId=slot, confirmedSlotId=slot,
                                                             floorSequence=8, generation=40)
        section(changed, "cleaned").update(floor=8, generation=40)
        with self.assertRaisesRegex(ValueError, "no reviewed baseline profile"):
            self.parse(changed)

    def test_seq11_browser_profile_is_pinned(self) -> None:
        binding = contract.EvidenceBinding("e" * 24, "f" * 64, contract.CONFIG_SHA256,
            "app-00000000000000000011-" + "c" * 64, 11, 49)
        self.assertEqual(receipt.browser_profile(binding), ("seq11-retry", "signed-seq11"))
        self.assertEqual(receipt.GATE_PROFILES[11], "seq11-retry")

    def test_unknown_gate_is_rejected(self) -> None:
        contents = self.parse(document())
        with self.assertRaisesRegex(ValueError, "unknown negative browser gate"):
            receipt.validate_gate("gui_happy_path", contents)


if __name__ == "__main__":
    unittest.main()
