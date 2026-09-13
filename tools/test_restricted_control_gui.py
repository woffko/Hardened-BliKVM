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


if __name__ == "__main__":
    unittest.main()
