from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/build-board-profile.sh"
BUILD_AARCH64 = ROOT / "scripts/build-aarch64.sh"


class BoardProfileBuildTests(unittest.TestCase):
    def test_board_bundle_pins_the_board_owner_features(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn(
            "cargo_features=restricted-control-surface,gadget-media-profile,"
            "gadget-hid-profile,chardev-atx-profile",
            script,
        )
        self.assertIn("BLI_KVM_CARGO_FEATURES=\"$cargo_features\"", script)
        self.assertIn("bundle_name=hardened-blikvm-board-profile-aarch64", script)
        self.assertIn("scripts/build-aarch64.sh", script)
        self.assertIn(
            'install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"',
            script,
        )
        self.assertIn(
            'install -m 0755 "$bench_gate_binary" '
            '"$stage/libexec/bli-kvm-atx-gpio-bench-gate"',
            script,
        )
        self.assertIn(
            'install -m 0755 "$video_proxy_binary" "$stage/libexec/bli-kvm-video-proxy"',
            script,
        )
        self.assertIn("packaging/systemd/hardened-blikvm-board-owner.service", script)
        self.assertIn(
            "packaging/systemd/hardened-blikvm-board-server.service", script
        )
        self.assertIn(
            "packaging/systemd/hardened-blikvm-board-video-proxy.service", script
        )
        self.assertIn("packaging/systemd/hardened-blikvm-ustreamer.service", script)
        self.assertIn("packaging/udev/99-hardened-blikvm-video.rules", script)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-gadget.conf", script)
        self.assertIn('profile: "board-owner-test"', script)
        self.assertIn("releaseAccepted: false", script)
        self.assertIn("deviceAccepted: false", script)

    def test_board_bundle_leaves_timings_to_the_operator(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn("atx.env.example", script)
        self.assertIn("BLI_KVM_ATX_POWER_SHORT_MS=", script)
        self.assertIn("BLI_KVM_ATX_POWER_LONG_MS=", script)
        self.assertIn("BLI_KVM_ATX_RESET_MS=", script)
        # No numeric timing values are baked in.
        for forbidden in (
            "BLI_KVM_ATX_POWER_SHORT_MS=1",
            "BLI_KVM_ATX_POWER_LONG_MS=1",
            "BLI_KVM_ATX_RESET_MS=1",
            "BLI_KVM_ATX_POWER_SHORT_MS=5",
            "BLI_KVM_ATX_POWER_LONG_MS=5",
            "BLI_KVM_ATX_RESET_MS=5",
        ):
            self.assertNotIn(forbidden, script)

    def test_board_bundle_refuses_relative_paths_without_secrets(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn("build paths must be absolute", script)
        for forbidden in ("not-a-real", "password=", "10.0.", "/home/w0w"):
            self.assertNotIn(forbidden, script)

    def test_aarch64_builder_accepts_a_feature_opt_in(self) -> None:
        build = BUILD_AARCH64.read_text()
        self.assertIn("BLI_KVM_CARGO_FEATURES", build)


if __name__ == "__main__":
    unittest.main()
