"""Guard the operator documentation against renamed or removed tools.

The plan, the audit and the device-gate runbook are the operator's checklist;
a path that no longer exists turns a reviewed step into a dead end. This check
requires every `tools/...` or `scripts/...` file referenced by those documents to
exist on disk.
"""

from __future__ import annotations

import re
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
DOCUMENTS = (
    ROOT / "docs/design/gui-parity-plan.md",
    ROOT / "docs/design/phase3-5-device-gate-runbook.md",
    ROOT / "docs/baselines/phase8-acceptance.md",
)
PATH = re.compile(r"(tools/[A-Za-z0-9_./-]+\.(?:py|mjs)|scripts/[A-Za-z0-9_./-]+\.sh)")


class DocToolPathsTests(unittest.TestCase):
    def test_referenced_tools_and_scripts_exist(self) -> None:
        missing: list[str] = []
        for document in DOCUMENTS:
            text = document.read_text(encoding="utf-8")
            for match in PATH.finditer(text):
                if not (ROOT / match.group(1)).exists():
                    missing.append(f"{document.name}: {match.group(1)}")
        self.assertEqual(missing, [], f"documented paths are missing: {missing}")


if __name__ == "__main__":
    unittest.main()
