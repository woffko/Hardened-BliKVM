import copy
import unittest

from tools import application_write_state as state


def snapshot() -> dict[str, object]:
    slots, journal = state.upload.u.STATE_ROOTS
    meta = {"mode": 1, "uid": 2, "gid": 3, "device": 4, "inode": 5}
    return {"runtime": {"peerPid": 12}, "managerStatus": {"generation": 37}, "state": {
        str(slots / "current"): {**meta, "target": "source"},
        str(journal / "application-state.json"): {**meta, "sha256": "before"},
        str(slots / "immutable-file"): {**meta, "sha256": "fixed"}}}


class StateTests(unittest.TestCase):
    def test_only_atomic_pointer_and_journal_changes_are_excluded(self) -> None:
        before = snapshot()
        after = copy.deepcopy(before)
        after["managerStatus"] = {"generation": 40}
        slots, journal = state.upload.u.STATE_ROOTS
        tree = after["state"]
        assert isinstance(tree, dict)
        tree[str(slots / "current")] = {**tree[str(slots / "current")], "inode": 99, "target": "target"}
        tree[str(journal / "application-state.json")] = {**tree[str(journal / "application-state.json")], "inode": 98, "sha256": "after"}
        state.preserved(before, after, allow_transition=True)
        with self.assertRaises(ValueError):
            state.preserved(before, after, allow_transition=False)

    def test_slots_inventory_metadata_and_runtime_remain_exact(self) -> None:
        before = snapshot()
        slots, journal = state.upload.u.STATE_ROOTS
        for path, key, value in ((slots / "current", "uid", 99),
                                 (journal / "application-state.json", "mode", 99),
                                 (slots / "immutable-file", "sha256", "changed")):
            after = copy.deepcopy(before)
            tree = after["state"]
            assert isinstance(tree, dict)
            tree[str(path)][key] = value
            with self.assertRaises(ValueError):
                state.preserved(before, after, allow_transition=True)
        after = copy.deepcopy(before)
        after["runtime"] = {"peerPid": 13}
        with self.assertRaises(ValueError):
            state.preserved(before, after, allow_transition=True)
        after = copy.deepcopy(before)
        tree = after["state"]
        assert isinstance(tree, dict)
        tree["foreign"] = {}
        with self.assertRaises(ValueError):
            state.preserved(before, after, allow_transition=True)


if __name__ == "__main__":
    unittest.main()
