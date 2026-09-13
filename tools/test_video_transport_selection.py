from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
HELPER = ROOT / "web/src/lib/video-transport.ts"


class VideoTransportSelectionTests(unittest.TestCase):
    def test_selection_is_limited_to_advertised_transports(self) -> None:
        source = HELPER.read_text()
        self.assertIn("manifest.video.transports", source)
        self.assertIn("transports.includes(candidate)", source)
        self.assertIn("return null", source)

    def test_webrtc_is_never_a_default_candidate(self) -> None:
        source = HELPER.read_text()
        self.assertNotIn("['web_rtc_janus'", source)
        self.assertNotIn('["web_rtc_janus"', source)
        self.assertIn("isWebRtcAdvertised", source)
        self.assertIn("transport === 'web_rtc_janus'", source)
        self.assertIn("transport === 'web_rtc_native'", source)

    def test_helper_is_capability_gated(self) -> None:
        source = HELPER.read_text()
        self.assertIn("CapabilityManifest", source)
        self.assertIn("if (!manifest)", source)


if __name__ == "__main__":
    unittest.main()
