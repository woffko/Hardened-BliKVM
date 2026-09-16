import subprocess
import unittest

from tools import stage_device_owner_image as image


def facts(**overrides: str) -> dict[str, str]:
    base = {
        "source_present": "yes",
        "source_bytes": "6841958400",
        "source_sha256": "e" * 64,
        "link_created": "yes",
    }
    base.update(overrides)
    return base


class OwnerImageStageTests(unittest.TestCase):
    def test_stage_script_is_shell_valid_and_reversible(self) -> None:
        script = image.build_stage_script("kubuntu.img", "kubuntu.img", "e" * 64)
        syntax = subprocess.run(
            ["sh", "-n"],
            input=script.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        self.assertIn(f"image_dir={image.IMAGE_DIR}", script)
        self.assertIn(f"source={image.SOURCE_DIR}/kubuntu.img", script)
        self.assertIn('ln -sfn "$source" "$target"', script)
        self.assertIn("sha256sum", script)
        self.assertIn("readlink", script)
        for forbidden in ("password", "sshpass", "/home/", "10.0.", "cp "):
            self.assertNotIn(forbidden, script)

    def test_names_are_plain_file_names(self) -> None:
        for bad in ("../escape.img", "dir/img.img", "", "with space.img"):
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    image.build_stage_script(bad, "kubuntu.img", "e" * 64)
                with self.assertRaises(ValueError):
                    image.build_stage_script("kubuntu.img", bad, "e" * 64)

    def test_validation_accepts_a_matching_source_and_rejects_drift(self) -> None:
        image.validate_stage(facts(), "e" * 64, 6841958400)
        for override in (
            {"source_present": "no"},
            {"source_bytes": "1"},
            {"source_sha256": "f" * 64},
            {"link_created": "no"},
        ):
            with self.subTest(override=override):
                with self.assertRaises(ValueError):
                    image.validate_stage(facts(**override), "e" * 64, 6841958400)

    def test_expected_hash_is_validated(self) -> None:
        with self.assertRaises(ValueError):
            image.build_stage_script("kubuntu.img", "kubuntu.img", "not-a-hash")


if __name__ == "__main__":
    unittest.main()
