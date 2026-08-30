import unittest

from tools import capture_pikvm_baseline as baseline


class CaptureBaselineTests(unittest.TestCase):
    def test_endpoint_rejects_embedded_credentials_and_http(self) -> None:
        for endpoint in (
            "http://device.invalid",
            "https://name:secret@device.invalid",
            "https://device.invalid/?token=value",
        ):
            with self.assertRaises(ValueError):
                _ = baseline.validate_endpoint(endpoint)

    def test_summary_keeps_versions_and_booleans_but_drops_identifiers(self) -> None:
        payload: dict[str, object] = {
            "ok": True,
            "result": {
                "system": {"kvmdVersion": "3.291", "enabled": True},
                "serial": "private-device-id",
                "username": "private-user",
                "token": "private-token",
                "network": {"address": "10.0.0.1"},
            },
        }
        summary = baseline.response_summary("/api/info", 200, payload)
        safe_facts = summary["safeFacts"]
        self.assertEqual(safe_facts["result.system.kvmdVersion"], "3.291")
        self.assertIs(safe_facts["result.system.enabled"], True)
        rendered = repr(summary)
        for forbidden in (
            "private-device-id",
            "private-user",
            "private-token",
            "10.0.0.1",
        ):
            self.assertNotIn(forbidden, rendered)

    def test_sensitive_version_named_key_is_still_removed(self) -> None:
        payload: dict[str, object] = {
            "result": {"tokenVersion": "must-not-appear"}
        }
        summary = baseline.response_summary("/api/info", 200, payload)
        self.assertNotIn("must-not-appear", repr(summary))


if __name__ == "__main__":
    _ = unittest.main()
