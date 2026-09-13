from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
WORKSPACE = ROOT / "Cargo.toml"
CRATES = ROOT / "crates"
LICENSING = ROOT / "docs/licensing.md"


class CrateLicenseTests(unittest.TestCase):
    def test_workspace_declares_the_declared_project_license(self) -> None:
        workspace = WORKSPACE.read_text()
        self.assertIn('license = "GPL-3.0-only"', workspace)
        self.assertIn("GPL-3.0-only", LICENSING.read_text())

    def test_every_crate_inherits_the_workspace_license(self) -> None:
        manifests = sorted(CRATES.glob("*/Cargo.toml"))
        self.assertGreater(len(manifests), 40)
        for manifest in manifests:
            text = manifest.read_text()
            self.assertIn(
                "license.workspace = true",
                text,
                f"{manifest.name} must inherit the workspace license",
            )
            self.assertIsNone(
                re.search(r'^license\s*=\s*"', text, flags=re.MULTILINE),
                f"{manifest.name} must not override the workspace license",
            )


if __name__ == "__main__":
    unittest.main()
