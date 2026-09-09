"""Bound orchestration policy around the existing application-update driver."""

from collections.abc import Callable
from typing import cast

from tools import installed_application_update as native
from tools import stage_installed_application_update as driver

PROFILE = "seq5-to-seq6-mjpeg"
PROFILES = (PROFILE, "seq6-to-seq7-write-optin")
AppStep = Callable[[str], dict[str, object]]
BrowserStep = Callable[[str, str], None]
Recorder = Callable[[dict[str, object]], None]


def envelope(value: dict[str, object], action: str, guard: str | None, *, profile_name: str = PROFILE) -> str:
    native.require(profile_name in PROFILES, "unreviewed application flow profile")
    profile = native.PROFILES[profile_name]
    native.require(value.get("probe") == "installed-application-update" and value.get("action") == action
                   and value.get("profile") == profile_name and value.get("bundleSha256") == profile.bundle_sha,
                   "application step binding mismatch")
    found = driver.validate_guard(value.get("guardRoot"))
    native.require(guard is None or found == guard, "application guard changed")
    return found


def accepted(value: dict[str, object], action: str, floor: int | None, *, profile_name: str = PROFILE) -> int:
    native.require(profile_name in PROFILES, "unreviewed application flow profile")
    profile = native.PROFILES[profile_name]
    native.require(value.get("result") == "passed" and value.get("controllerStagingRemoved") is True,
                   "application step failed or staging remains")
    device = cast(dict[str, object], value["device"])
    native.require(device.get("result") == {"prepare": "rehearsed", "recover": "recovered"}.get(action, action),
                   "unexpected device action")
    if action in ("activate", "commit", "cleanup"):
        allowed = {"activate": {"confirmed"}, "commit": {"committed-update"},
                   "cleanup": {"committed-update", "recovered"}}[action]
        native.require(device.get("phase") in allowed, "unexpected device phase")
    verification = cast(dict[str, object], device["verification"])
    native.require(verification.get("accountTlsTrustPeersPreserved") is True, "preservation not verified")
    status = cast(dict[str, object], verification["status"])
    actual = status.get("floorSequence")
    slots = {profile.source_sequence: profile.source_slot, profile.target_sequence: profile.target_slot}
    native.require(type(actual) is int and actual in slots and (floor is None or actual == floor)
                   and status.get("pending") is None
                   and status.get("activeSlotId") == status.get("confirmedSlotId") == slots[actual],
                   "application state did not converge")
    if action == "prepare":
        native.require(status.get("lastRollback") == "health_timeout", "rollback rehearsal not verified")
    return cast(int, actual)


def run_flow(app: AppStep, browser: BrowserStep, record: Recorder, *, profile_name: str = PROFILE) -> dict[str, object]:
    native.require(profile_name in PROFILES, "unreviewed application flow profile")
    profile = native.PROFILES[profile_name]
    result: dict[str, object] = {"probe": "application-update-flow", "profile": profile_name,
                                "result": "failed", "guardMayRemain": True, "targetConfirmed": False}
    guard: str | None = None
    observed_floor = profile.source_sequence
    recovering = False

    def checkpoint() -> None:
        try:
            record(result)
        except Exception:
            if not recovering:
                raise
            result["receiptWriteFailed"] = True

    def step(action: str, floor: int | None) -> None:
        nonlocal guard, observed_floor
        result["phase"] = action
        checkpoint()
        value = app(action)
        guard = envelope(value, action, guard, profile_name=profile_name)
        result["guardRoot"] = guard
        result[action] = value
        checkpoint()
        actual = accepted(value, action, floor, profile_name=profile_name)
        native.require(actual >= observed_floor, "application floor regressed")
        observed_floor = actual
        result["observedFloor"] = actual
        result["targetConfirmed"] = actual == profile.target_sequence

    try:
        step("prepare", profile.source_sequence)
        assert guard is not None
        result["phase"] = "browser-rehearsal"
        checkpoint()
        browser("rehearsal", guard)
        result["rehearsalBrowserPassed"] = True
        checkpoint()
        step("activate", profile.target_sequence)
        result["phase"] = "browser-confirmed"
        checkpoint()
        browser("confirmed", guard)
        result["confirmedBrowserPassed"] = True
        checkpoint()
        step("commit", profile.target_sequence)
        step("cleanup", profile.target_sequence)
        result.update({"result": "passed", "guardMayRemain": False})
    except Exception as error:
        recovering = True
        # Exceptions stop forward progress. No prepare/activate is retried.
        result["failedPhase"] = result["phase"]
        result["errorType"] = type(error).__name__
        if guard is not None:
            try:
                step("recover", None)
                step("cleanup", observed_floor)
                result["guardMayRemain"] = False
            except Exception as recovery_error:
                result["recoveryErrorType"] = type(recovery_error).__name__
        else:
            result["independentInspectionRequired"] = True
    checkpoint()
    return result
