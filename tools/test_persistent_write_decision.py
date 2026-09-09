from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
import os
import tempfile
import unittest
from unittest.mock import patch

from tools import persistent_write_decision as decisions


class DecisionTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        parent = Path(temporary.name)
        self.root = parent / (decisions.contract.PREFIX + "a" * 24)
        self.root.mkdir(mode=0o700)
        for name, value in (("PARENT", parent), ("OWNER_UID", os.getuid())):
            setting = patch.object(decisions, name, value)
            setting.start()
            self.addCleanup(setting.stop)
        self.binding = decisions.contract.EvidenceBinding("a" * 24, "b" * 64, decisions.contract.CONFIG_SHA256,
            "app-00000000000000000008-" + "c" * 64, 8, 40)
        self.commit = decisions.Decision(self.binding, "committed", dict.fromkeys(decisions.contract.REQUIRED_GATES, "d" * 64))
        self.revoke = decisions.Decision(self.binding, "revoked", {})

    def test_late_deadline_cannot_replace_commit(self) -> None:
        self.assertEqual(decisions.publish(self.root, self.commit), self.commit)
        self.assertEqual(decisions.publish(self.root, self.revoke), self.commit)
        self.assertEqual(list(path.name for path in self.root.iterdir()), ["decision.json"])

    def test_revocation_prevents_late_commit(self) -> None:
        self.assertEqual(decisions.publish(self.root, self.revoke), self.revoke)
        self.assertEqual(decisions.publish(self.root, self.commit), self.revoke)

    def test_racing_publishers_observe_one_complete_winner(self) -> None:
        with ThreadPoolExecutor(max_workers=2) as pool:
            first = pool.submit(decisions.publish, self.root, self.commit)
            second = pool.submit(decisions.publish, self.root, self.revoke)
            self.assertEqual(first.result(), second.result())
        self.assertIn(decisions.read(self.root), (self.commit, self.revoke))
        self.assertEqual((self.root / "decision.json").stat().st_nlink, 1)

    def test_partial_or_symlink_decision_is_never_accepted(self) -> None:
        path = self.root / "decision.json"
        path.write_bytes(b"{")
        path.chmod(0o600)
        with self.assertRaises(ValueError):
            decisions.read(self.root)
        path.unlink()
        path.symlink_to(self.root / "missing")
        with self.assertRaises(ValueError):
            decisions.read(self.root)


if __name__ == "__main__":
    unittest.main()
