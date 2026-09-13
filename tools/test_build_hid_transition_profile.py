from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/build-hid-transition-profile.sh"
BUILD_AARCH64 = ROOT / "scripts/build-aarch64.sh"


class HidTransitionProfileBuildTests(unittest.TestCase):
    def test_profile_bundle_builds_only_the_feature_broker(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn("BLI_KVM_CARGO_FEATURES=hid-transition-profile", script)
        self.assertIn('bundle_name=hardened-blikvm-hid-transition-profile-aarch64', script)
        self.assertIn("scripts/build-aarch64.sh", script)
        self.assertIn('install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"', script)
        self.assertIn("packaging/systemd/hardened-blikvm-hid-transition.service", script)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-hid.conf", script)
        self.assertIn("packaging/sysusers.d/hardened-blikvm.conf", script)
        self.assertIn('profile: "hid-transition-test"', script)
        self.assertIn("releaseAccepted: false", script)
        self.assertIn("deviceAccepted: false", script)

    def test_profile_bundle_refuses_relative_paths_without_secrets(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn("build paths must be absolute", script)
        for forbidden in ("not-a-real", "password=", "10.0.", "/home/w0w"):
            self.assertNotIn(forbidden, script)

    def test_aarch64_builder_accepts_a_feature_opt_in(self) -> None:
        build = BUILD_AARCH64.read_text()
        self.assertIn("BLI_KVM_CARGO_FEATURES", build)


if __name__ == "__main__":
    unittest.main()
