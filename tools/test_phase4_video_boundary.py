from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
PLATFORM = ROOT / "crates/bli-kvm-platform-bli/src/lib.rs"
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"
BROKER_CLIENT = ROOT / "crates/bli-kvm-broker-client/src/lib.rs"
BROKER_PROTOCOL = ROOT / "crates/bli-kvm-broker-protocol/src/lib.rs"


class Phase4VideoBoundaryTests(unittest.TestCase):
    def test_only_loopback_mjpeg_is_advertised(self) -> None:
        platform = PLATFORM.read_text()
        production = platform.split("#[cfg(test)]", 1)[0]
        self.assertIn("transports: vec![VideoTransport::Mjpeg]", production)
        self.assertIn("preferred_transport: Some(VideoTransport::Mjpeg)", production)
        self.assertIn("audio: FeatureStatus::unavailable()", production)
        self.assertNotIn("VideoTransport::WebRtcJanus", production)

    def test_video_read_is_a_loopback_unix_socket(self) -> None:
        server = SERVER.read_text()
        self.assertIn(
            'video_socket: PathBuf::from("/run/hardened-blikvm-video/mjpeg.sock")', server
        )
        self.assertIn("VideoReadClient::new(&options.video_socket)", server)
        for forbidden in ("webrtc", "WebRTC", "janus_route", "RTCPeer"):
            self.assertNotIn(forbidden, server)

    def test_janus_inventory_remains_disabled(self) -> None:
        for source in (
            BROKER_CLIENT.read_text(),
            BROKER_PROTOCOL.read_text(),
        ):
            self.assertIn("janus_enabled: Some(false)", source)

    def test_web_has_no_webrtc_transport_enabled(self) -> None:
        package = (ROOT / "web/package.json").read_text()
        self.assertNotIn("janus", package.lower())
        self.assertNotIn("@livekit", package.lower())
        self.assertNotIn("simple-peer", package.lower())


if __name__ == "__main__":
    unittest.main()
