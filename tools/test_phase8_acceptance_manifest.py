"""Guard the acceptance manifest against phantom receipts.

Every device item pins a probe (or a marker key) that some tool must actually
write. Two defects of exactly this class were found by hand: the manifest asked
for `board-handoff-acceptance-*.json` and
`restricted-control-media-attach-eject`, neither of which any tool produces, so
a successful device run would never have satisfied the audit. This check walks
the manifest and requires each probe to appear in a writer tool.
"""

from __future__ import annotations

import unittest
from pathlib import Path

from tools import phase8_acceptance_report as report


ROOT = Path(__file__).resolve().parent.parent
WRITER_GLOBS = ("tools/*.py", "tools/*.mjs", "tools/playwright/*.mjs")
# The report itself only consumes receipts; the removal tool both writes its own
# receipt and reads the acceptance ones, so it stays a writer here.
CONSUMERS = {"tools/phase8_acceptance_report.py"}


def writer_sources() -> dict[str, str]:
    sources: dict[str, str] = {}
    for pattern in WRITER_GLOBS:
        for path in ROOT.glob(pattern):
            relative = path.relative_to(ROOT).as_posix()
            if relative in CONSUMERS or path.name.startswith("test_"):
                continue
            sources[relative] = path.read_text(encoding="utf-8")
    return sources


class Phase8AcceptanceManifestTests(unittest.TestCase):
    def test_every_pinned_probe_has_a_writer_tool(self) -> None:
        sources = writer_sources()
        for spec in report.PHASES:
            for item in spec["device"]:
                probe = item.get("probe")
                if not probe:
                    continue
                writers = [name for name, text in sources.items() if probe in text]
                self.assertTrue(
                    writers,
                    f"{item['name']} pins probe {probe!r}, but no tool writes it",
                )

    def test_marker_keys_are_written_by_the_probe_owner(self) -> None:
        sources = writer_sources()
        for spec in report.PHASES:
            for item in spec["device"]:
                probe = item.get("probe")
                if not probe:
                    continue
                writers = [text for text in sources.values() if probe in text]
                for key in (item.get("marker", ""), *item.get("require", ())):
                    top_level = key.partition("=")[0].split(".")[0]
                    if not top_level:
                        continue
                    self.assertTrue(
                        any(top_level in text for text in writers),
                        f"{item['name']} expects {top_level!r}, absent from every "
                        f"writer of {probe!r}",
                    )

    def test_every_glob_prefix_is_documented(self) -> None:
        docs = "\n".join(
            [
                (ROOT / "docs/baselines/phase8-acceptance.md").read_text(encoding="utf-8"),
                (ROOT / "docs/design/phase3-5-device-gate-runbook.md").read_text(
                    encoding="utf-8"
                ),
            ]
        )
        for spec in report.PHASES:
            for item in spec["device"]:
                prefix = item["glob"].split("*", 1)[0]
                self.assertIn(
                    prefix,
                    docs,
                    f"{item['name']} expects {item['glob']}, undocumented in the audit/runbook",
                )

    def test_marker_only_items_pin_a_field_that_exists_somewhere(self) -> None:
        sources = writer_sources()
        for spec in report.PHASES:
            for item in spec["device"]:
                if item.get("probe"):
                    continue
                key = (item.get("marker") or item.get("require", ("",))[0]).partition(
                    "="
                )[0]
                top_level = key.split(".")[0]
                if not top_level:
                    continue
                self.assertTrue(
                    any(top_level in text for text in sources.values()),
                    f"{item['name']} expects {top_level!r}, which no tool writes",
                )


if __name__ == "__main__":
    unittest.main()
