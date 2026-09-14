from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"
API_DIR = ROOT / "web/src/api"
VISIBLE_MODULES = ("capabilities.ts", "inventory.ts", "diagnostics.ts", "auth.ts")
ENDPOINT = re.compile(r"""['"`](/api/[a-zA-Z0-9/_-]+)['"`]""")


def server_routes() -> set[str]:
    text = SERVER.read_text(encoding="utf-8")
    return set(re.findall(r'\.route\(\s*"([^"]+)"', text))


def module_endpoints(name: str) -> set[str]:
    text = (API_DIR / name).read_text(encoding="utf-8")
    return {match.removeprefix("/api") for match in ENDPOINT.findall(text)}


class HardenedSettingsCoherenceTests(unittest.TestCase):
    def test_always_visible_modules_only_call_implemented_routes(self) -> None:
        routes = server_routes()
        for module in VISIBLE_MODULES:
            for endpoint in module_endpoints(module):
                self.assertIn(
                    endpoint,
                    routes,
                    f"{module} calls unimplemented route {endpoint}",
                )

    def test_settings_core_routes_are_implemented(self) -> None:
        routes = server_routes()
        for route in (
            "/capabilities",
            "/health",
            "/inventory",
            "/hostname",
            "/diagnostics",
            "/auth/setup",
            "/auth/login",
            "/auth/logout",
            "/auth/account",
            "/auth/password",
            "/application/current-version",
        ):
            self.assertIn(route, routes, f"settings route {route} is missing")

    def test_hardened_hostname_has_no_legacy_setter(self) -> None:
        vm = (API_DIR / "vm.ts").read_text(encoding="utf-8")
        self.assertIn("'/api/hostname'", vm)
        self.assertNotIn("'/api/vm/hostname'", vm)

    def test_read_only_settings_modules_use_implemented_get_routes(self) -> None:
        routes = server_routes()
        for module, endpoints in {
            "system-time.ts": ("/system/time",),
            "system-log.ts": ("/system-log/config", "/system-log/messages"),
        }.items():
            called = module_endpoints(module)
            for endpoint in endpoints:
                self.assertIn(endpoint, routes, f"{module} reads unimplemented {endpoint}")
                self.assertIn(endpoint, called, f"{module} no longer reads {endpoint}")

    def test_system_tab_is_capability_gated_and_read_only(self) -> None:
        capabilities = (ROOT / "web/src/lib/capabilities.ts").read_text(encoding="utf-8")
        self.assertIn("supportsSystemSection", capabilities)
        self.assertIn("systemSectionWritable", capabilities)
        self.assertIn("system_settings", capabilities)

        page = (
            ROOT / "web/src/pages/desktop/menu/settings/system/index.tsx"
        ).read_text(encoding="utf-8")
        self.assertIn("supportsSystemSection", page)
        self.assertIn("readOnly={!sectionWritable}", page)

        time_page = (
            ROOT / "web/src/pages/desktop/menu/settings/system/time.tsx"
        ).read_text(encoding="utf-8")
        self.assertIn("disabled={readOnly}", time_page)
        self.assertIn("!readOnly &&", time_page)

        log_page = (
            ROOT / "web/src/pages/desktop/menu/settings/system-log/index.tsx"
        ).read_text(encoding="utf-8")
        self.assertIn("disabled={readOnly}", log_page)
        self.assertIn("!readOnly &&", log_page)


if __name__ == "__main__":
    unittest.main()
