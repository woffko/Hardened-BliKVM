import os
from pathlib import Path
import tempfile
import unittest

from tools.application_bootstrap_roots import BootstrapRoots, identity, rename_owned


class BootstrapRootsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.parent = Path(self.temporary.name)
        self.guard = self.parent / "guard"
        self.guard.mkdir(mode=0o700)
        self.roots = BootstrapRoots(self.guard, self.parent, "a" * 64)
        self.roots.prepare(os.geteuid(), os.getegid())

    def test_partial_publication_recovers_both_roots_without_losing_contents(self) -> None:
        first = self.guard / "prepared-slots"
        (first / "partial.bin").write_bytes(b"incomplete payload")
        rename_owned(first, self.parent / "hardened-blikvm-slots", identity(first))
        self.roots.quarantine()
        self.assertEqual((self.guard / "quarantined-slots/partial.bin").read_bytes(), b"incomplete payload")
        self.assertFalse((self.parent / "hardened-blikvm-slots").exists())
        self.assertFalse((self.parent / "hardened-blikvm-update").exists())
        self.roots.quarantine()
        self.assertEqual(self.roots.load()["phase"], "quarantined")

    def test_committed_roots_cannot_be_quarantined(self) -> None:
        self.roots.publish()
        self.roots.commit()
        with self.assertRaises(ValueError):
            self.roots.quarantine()
        self.assertTrue((self.parent / "hardened-blikvm-slots").is_dir())

    def test_foreign_destination_is_preserved_and_prevents_publication(self) -> None:
        foreign = self.parent / "hardened-blikvm-slots"
        foreign.mkdir()
        (foreign / "keep").write_text("foreign")
        with self.assertRaises(ValueError):
            self.roots.publish()
        self.assertEqual((foreign / "keep").read_text(), "foreign")
        self.assertTrue((self.guard / "prepared-state").exists())

    def test_no_replace_does_not_overwrite_even_an_empty_directory(self) -> None:
        source = self.guard / "prepared-slots"
        target = self.parent / "occupied"
        target.mkdir()
        previous = identity(target)
        with self.assertRaises(OSError):
            rename_owned(source, target, identity(source))
        self.assertEqual(identity(target), previous)
        self.assertTrue(source.exists())

    def test_replaced_live_root_is_not_moved_or_deleted(self) -> None:
        self.roots.publish()
        live = self.parent / "hardened-blikvm-slots"
        live.rename(self.parent / "saved-original")
        live.mkdir(mode=0o750)
        with self.assertRaises(ValueError):
            self.roots.quarantine()
        self.assertTrue(live.exists())
        self.assertTrue((self.parent / "hardened-blikvm-update").exists())

    def test_quarantine_preserves_inner_symlinks_without_following_them(self) -> None:
        sentinel = self.parent / "outside"
        sentinel.write_text("untouched")
        (self.guard / "prepared-slots/link").symlink_to(sentinel)
        self.roots.publish()
        self.roots.quarantine()
        self.assertEqual(sentinel.read_text(), "untouched")
        self.assertTrue((self.guard / "quarantined-slots/link").is_symlink())

    def test_incomplete_marker_write_can_recover_from_last_valid_identity_record(self) -> None:
        self.roots.publish()
        part = self.guard / "roots.json.part"
        part.write_text('{"phase":')
        part.chmod(0o600)
        self.roots.quarantine()
        self.assertEqual(self.roots.load()["phase"], "quarantined")
        self.assertFalse(part.exists())

    def test_binding_and_marker_symlink_are_rejected(self) -> None:
        foreign = BootstrapRoots(self.guard, self.parent, "b" * 64)
        with self.assertRaises(ValueError):
            foreign.load()
        self.roots.marker.rename(self.guard / "saved-marker")
        self.roots.marker.symlink_to(self.guard / "saved-marker")
        with self.assertRaises(OSError):
            self.roots.load()


if __name__ == "__main__":
    unittest.main()
