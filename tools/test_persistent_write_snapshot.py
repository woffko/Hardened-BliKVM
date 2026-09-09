import copy
from pathlib import Path
import unittest
from unittest.mock import patch

from tools import persistent_write_snapshot as snapshot


def baseline() -> dict[str, object]:
    slots, journal = snapshot.u.STATE_ROOTS
    metadata = {"mode": 0o100600, "uid": 1, "gid": 1, "device": 1, "inode": 1}
    return {"runtime": {**dict.fromkeys(snapshot.s.CHECKED_FILES, "fixed"),
        "brokerPid": "10", "videoPid": "11", "nginxPid": "12", "brokerRestarts": "0", "videoRestarts": "0"},
        "serving": {"executableSha256": "old", "result": "verified"}, "managerStatus": {"generation": 40},
        "state": {str(slots / "current"): {**metadata, "target": "old"},
                  str(journal / "application-state.json"): {**metadata, "sha256": "old"},
                  str(slots / ("app-00000000000000000008-" + "a" * 64)): metadata},
        "nginxUnit": "fixed", "persistentDropinHashes": {"85": "fixed"}}


class SnapshotTests(unittest.TestCase):
    def test_transition_allows_only_mutable_pointer_journal_and_new_slots(self) -> None:
        before = baseline()
        after = copy.deepcopy(before)
        state = snapshot.cast(dict[str, object], after["state"])
        for name in (snapshot.u.STATE_ROOTS[0] / "current", snapshot.u.STATE_ROOTS[1] / "application-state.json"):
            entry = snapshot.cast(dict[str, object], state[str(name)])
            entry["inode"] = 2
            entry["target" if name.name == "current" else "sha256"] = "new"
        state[str(snapshot.u.STATE_ROOTS[0] / ("app-00000000000000000009-" + "b" * 64))] = {"inode": 3}
        snapshot.cast(dict[str, object], after["serving"])["executableSha256"] = "new"
        after["managerStatus"] = {"generation": 43}
        snapshot.preserve(before, after, new_boot=False, allow_transition=True)
        with self.assertRaises(ValueError):
            snapshot.preserve(before, after, new_boot=False, allow_transition=False)

    def test_no_retained_slot_removal_or_replacement(self) -> None:
        for remove in (True, False):
            before = baseline()
            after = copy.deepcopy(before)
            state = snapshot.cast(dict[str, object], after["state"])
            name = str(snapshot.u.STATE_ROOTS[0] / ("app-00000000000000000008-" + "a" * 64))
            if remove:
                del state[name]
            else:
                state[name] = {"inode": 99}
            with self.assertRaisesRegex(ValueError, "immutable"):
                snapshot.preserve(before, after, new_boot=False, allow_transition=True)

    def test_foreign_new_journal_file_is_not_a_slot(self) -> None:
        before = baseline()
        after = copy.deepcopy(before)
        snapshot.cast(dict[str, object], after["state"])[str(snapshot.u.STATE_ROOTS[1] / "foreign")] = {}
        with self.assertRaisesRegex(ValueError, "journal"):
            snapshot.preserve(before, after, new_boot=False, allow_transition=True)

    def test_new_boot_allows_pids_but_not_configuration_changes(self) -> None:
        before = baseline()
        after = copy.deepcopy(before)
        runtime = snapshot.cast(dict[str, object], after["runtime"])
        runtime["brokerPid"] = "99"
        snapshot.preserve(before, after, new_boot=True, allow_transition=True)
        with self.assertRaises(ValueError):
            snapshot.preserve(before, after, new_boot=False, allow_transition=True)
        runtime["account"] = "changed"
        with self.assertRaisesRegex(ValueError, "configuration"):
            snapshot.preserve(before, after, new_boot=True, allow_transition=True)

    def test_loaded_override_inventory_rejects_extra_path(self) -> None:
        root = Path("/var/lib") / (snapshot.contract.PREFIX + "a" * 24)
        def value(unit: str, field: str) -> str:
            if field == "FragmentPath":
                return str(snapshot.files.LIB_ROOT / unit)
            return "/etc/systemd/system/foreign.conf"
        with patch.object(snapshot.s, "property_value", side_effect=value):
            with self.assertRaisesRegex(ValueError, "overrides"):
                snapshot.loaded_units(root, wired=True, masked=False)
