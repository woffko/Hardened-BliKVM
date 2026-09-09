import copy
from pathlib import Path
import unittest

from tools import application_update_flow as flow

GUARD = "/var/lib/hardened-blikvm-application-update-" + "a" * 24


def response(action: str, floor: int, *, passed: bool = True, guard: str = GUARD) -> dict[str, object]:
    profile = flow.native.PROFILES[flow.PROFILE]
    slot = profile.source_slot if floor == 5 else profile.target_slot
    return {"probe": "installed-application-update", "profile": flow.PROFILE, "action": action,
            "bundleSha256": profile.bundle_sha, "guardRoot": guard, "result": "passed" if passed else "failed",
            "controllerStagingRemoved": True, "device": {
                "result": {"prepare": "rehearsed", "recover": "recovered"}.get(action, action),
                "phase": {"prepare": "rehearsed", "activate": "confirmed", "commit": "committed-update",
                          "recover": "recovered", "cleanup": "committed-update" if floor == 6 else "recovered"}[action],
                "verification": {"accountTlsTrustPeersPreserved": True,
                                 "status": {"floorSequence": floor, "activeSlotId": slot, "confirmedSlotId": slot,
                                            "pending": None, "lastRollback": "health_timeout" if floor == 5 else None}}}}


class ApplicationFlowTests(unittest.TestCase):
    def test_success_and_failures_never_skip_browser_or_repeat_activation(self) -> None:
        for failure in ("none", "prepare", "unknown-prepare", "browser-rehearsal", "activate", "browser-confirmed", "commit", "foreign-guard", "recover"):
            with self.subTest(failure=failure):
                events: list[str] = []
                records: list[dict[str, object]] = []
                floor = 5
                def app(action: str) -> dict[str, object]:
                    nonlocal floor
                    events.append(action)
                    if failure == "unknown-prepare" and action == "prepare":
                        raise TimeoutError("synthetic unknown prepare")
                    if action == "activate":
                        floor = 6
                    if failure == "recover" and action == "recover":
                        raise TimeoutError("synthetic recovery failure")
                    return response(action, floor, passed=action != failure,
                                    guard=GUARD.replace("a" * 24, "b" * 24) if failure == "foreign-guard" and action == "activate" else GUARD)
                def browser(phase: str, guard: str) -> None:
                    self.assertEqual(guard, GUARD)
                    events.append("browser-" + phase)
                    if failure == "browser-" + phase or (failure == "recover" and phase == "rehearsal"):
                        raise ValueError("synthetic browser rejection")
                def record(value: dict[str, object]) -> None:
                    records.append(copy.deepcopy(value))
                value = flow.run_flow(app, browser, record)
                if failure == "none":
                    self.assertEqual(events, ["prepare", "browser-rehearsal", "activate", "browser-confirmed", "commit", "cleanup"])
                    self.assertEqual(value["result"], "passed")
                    self.assertIs(value["targetConfirmed"], True)
                    self.assertIs(value["guardMayRemain"], False)
                else:
                    self.assertEqual(value["result"], "failed")
                    self.assertLessEqual(events.count("prepare"), 1)
                    self.assertLessEqual(events.count("activate"), 1)
                    if failure == "unknown-prepare":
                        self.assertEqual(events, ["prepare"])
                        self.assertIs(value["independentInspectionRequired"], True)
                    elif failure == "recover":
                        self.assertNotIn("cleanup", events)
                        self.assertIs(value["guardMayRemain"], True)
                    else:
                        self.assertEqual(events[-2:], ["recover", "cleanup"])
                        self.assertIs(value["guardMayRemain"], False)
                        self.assertEqual(value["observedFloor"], floor)
                self.assertTrue(records)

    def test_receipt_failure_does_not_prevent_known_guard_recovery(self) -> None:
        events: list[str] = []
        def app(action: str) -> dict[str, object]:
            events.append(action)
            return response(action, 5)
        def record(value: dict[str, object]) -> None:
            if value.get("phase") == "browser-rehearsal" or value.get("phase") == "recover":
                raise OSError("synthetic receipt storage failure")
        result = flow.run_flow(app, lambda _phase, _guard: None, record)
        self.assertEqual(events, ["prepare", "recover", "cleanup"])
        self.assertIs(result["guardMayRemain"], False)
        self.assertIs(result["receiptWriteFailed"], True)

    def test_recovery_and_final_receipt_failure_return_bounded_failed_result(self) -> None:
        events: list[str] = []
        def app(action: str) -> dict[str, object]:
            events.append(action)
            return response(action, 5)
        def record(value: dict[str, object]) -> None:
            if "guardRoot" in value:
                raise OSError("synthetic full output filesystem")
        result = flow.run_flow(app, lambda _phase, _guard: None, record)
        self.assertEqual(events, ["prepare", "recover", "cleanup"])
        self.assertEqual(result["result"], "failed")
        self.assertIs(result["guardMayRemain"], False)
        self.assertIs(result["receiptWriteFailed"], True)

    def test_recovery_cannot_lower_an_observed_confirmed_floor(self) -> None:
        events: list[str] = []
        def app(action: str) -> dict[str, object]:
            events.append(action)
            return response(action, 6 if action == "activate" else 5)
        def browser(phase: str, _guard: str) -> None:
            if phase == "confirmed":
                raise TimeoutError("synthetic confirmed browser timeout")
        result = flow.run_flow(app, browser, lambda _value: None)
        self.assertEqual(events, ["prepare", "activate", "recover"])
        self.assertEqual(result["observedFloor"], 6)
        self.assertIs(result["targetConfirmed"], True)
        self.assertIs(result["guardMayRemain"], True)

    def test_envelope_rejects_foreign_profile_bundle_and_action(self) -> None:
        for key in ("profile", "bundleSha256", "action", "guardRoot"):
            value = response("prepare", 5)
            value[key] = "foreign"
            with self.assertRaises(ValueError):
                flow.envelope(value, "prepare", None)

    def test_failed_browser_cannot_be_used_to_commit(self) -> None:
        source = Path("tools/run_application_update_flow.py").read_text()
        self.assertIn("driver.browser_gate(str(target), guard, phase)", source)
        self.assertIn('argv += ["--browser-receipt", str(browser_receipts[phase])]', source)
        self.assertIn('"--profile", flow.PROFILE', source)
        self.assertIn('input=pair[role]', source)
        self.assertIn('stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL', source)


if __name__ == "__main__":
    unittest.main()
