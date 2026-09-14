from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
PLATFORM = ROOT / "crates/bli-kvm-platform-bli/src/lib.rs"
BROKER_V2 = ROOT / "crates/bli-kvm-control-broker/src/v2.rs"
BROKER_SERVER = ROOT / "crates/bli-kvm-control-broker/src/server.rs"
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"
CONTROL_V2_DOC = ROOT / "docs/api/control-v2.md"


class Phase3ControlBoundaryTests(unittest.TestCase):
    def test_hid_and_power_capabilities_stay_fail_closed_on_h616(self) -> None:
        platform = PLATFORM.read_text()
        self.assertIn("HidCapabilities {", platform)
        self.assertIn("status: FeatureStatus::not_exposed(runtime_verified)", platform)
        for mode in ("HidMode::Keyboard", "HidMode::AbsoluteMouse", "HidMode::RelativeMouse"):
            self.assertIn(mode, platform)
        self.assertIn("led_state: FeatureStatus::read_only(runtime_verified)", platform)
        self.assertIn("FeatureStatus::not_exposed(EvidenceLevel::SourceVerified)", platform)

    def test_broker_v2_default_is_disabled_and_has_no_production_enabled_path(self) -> None:
        v2 = BROKER_V2.read_text()
        server = BROKER_SERVER.read_text()
        self.assertIn("pub fn disabled()", v2)
        self.assertIn("#[cfg(test)]", v2)
        self.assertIn("fn enabled_for_test()", v2)
        self.assertNotIn("pub fn enabled(", v2)
        self.assertIn("ControlV2State::disabled()", server)
        self.assertNotIn("enabled_for_test", server)
        self.assertIn("CapabilityDisabled", v2)

    def test_focus_endpoint_is_opt_in_and_attestation_only(self) -> None:
        server = SERVER.read_text()
        self.assertIn("pub control_socket: Option<PathBuf>", server)
        self.assertIn("control_socket: None,", server)
        self.assertIn('"/hid/focus"', server)
        self.assertIn('"/hid/lease"', server)
        self.assertIn('"/application-power"', server)
        hid = (ROOT / "crates/bli-kvm-server/src/hid.rs").read_text()
        self.assertIn("TargetAttest", hid)
        self.assertIn("a current focus confirmation is required", hid)
        self.assertNotIn("/dev/", hid)
        self.assertNotIn("Command", hid)
        atx = (ROOT / "crates/bli-kvm-server/src/atx.rs").read_text()
        self.assertIn("PlatformControlRequestV2::Atx", atx)
        self.assertIn("a current focus confirmation is required", atx)
        for forbidden in ("/dev/", "gpio", "Command"):
            self.assertNotIn(forbidden, atx)

    def test_control_v2_doc_keeps_mutation_disabled(self) -> None:
        doc = CONTROL_V2_DOC.read_text()
        self.assertIn("compile-time-disabled", doc)
        self.assertIn("capability_disabled", doc)
        self.assertIn("the capability stays unavailable", doc)

    def test_atx_uses_only_fixed_kvmd_waiting_routes(self) -> None:
        client = (ROOT / "crates/bli-kvm-kvmd-atx-client/src/lib.rs").read_text()
        routes = set(re.findall(r"/atx/click\?button=([a-z_]+)&wait=1", client))
        self.assertEqual(routes, {"power", "power_long", "reset"})
        for forbidden in ("/dev/", "gpio", "libc::", "std::process", "Command"):
            self.assertNotIn(forbidden, client)

        # The default profile still pulses through the KVMD fixed routes. A
        # native GPIO executor exists for the reviewed board handoff, but it is
        # compile-time gated, off by default and holds no device path here.
        transition = (ROOT / "crates/bli-kvm-atx-transition/src/lib.rs").read_text()
        self.assertIn("KvmdAtxClient", transition)
        self.assertIn('#[cfg(feature = "gpio-atx-profile")]', transition)
        self.assertIn("AtxPulseRunner", transition)
        for forbidden in ("/dev/", "libc::", "Command"):
            self.assertNotIn(forbidden, transition)

        cargo = (ROOT / "crates/bli-kvm-atx-transition/Cargo.toml").read_text()
        default_line = next(
            line for line in cargo.splitlines() if line.startswith("default = ")
        )
        self.assertNotIn("gpio-atx-profile", default_line)

        executor = (ROOT / "crates/bli-kvm-atx-gpio-executor/src/lib.rs").read_text()
        for forbidden in ("/dev/", "libc::", "Command", "std::process"):
            self.assertNotIn(forbidden, executor)


if __name__ == "__main__":
    unittest.main()
