import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_write_files as files


class PersistentWriteFileTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        unit_root = Path(temporary.name) / "system"
        unit_root.mkdir(mode=0o755)
        unit_root.chmod(0o755)
        for name, value in (("UNIT_ROOT", unit_root), ("OWNER_UID", os.getuid())):
            setting = patch.object(files, name, value)
            setting.start()
            self.addCleanup(setting.stop)
        self.root = Path("/var/lib") / (files.contract.PREFIX + "a" * 24)
        for path in files.status_files():
            path.parent.mkdir(mode=0o755)
            path.parent.chmod(0o755)
            path.write_bytes(files.status.DROPIN_BYTES)
            path.chmod(0o644)

    def install(self) -> dict[str, object]:
        backup = files.preflight(self.root)
        files.install(self.root, backup, transaction_only=True)
        files.install(self.root, backup, transaction_only=False)
        return backup

    def test_round_trip_preserves_status_and_removes_only_owned_policy(self) -> None:
        backup = self.install()
        self.assertTrue(all(path.is_symlink() for path in files.links()))
        files.remove(self.root, backup)
        files.remove(self.root, backup)
        self.assertTrue(all(path.read_bytes() == files.status.DROPIN_BYTES for path in files.status_files()))
        self.assertTrue(all(not files.exists(path) for path in (*files.files(self.root), *files.links())))

    def test_policy_cannot_be_installed_before_guard_dependencies(self) -> None:
        backup = files.preflight(self.root)
        with self.assertRaises(OSError):
            files.install(self.root, backup, transaction_only=False)
        self.assertTrue(all(not files.exists(path) for path in files.files(self.root)))

    def test_changed_last_file_prevents_any_removal(self) -> None:
        backup = self.install()
        paths = files.files(self.root)
        last = list(paths)[-1]
        last.write_bytes(b"foreign")
        with self.assertRaises(ValueError):
            files.remove(self.root, backup)
        first = next(iter(paths))
        self.assertEqual(first.read_bytes(), paths[first])
        self.assertTrue(all(path.is_symlink() for path in files.links()))

    def test_commit_detachment_keeps_write_policy_and_enablement(self) -> None:
        backup = self.install()
        files.detach_guard(self.root, backup)
        files.detach_guard(self.root, backup)
        for path in files.files(self.root):
            self.assertEqual(files.exists(path), path.name == files.contract.DROPIN_NAME)
        self.assertTrue(all(path.is_symlink() for path in files.links()))
        files.remove(self.root, backup)

    def test_foreign_backup_directory_cannot_be_removed(self) -> None:
        backup = self.install()
        backup["createdDirectories"] = [str(files.UNIT_ROOT)]
        with self.assertRaises(ValueError):
            files.remove(self.root, backup)
        self.assertTrue(all(files.exists(path) for path in files.files(self.root)))
