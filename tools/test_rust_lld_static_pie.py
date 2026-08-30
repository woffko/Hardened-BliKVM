from pathlib import Path
import tempfile
import unittest

from scripts import rust_lld_static_pie as linker


class RustLldStaticPieTests(unittest.TestCase):
    def _crt_directory(self, root: Path) -> Path:
        directory = root / "lib" / "rustlib" / "target" / "lib" / "self-contained"
        directory.mkdir(parents=True)
        for name in (*linker.CRT_REPLACEMENTS, *linker.STATIC_PIE_CRT):
            _ = (directory / name).touch()
        return directory

    def test_static_pie_uses_self_relocating_crt_sequence(self) -> None:
        with tempfile.TemporaryDirectory() as directory_name:
            directory = self._crt_directory(Path(directory_name))
            original = [
                "-flavor",
                "gnu",
                str(directory / "crt1.o"),
                str(directory / "crtbegin.o"),
                "object.o",
                "-static",
                "-pie",
                str(directory / "crtend.o"),
            ]
            rewritten = linker.rewrite_link_args(original)
            names = {Path(argument).name for argument in rewritten}
            self.assertTrue(linker.STATIC_PIE_CRT.issubset(names))
            self.assertFalse(set(linker.CRT_REPLACEMENTS).intersection(names))
            self.assertEqual(original[0:2], rewritten[0:2])

    def test_non_static_pie_link_is_unchanged(self) -> None:
        arguments = ["crt1.o", "object.o", "-static"]
        self.assertEqual(linker.rewrite_link_args(arguments), arguments)

    def test_incomplete_or_foreign_crt_sequence_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as directory_name:
            directory = self._crt_directory(Path(directory_name))
            with self.assertRaises(ValueError):
                _ = linker.rewrite_link_args(
                    [str(directory / "crt1.o"), "-static", "-pie"]
                )
            with self.assertRaises(ValueError):
                _ = linker.rewrite_link_args(
                    ["/foreign/crt1.o", "-static", "-pie"]
                )


if __name__ == "__main__":
    _ = unittest.main()
