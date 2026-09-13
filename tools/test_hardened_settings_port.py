from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
PLAN = ROOT / "docs/design/hardened-settings-port-plan.md"
VM_API = ROOT / "web/src/api/vm.ts"
HOSTNAME = ROOT / "web/src/pages/desktop/menu/settings/about/hostname.tsx"
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"


class HardenedSettingsPortTests(unittest.TestCase):
    def test_plan_records_the_coherence_rules(self) -> None:
        plan = PLAN.read_text(encoding="utf-8")
        for required in (
            "No visible control may call an endpoint the hardened server does not expose",
            "New server endpoints are read-only",
            "Slice 0 detail",
        ):
            self.assertIn(required, plan)

    def test_hostname_uses_the_read_only_hardened_route(self) -> None:
        api = VM_API.read_text(encoding="utf-8")
        self.assertNotIn("'/api/vm/hostname'", api)
        self.assertIn("'/api/hostname'", api)
        component = HOSTNAME.read_text(encoding="utf-8")
        self.assertIn("readOnly", component)
        self.assertNotIn("api.setHostname", component)
        self.assertNotIn("api\n      .setHostname", component)

    def test_server_exposes_a_bounded_read_only_hostname(self) -> None:
        server = SERVER.read_text(encoding="utf-8")
        self.assertIn('.route("/hostname", get(hostname))', server)
        self.assertIn("fn validate_hostname", server)
        self.assertIn("/proc/sys/kernel/hostname", server)


if __name__ == "__main__":
    unittest.main()
