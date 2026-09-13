from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
SCRIPT = ROOT / "scripts/build-restricted-control-profile.sh"
BUILD_AARCH64 = ROOT / "scripts/build-aarch64.sh"
SERVICE = ROOT / "packaging/systemd/hardened-blikvm-control-transition.service"
TMPFILES = ROOT / "packaging/tmpfiles.d/hardened-blikvm-control-transition.conf"
DROPIN = (
    ROOT
    / "packaging/dropins/hardened-blikvm-server-current.service.d/90-restricted-control.conf"
)


class RestrictedControlProfileBuildTests(unittest.TestCase):
    def test_profile_bundle_builds_both_feature_binaries(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn(
            "cargo_features=restricted-control-surface,hid-transition-profile,atx-transition-profile",
            script,
        )
        self.assertIn(
            "bundle_name=hardened-blikvm-restricted-control-profile-aarch64", script
        )
        self.assertIn("scripts/build-aarch64.sh", script)
        self.assertIn("corepack pnpm --dir web build", script)
        self.assertIn('cp -a web/dist/. "$stage/share/web/"', script)
        self.assertIn('install -m 0755 "$server_binary" "$stage/libexec/bli-kvm-server"', script)
        self.assertIn(
            'install -m 0755 "$broker_binary" "$stage/libexec/bli-kvm-control-broker"', script
        )
        self.assertIn("packaging/systemd/hardened-blikvm-control-transition.service", script)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-control-transition.conf", script)
        self.assertIn(
            "packaging/dropins/hardened-blikvm-server-current.service.d/90-restricted-control.conf",
            script,
        )
        self.assertIn('profile: "restricted-control-test"', script)
        self.assertIn("releaseAccepted: false", script)
        self.assertIn("deviceAccepted: false", script)

    def test_profile_bundle_refuses_relative_paths_without_secrets(self) -> None:
        script = SCRIPT.read_text()
        self.assertIn("build paths must be absolute", script)
        for forbidden in ("not-a-real", "password=", "10.0.", "/home/w0w"):
            self.assertNotIn(forbidden, script)

    def test_combined_unit_attaches_both_profiles_and_credentials(self) -> None:
        unit = SERVICE.read_text()
        self.assertIn("BLI_KVM_HID_PROFILE=1", unit)
        self.assertIn("BLI_KVM_ATX_PROFILE=1", unit)
        self.assertIn("BLI_KVM_CONTROL_SOCKET=/run/hardened-blikvm-control-transition/control.sock", unit)
        self.assertIn("LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user", unit)
        self.assertIn("LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password", unit)
        self.assertIn("Before=hardened-blikvm-server.service hardened-blikvm-server-current.service", unit)
        for hardening in ("NoNewPrivileges=yes", "ProtectSystem=strict", "PrivateNetwork=yes"):
            self.assertIn(hardening, unit)

    def test_server_dropin_opts_into_the_restricted_surface(self) -> None:
        dropin = DROPIN.read_text()
        self.assertIn("BLI_KVM_RESTRICTED_CONTROLS=1", dropin)
        self.assertIn(
            "BLI_KVM_CONTROL_SOCKET=/run/hardened-blikvm-control-transition/control.sock",
            dropin,
        )
        self.assertIn("Wants=hardened-blikvm-control-transition.service", dropin)

    def test_tmpfiles_owns_the_transition_state_directory(self) -> None:
        tmpfiles = TMPFILES.read_text()
        self.assertIn("/var/lib/hardened-blikvm-control-transition", tmpfiles)
        self.assertIn("hardened-blikvm-hid", tmpfiles)

    def test_aarch64_builder_accepts_a_feature_opt_in(self) -> None:
        build = BUILD_AARCH64.read_text()
        self.assertIn("BLI_KVM_CARGO_FEATURES", build)


if __name__ == "__main__":
    unittest.main()
