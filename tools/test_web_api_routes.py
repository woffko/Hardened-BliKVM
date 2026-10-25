"""Every web API call must reach a server route, or be named as gated.

`web/src/api` keeps reference functions whose controls the capability gates
hide on this board. A gate change could otherwise expose a control whose
request the server does not route. This test lists each call's method and
path, matches it against the routes in `crates/bli-kvm-server/src/lib.rs`, and
requires every unrouted call to be listed below with the gate that hides it;
a listed call that has since gained a route must leave the list.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"
API = ROOT / "web/src/api"

# (file, function): the gate that keeps its control away on this board.
GATED = {
    ("application.ts", "checkSystemUpdate"): "update page: non-BliKVM application updates only",
    ("application.ts", "downloadSystemUpdate"): "update page: non-BliKVM application updates only",
    ("application.ts", "getSystemVersion"): "not called",
    ("application.ts", "setPreviewUpdates"): "update page: non-BliKVM preview channel only",
    ("application.ts", "offlineUpdate"): "update page: non-BliKVM offline packages only",
    ("application.ts", "update"): "update page: non-BliKVM online update only",
    ("autostart.ts", "uploadAutostart"): "advanced autostart editor is not rendered",
    ("autostart.ts", "deleteAutostart"): "advanced autostart editor is not rendered",
    ("hid.ts", "getHidMode"): "mouse menu and virtual devices: legacy HID only",
    ("hid.ts", "setHidMode"): "mouse menu: legacy HID only",
    ("hid.ts", "getUsbWakeup"): "virtual devices: capability unavailable",
    ("hid.ts", "setUsbWakeup"): "virtual devices: capability unavailable",
    ("hid.ts", "paste"): "paste: the restricted HID lease types instead",
    ("hid.ts", "reset"): "HID reset: legacy HID only",
    ("network.ts", "verifyApLogin"): "Wi-Fi access-point page: the AP mode is refused",
    ("system-log.ts", "setConfig"): "system log: read-only capability",
    ("system-time.ts", "setConfig"): "time: read-only capability",
    ("system-time.ts", "syncNow"): "time: read-only capability",
    ("virtual-device.ts", "getVirtualDevice"): "virtual devices: capability unavailable",
    ("virtual-device.ts", "updateVirtualDevice"): "virtual devices: capability unavailable",
    ("vm.ts", "getGpio"): "power menu: legacy GPIO only",
    ("vm.ts", "setGpio"): "power menu: legacy GPIO only",
    ("vm.ts", "getHdmiState"): "HDMI settings: capability unavailable",
    ("vm.ts", "enableHdmi"): "not called",
    ("vm.ts", "disableHdmi"): "not called",
    ("vm.ts", "resetHdmi"): "screen menu: the reset entry is not rendered",
    ("vm.ts", "getSwap"): "swap settings: capability unavailable",
    ("vm.ts", "setSwap"): "swap settings: capability unavailable",
    ("vm.ts", "getMemoryLimit"): "memory limit: capability unavailable",
    ("vm.ts", "setMemoryLimit"): "memory limit: capability unavailable",
}


def server_routes() -> dict[str, set[str]]:
    text = SERVER.read_text(encoding="utf-8")
    routes: dict[str, set[str]] = {}
    for match in re.finditer(r'\.route\(\s*"([^"]+)"\s*,((?:[^()]|\((?:[^()]|\([^()]*\))*\))*)\)', text):
        methods = set(re.findall(r"\b(get|post|delete|put|patch)\(", match.group(2)))
        routes.setdefault(match.group(1), set()).update(methods)
    return routes


def pattern(route: str) -> re.Pattern[str]:
    return re.compile("^" + re.sub(r"\\\{[^}]+\\\}", "[^/]+", re.escape(route)) + "$")


def web_calls() -> list[tuple[str, str, str, str]]:
    calls = []
    for path in sorted(API.glob("*.ts")):
        text = path.read_text(encoding="utf-8")
        for function in re.finditer(r"export (?:async )?function (\w+)\s*(?:<[^>]*>)?\s*\(", text):
            start = function.end()
            depth, index = 0, text.index("{", start)
            end = index
            for end in range(index, len(text)):
                if text[end] == "{":
                    depth += 1
                elif text[end] == "}":
                    depth -= 1
                    if depth == 0:
                        break
            body = text[index:end]
            # Every API path in the body: http.get/post/..., http.request({url}),
            # and fetch with a base URL in front.
            request_method = re.search(r"method:\s*['\"](\w+)['\"]", body)
            found = []
            for literal in re.finditer(r"[`'\"](?:\$\{[^}]+\})?(/api/[^`'\"]*)", body):
                helper = re.search(r"http\.(get|post|delete|put)\s*(?:<[^>]*>)?\(\s*$", body[: literal.start()])
                if helper:
                    method = helper.group(1)
                else:
                    method = (request_method.group(1) if request_method else "get").lower()
                found.append((method, literal.group(1)))
            for method, url in found:
                route = url.split("?")[0][len("/api") :]
                route = re.sub(r"\$\{[^}]+\}", "X", route)
                if route.endswith("/"):
                    route += "X"
                calls.append((path.name, function.group(1), method, route))
    return calls


def routed(routes: dict[str, set[str]], method: str, route: str) -> bool:
    return any(pattern(known).match(route) and method in methods for known, methods in routes.items())


class WebApiRouteTests(unittest.TestCase):
    def test_every_web_call_is_routed_or_gated(self) -> None:
        routes = server_routes()
        self.assertGreater(len(routes), 100)
        unrouted = {(file, function) for file, function, method, route in web_calls() if not routed(routes, method, route)}
        self.assertEqual(
            sorted(unrouted - set(GATED)),
            [],
            "web calls without a server route and without a named gate",
        )
        self.assertEqual(
            sorted(set(GATED) - unrouted),
            [],
            "gated calls that are routed now: drop them from the list",
        )

    def test_query_parameters_are_passed_as_they_are(self) -> None:
        # `http.get(url, params)` hands its second argument to axios as the
        # query; a `{ params }` there arrives as `params[...]`, which the
        # server ignores (code audit of 2026-10-20, AUD-15).
        wrapped = []
        for path in sorted((ROOT / "web/src").rglob("*.ts*")):
            text = path.read_text(encoding="utf-8")
            for match in re.finditer(r"http\.get\(\s*[^,()]+,\s*\{\s*params\b", text):
                line = text.count("\n", 0, match.start()) + 1
                wrapped.append(f"{path.relative_to(ROOT)}:{line}")
        self.assertEqual(wrapped, [], "query parameters wrapped in a params object")


if __name__ == "__main__":
    unittest.main()
