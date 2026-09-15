"""Guard the canonical plan against route drift.

The parity matrix once claimed a stale route count and marked implemented
read-only endpoints as hidden. This check pins the plan's stated route count to
the actual server routes and requires the read-only endpoints to be described.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
SERVER = ROOT / "crates/bli-kvm-server/src/lib.rs"
PLAN = ROOT / "docs/design/gui-parity-plan.md"


def server_routes() -> set[str]:
    text = SERVER.read_text(encoding="utf-8")
    return set(re.findall(r'\.route\(\s*"([^"]+)"', text))


class PlanRouteDriftTests(unittest.TestCase):
    def test_plan_route_count_matches_the_server(self) -> None:
        plan = PLAN.read_text(encoding="utf-8")
        match = re.search(r"as of 2026-09-\d{2}: (\d+) routes", plan)
        assert match is not None, "plan route count sentence is missing"
        stated = int(match.group(1))
        self.assertEqual(
            stated,
            len(server_routes()),
            "the canonical plan route count drifted from lib.rs",
        )

    def test_read_only_endpoints_are_described_in_the_plan(self) -> None:
        plan = PLAN.read_text(encoding="utf-8")
        routes = server_routes()
        for route in (
            "/system/time",
            "/system-log/config",
            "/system-log/messages",
            "/application/version",
            "/application/preview",
            "/vm/info",
            "/vm/hardware",
            "/vm/autostart",
            "/download/image/enabled",
        ):
            self.assertIn(route, routes, f"server no longer exposes {route}")
        for marker in (
            "`time` read-only",
            "`config`,`messages` read-only",
            "`version`,`preview` read-only",
            "`hostname`,`info`,`hardware` read-only",
            "`autostart` list/content read-only",
            "`image/enabled`,`image/remote/enabled` read-only",
        ):
            self.assertIn(marker, plan, f"plan does not describe: {marker}")


if __name__ == "__main__":
    _ = unittest.main()
