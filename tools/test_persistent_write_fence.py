import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_write_fence as fence


class FenceTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        runtime = Path(temporary.name)
        units = runtime / "systemd/system"
        units.mkdir(parents=True, mode=0o755)
        units.chmod(0o755)
        for name, value in (("RUNTIME_ROOT", runtime), ("UNIT_ROOT", units), ("OWNER_UID", os.getuid())):
            setting = patch.object(fence, name, value)
            setting.start()
            self.addCleanup(setting.stop)
        self.guard = Path("/var/lib") / (fence.contract.PREFIX + "a" * 24)

    def test_idempotent_mask_round_trip_has_private_inode_anchors(self) -> None:
        fence.install(self.guard)
        fence.install(self.guard)
        for name in fence.contract.SOCKETS:
            public = (fence.UNIT_ROOT / name).lstat()
            anchor = (fence.root_for(self.guard) / name).lstat()
            self.assertEqual((public.st_dev, public.st_ino), (anchor.st_dev, anchor.st_ino))
            self.assertEqual(public.st_nlink, 2)
        fence.remove(self.guard)
        fence.remove(self.guard)
        self.assertEqual(list(fence.UNIT_ROOT.iterdir()), [])

    def test_identical_foreign_mask_is_not_adopted(self) -> None:
        path = fence.UNIT_ROOT / fence.contract.SOCKETS[0]
        path.symlink_to("/dev/null")
        with self.assertRaises(ValueError):
            fence.install(self.guard)
        self.assertTrue(path.is_symlink())

    def test_foreign_replacement_prevents_any_mask_removal(self) -> None:
        fence.install(self.guard)
        path = fence.UNIT_ROOT / fence.contract.SOCKETS[1]
        path.unlink()
        path.symlink_to("/dev/null")
        with self.assertRaises(ValueError):
            fence.remove(self.guard)
        self.assertTrue(all((fence.UNIT_ROOT / name).is_symlink() for name in fence.contract.SOCKETS))

    def test_interrupted_anchor_creation_can_complete(self) -> None:
        root = fence.root_for(self.guard)
        root.mkdir(mode=0o700)
        (root / fence.contract.SOCKETS[0]).symlink_to("/dev/null")
        fence.install(self.guard)
        fence.validate_owned(self.guard)
        fence.remove(self.guard)
