"""Keep normative firewall range exceptions exact and path-bound."""

from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/check-source-boundary.sh"
RULES = "crates/bli-kvm-control-broker/src/firewall_rules.rs"
OBSERVATION = "crates/bli-kvm-control-broker/src/firewall_observation.rs"


class SourceBoundaryTest(unittest.TestCase):
    def check_source(self, files: dict[str, str]) -> subprocess.CompletedProcess[str]:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            subprocess.run(
                ["git", "init", "--quiet", directory], check=True, capture_output=True
            )
            for relative, content in files.items():
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(content + "\n", encoding="utf-8")
            return subprocess.run(
                [str(SCRIPT)], cwd=root, capture_output=True, text=True, check=False,
                timeout=15,
            )

    def definitions(self) -> dict[str, str]:
        return {
            RULES: next(line for line in (ROOT / RULES).read_text().splitlines()
                        if line.startswith("const LOCAL_V4:")),
            OBSERVATION: next(line for line in (ROOT / OBSERVATION).read_text().splitlines()
                              if "let v4 = json!" in line),
        }

    def test_normative_definitions_pass(self) -> None:
        result = self.check_source(self.definitions())
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_exception_does_not_cover_another_file(self) -> None:
        for definition in self.definitions().values():
            with self.subTest(definition=definition):
                result = self.check_source({"unexpected.rs": definition})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("private IPv4 literal", result.stderr)

    def test_address_on_same_or_separate_line_is_rejected(self) -> None:
        address = ".".join(str(part) for part in (192, 168, 23, 45))
        for path, definition in self.definitions().items():
            for separator in (" ", "\n"):
                with self.subTest(path=path, separator=separator):
                    result = self.check_source({path: definition + separator + "// " + address})
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("private IPv4 literal", result.stderr)

    def test_modified_prefix_is_rejected(self) -> None:
        for path, definition in self.definitions().items():
            changed = definition.replace("/8,", "/9,").replace('"len":8', '"len":9')
            self.assertNotEqual(changed, definition)
            result = self.check_source({path: changed})
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("private IPv4 literal", result.stderr)


if __name__ == "__main__":
    unittest.main()
