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
        hid = (ROOT / "crates/bli-kvm-server/src/hid.rs").read_text()
        self.assertIn("TargetAttest", hid)
        self.assertNotIn("HidLeaseRequest", hid)
        self.assertNotIn("/dev/", hid)
        self.assertNotIn("Command", hid)

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
        transition = (ROOT / "crates/bli-kvm-atx-transition/src/lib.rs").read_text()
        self.assertIn("KvmdAtxClient", transition)
        for forbidden in ("/dev/", "gpio", "libc::", "Command"):
            self.assertNotIn(forbidden, transition)


if __name__ == "__main__":
    unittest.main()
