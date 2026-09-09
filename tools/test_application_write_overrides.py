import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import application_write_overrides as overrides


class OverrideTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        root = Path(temporary.name)
        for target, value in (("UNIT_ROOT", root), ("OWNER_UID", os.getuid())):
            setting = patch.object(overrides, target, value)
            setting.start()
            self.addCleanup(setting.stop)

    def test_round_trip_exact_runtime_only_and_idempotent_removal(self) -> None:
        overrides.install()
        for unit in (overrides.contract.CURRENT, overrides.contract.CANDIDATE):
            self.assertIsNotNone(overrides.inspect(unit, allow_absent=False))
        overrides.remove()
        overrides.remove()
        self.assertEqual(list(overrides.UNIT_ROOT.iterdir()), [])

    def test_preexisting_namespace_is_not_adopted(self) -> None:
        overrides.directory(overrides.contract.CANDIDATE).mkdir()
        with self.assertRaises(ValueError):
            overrides.install()
        self.assertFalse(overrides.directory(overrides.contract.CURRENT).exists())

    def test_foreign_second_override_prevents_first_deletion(self) -> None:
        overrides.install()
        first = overrides.directory(overrides.contract.CURRENT) / overrides.contract.WRITE_DROPIN
        second = overrides.directory(overrides.contract.CANDIDATE) / overrides.contract.WRITE_DROPIN
        second.write_bytes(b"foreign")
        with self.assertRaises(ValueError):
            overrides.remove()
        self.assertEqual(first.read_bytes(), overrides.contract.WRITE_DROPIN_BYTES)
        self.assertEqual(second.read_bytes(), b"foreign")

    def test_symlink_and_hardlink_are_not_owned(self) -> None:
        overrides.install()
        target = overrides.directory(overrides.contract.CURRENT) / overrides.contract.WRITE_DROPIN
        extra = overrides.UNIT_ROOT / "hardlink"
        os.link(target, extra)
        with self.assertRaises(ValueError):
            overrides.remove()
        extra.unlink()
        target.unlink()
        target.symlink_to("missing")
        with self.assertRaises(ValueError):
            overrides.remove()
        self.assertTrue(target.is_symlink())

    def test_empty_owned_directory_from_interrupted_install_is_recoverable(self) -> None:
        parent = overrides.directory(overrides.contract.CURRENT)
        parent.mkdir(mode=0o755)
        parent.chmod(0o755)
        overrides.remove()
        self.assertFalse(parent.exists())


if __name__ == "__main__":
    unittest.main()
