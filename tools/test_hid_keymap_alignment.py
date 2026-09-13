from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
ADAPTER = ROOT / "crates/bli-kvm-hid-adapter/src/lib.rs"
KEYMAP = ROOT / "web/src/lib/keymap.ts"


def adapter_usages(source: str) -> set[int]:
    key_name = source.split("fn key_name", 1)[1].split("\nfn ", 1)[0]
    accepted: set[int] = set()
    for start, end in re.findall(r"(\d+)\.\.=(\d+) =>", key_name):
        accepted.update(range(int(start), int(end) + 1))
    for single in re.findall(r"\n\s*(\d+) => \"", key_name):
        accepted.add(int(single))
    if "0xe0..=0xe7" in source:
        accepted.update(range(0xE0, 0xE8))
    return accepted


def gui_usages(source: str) -> dict[str, int]:
    return {
        name: int(value, 16)
        for name, value in re.findall(r"(\w+): 0x([0-9a-fA-F]{2})", source)
    }


class HidKeymapAlignmentTests(unittest.TestCase):
    def test_gui_keycodes_are_a_subset_of_the_pinned_adapter_table(self) -> None:
        accepted = adapter_usages(ADAPTER.read_text())
        gui = gui_usages(KEYMAP.read_text())
        rejected = {name: code for name, code in gui.items() if code not in accepted}
        self.assertEqual(rejected, {}, f"GUI offers unsupported usages: {rejected}")
        self.assertGreaterEqual(len(gui), 90)

    def test_adapter_table_is_the_documented_ranges(self) -> None:
        accepted = adapter_usages(ADAPTER.read_text())
        expected = (
            set(range(4, 50))
            | set(range(51, 70))
            | set(range(70, 103))
            | set(range(135, 140))
            | set(range(0xE0, 0xE8))
        )
        self.assertEqual(accepted, expected)

    def test_modifier_map_covers_exactly_the_eight_usage_buttons(self) -> None:
        source = KEYMAP.read_text()
        block = source.split("export const ModifierMap", 1)[1].split("};", 1)[0]
        names = re.findall(r"(\w+): ModifierBits\.", block)
        self.assertEqual(
            names,
            [
                "ControlLeft",
                "ShiftLeft",
                "AltLeft",
                "MetaLeft",
                "ControlRight",
                "ShiftRight",
                "AltRight",
                "MetaRight",
            ],
        )


if __name__ == "__main__":
    unittest.main()
