from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
API = ROOT / "web/src/api/restricted-control.ts"
LEASE = ROOT / "web/src/lib/hid-lease.ts"
CAPABILITIES = ROOT / "web/src/lib/capabilities.ts"


class RestrictedControlGuiTests(unittest.TestCase):
    def test_api_client_uses_only_the_restricted_endpoints(self) -> None:
        api = API.read_text()
        for path in (
            "/api/hid/focus",
            "/api/hid/lease",
            "/api/application-power",
        ):
            self.assertIn(path, api)
        for action in ("'acquire'", "'renew'", "'report'", "'release'"):
            self.assertIn(action, api)
        self.assertNotIn("legacy", api)
        self.assertNotIn("/dev/", api)

    def test_lease_session_renews_before_ttl_and_releases_on_stop(self) -> None:
        lease = LEASE.read_text()
        self.assertIn("RENEW_FRACTION = 0.5", lease)
        self.assertIn("scheduleRenew(response.data.ttlSeconds ?? ttlSeconds)", lease)
        self.assertIn("clearTimeout(this.renewTimer)", lease)
        self.assertIn("api.releaseLease(newOperationId(), leaseId)", lease)
        self.assertRegex(lease, r"Math\.max\(1, Math\.floor\(ttlSeconds \* 1000 \* RENEW_FRACTION\)\)")
        operation_id = re.search(r"OPERATION_ID_BYTES = (\d+)", lease)
        self.assertEqual(operation_id.group(1) if operation_id else "", "16")
        self.assertIn("if (!response.data.applied)", lease)
        self.assertIn("request.abort()", lease)
        self.assertIn("requireGeneration", lease)

    def test_controls_are_capability_gated(self) -> None:
        capabilities = CAPABILITIES.read_text()
        self.assertIn("supportsRestrictedControls", capabilities)
        self.assertIn("isWritable(manifest.hid.status)", capabilities)
        self.assertIn("isWritable(manifest.power.status)", capabilities)

    def test_no_secrets_in_the_client(self) -> None:
        for path in (API, LEASE):
            text = path.read_text()
            for forbidden in ("password", "10.0.", "/home/", "token"):
                self.assertNotIn(forbidden, text.lower())

    def test_power_components_use_the_restricted_path_when_advertised(self) -> None:
        power = ROOT / "web/src/pages/desktop/menu/power"
        expectations = {
            "power-short.tsx": "'power_short'",
            "power-long.tsx": "'power_long'",
            "reset.tsx": "'reset'",
        }
        for name, action in expectations.items():
            text = (power / name).read_text()
            self.assertIn("useRestrictedPower", text)
            self.assertIn(action, text)
            self.assertIn("setGpio", text)
        hook = (ROOT / "web/src/hooks/useRestrictedPower.ts").read_text()
        self.assertIn("supportsPowerControl", hook)
        self.assertIn("performPowerAction", hook)
        self.assertNotIn("useFocusConfirmation", hook)
        for name in expectations:
            text = (power / name).read_text()
            self.assertIn("showConfirm || restricted", text)
        helper = (ROOT / "web/src/lib/restricted-power.ts").read_text()
        self.assertIn("crypto.subtle.digest('SHA-256'", helper)
        self.assertIn("ensureFocus", helper)
        self.assertIn("submitPowerAction", helper)

    def test_keyboard_uses_the_lease_path_when_advertised(self) -> None:
        keyboard = (ROOT / "web/src/pages/desktop/keyboard/index.tsx").read_text()
        self.assertIn("useHidLease", keyboard)
        self.assertIn("mode: 'keyboard'", keyboard)
        self.assertIn("client.send", keyboard)
        hook = (ROOT / "web/src/hooks/useHidLease.ts").read_text()
        self.assertIn("HidLeaseSession", hook)
        self.assertIn("ensureFocus", hook)
        self.assertIn("sharedQueue", hook)
        self.assertIn("supportsRestrictedControls", hook)
        self.assertIn("session.stop()", hook)

    def test_all_input_surfaces_share_one_broker_lease(self) -> None:
        hook = (ROOT / "web/src/hooks/useHidLease.ts").read_text()
        # The broker admits one active HID lease; a per-surface lease would make
        # the keyboard and the mouse mutually exclusive with lease_conflict.
        self.assertIn("let sharedSession: HidLeaseSession | null = null;", hook)
        self.assertIn("let sharedQueue: Promise<void> = Promise.resolve();", hook)
        # One shared session, and it refreshes the target attestation the
        # broker requires before every acquire and renewal.
        self.assertEqual(hook.count("new HidLeaseSession("), 1)
        self.assertIn("new HidLeaseSession(() => ensureFocus(platformId))", hook)
        self.assertIn("let consumers = 0;", hook)
        self.assertIn("consumers === 0", hook)

    def test_the_lease_session_refreshes_the_attestation(self) -> None:
        lease = (ROOT / "web/src/lib/hid-lease.ts").read_text()
        # The broker admits a lease only while the target attestation is live,
        # and it expires long before an idle session would end.
        self.assertEqual(lease.count("await this.attest();"), 2)
        self.assertIn("constructor(private readonly attest: () => Promise<void>) {}", lease)
        focus = (ROOT / "web/src/lib/restricted-power.ts").read_text()
        self.assertIn("FOCUS_TTL_MS = 15_000", focus)

    def test_mouse_coalescing_preserves_relative_wheel_and_button_semantics(self) -> None:
        hook = (ROOT / "web/src/hooks/useHidLease.ts").read_text()
        queue = (ROOT / "web/src/lib/hid-report-queue.ts").read_text()
        # Absolute positions may collapse to the latest point. Relative deltas
        # are summed, while wheel and button transitions flush in order.
        self.assertIn("let pendingMouse: MouseReport | null = null;", hook)
        self.assertIn("function scheduleMouseFlush()", hook)
        self.assertIn("MOUSE_FLUSH_MS", hook)
        self.assertIn("if (report.mode === 'keyboard') {", hook)
        self.assertIn("signedByte(current.data[1]) + signedByte(next.data[1])", queue)
        self.assertIn("current.data[0] !== next.data[0]", queue)
        self.assertIn("hasWheel(report)", hook)

    def test_stop_cancels_stale_queued_and_inflight_reports(self) -> None:
        hook = (ROOT / "web/src/hooks/useHidLease.ts").read_text()
        self.assertIn("let queueGeneration = 0;", hook)
        self.assertIn("queueGeneration += 1;", hook)
        self.assertIn("generation !== queueGeneration", hook)
        self.assertIn("startingSession", hook)
        self.assertIn("!confirmedRef.current", hook)
        self.assertIn("window.addEventListener('blur', stopControl)", hook)
        self.assertIn("document.visibilityState === 'hidden'", hook)

    def test_mouse_uses_the_lease_path_when_advertised(self) -> None:
        mouse = ROOT / "web/src/pages/desktop/mouse"
        for name in ("absolute.tsx", "relative.tsx", "mobile-trackpad.tsx"):
            text = (mouse / name).read_text()
            self.assertIn("useHidLease", text)
            self.assertIn("client.send", text)
        self.assertIn("mode: 'absolute_mouse'", (mouse / "absolute.tsx").read_text())
        self.assertIn("mode: 'relative_mouse'", (mouse / "relative.tsx").read_text())
        self.assertIn("mode: 'relative_mouse'", (mouse / "mobile-trackpad.tsx").read_text())

    def test_mobile_and_virtual_keyboards_use_the_restricted_path(self) -> None:
        mobile = (
            ROOT / "web/src/pages/desktop/mobile-local-keyboard/index.tsx"
        ).read_text()
        self.assertIn("useHidLease", mobile)
        self.assertIn("reportHid", mobile)
        self.assertIn("textToKeyReports", mobile)
        self.assertIn("if (restricted) {", mobile)
        virtual = (ROOT / "web/src/pages/desktop/virtual-keyboard/index.tsx").read_text()
        self.assertIn("useHidLease", virtual)
        self.assertIn("reportHid", virtual)
        self.assertIn("data: [modifier, 0, code, 0, 0, 0, 0, 0]", virtual)
        helper = (ROOT / "web/src/lib/hid-text.ts").read_text()
        self.assertIn("export function textToKeyReports", helper)
        self.assertIn("getModifierBit('ShiftLeft')", helper)

    def test_power_led_reads_the_hardened_inventory(self) -> None:
        power = (ROOT / "web/src/pages/desktop/menu/power/index.tsx").read_text()
        self.assertIn("getRuntimeInventory", power)
        self.assertIn("inventory.data.powerLed", power)
        # The hardened platform has no legacy GPIO endpoint, so the fallback
        # must never be reached for it.
        self.assertIn("usesBliApplicationUpdates(manifest)", power)
        self.assertIn("if (hardened) {", power)

    def test_paste_and_shortcuts_follow_the_restricted_capability(self) -> None:
        paste = (ROOT / "web/src/pages/desktop/menu/keyboard/paste.tsx").read_text()
        self.assertIn("useHidLease", paste)
        self.assertIn("textToKeyReports", paste)
        self.assertIn("if (restricted) {", paste)
        mouse = (ROOT / "web/src/pages/desktop/menu/mouse/index.tsx").read_text()
        # HID-only mode and the legacy HID reset call endpoints the hardened
        # server does not expose, so they must be hidden on this surface.
        self.assertIn("{!restricted && <HidMode />}", mouse)
        self.assertIn("{!restricted && <ResetHid />}", mouse)
        self.assertIn("supportsRestrictedControls(manifest)", mouse)
        menu = (ROOT / "web/src/pages/desktop/menu/keyboard/index.tsx").read_text()
        # Shortcuts and the leader key rely on legacy endpoints that the
        # hardened server does not expose, so they must be hidden.
        self.assertIn("{!restricted && <Shortcuts />}", menu)
        self.assertIn("{!restricted && <LeaderKey />}", menu)

    def test_explicit_operator_confirmation_is_required(self) -> None:
        atom = (ROOT / "web/src/jotai/restricted-control.ts").read_text()
        self.assertIn("focusConfirmedAtom = atom(false)", atom)
        confirmation = (ROOT / "web/src/hooks/useFocusConfirmation.ts").read_text()
        self.assertIn("Modal.confirm(", confirmation)
        self.assertIn("onOk:", confirmation)
        self.assertIn("setConfirmed(true)", confirmation)
        self.assertIn("pendingConfirmation", confirmation)
        self.assertIn("confirmedGlobal", confirmation)
        self.assertIn("requestConfirmation", confirmation)
        self.assertIn("const ensureConfirmed = useCallback(async () => confirmedGlobal, [])", confirmation)
        hid_hook = (ROOT / "web/src/hooks/useHidLease.ts").read_text()
        self.assertIn("useFocusConfirmation", hid_hook)
        self.assertIn("ensureConfirmed", hid_hook)
        control = (ROOT / "web/src/pages/desktop/menu/remote-control.tsx").read_text()
        self.assertIn("Enable remote control", control)
        self.assertIn("disableControl", control)


if __name__ == "__main__":
    unittest.main()
