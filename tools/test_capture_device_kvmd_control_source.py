import unittest

from tools import capture_device_kvmd_control_source as capture


def file_lines() -> list[str]:
    return [
        f"file:{digest}:{path}"
        for path, digest in capture.EXPECTED_SOURCE_HASHES.items()
    ]


DISASSEMBLY = [
    "    17c4:\tmov\tw1, #0x1",
    "    17c8:\tbl\t25e8 <AIOWriteGPIO>",
    "    17cc:\tmov\tw0, #0xa120                \t// #41248",
    "    17d0:\tmovk\tw0, #0x7, lsl #16",
    "    17d4:\tbl\t14a0 <usleep@plt>",
    "    17e4:\tmov\tw1, #0x0",
    "    17e8:\tbl\t25e8 <AIOWriteGPIO>",
    "    1808:\tmov\tw1, #0x1",
    "    180c:\tbl\t25e8 <AIOWriteGPIO>",
    "    1810:\tmov\tw0, #0x4b40                \t// #19264",
    "    1814:\tmovk\tw0, #0x4c, lsl #16",
    "    1818:\tbl\t14a0 <usleep@plt>",
    "    1828:\tmov\tw1, #0x0",
    "    182c:\tbl\t25e8 <AIOWriteGPIO>",
    "    184c:\tmov\tw1, #0x1",
    "    1850:\tbl\t25e8 <AIOWriteGPIO>",
    "    1854:\tmov\tw0, #0xa120                \t// #41248",
    "    1858:\tmovk\tw0, #0x7, lsl #16",
    "    185c:\tbl\t14a0 <usleep@plt>",
    "    186c:\tmov\tw1, #0x0",
    "    1870:\tbl\t25e8 <AIOWriteGPIO>",
]


def probe(*, pack: str | None = None, second_pulse: str | None = None) -> bytes:
    lines = file_lines()
    lines.append("remap:" + capture.EXPECTED_REMAP)
    lines.append("pack:" + (pack or capture.EXPECTED_PACK))
    lines.append("mouserange:-32768:32767")
    lines.append("kvmd:gpio")
    lines.append("chip:/dev/gpiochip0")
    for pin, value in capture.EXPECTED_ATX_PINS.items():
        lines.append(f"pin:{pin}:{value}")
    lines.append("objdump:/usr/bin/aarch64-linux-gnu-objdump")
    for line in DISASSEMBLY:
        if second_pulse and line.strip().startswith("1810"):
            line = "\t".join(["    1810:", "mov\tw0, #0x1000"])
        lines.append("dis:" + line)
    return ("\n".join(lines) + "\n").encode()


class CaptureDeviceKvmdControlSourceTests(unittest.TestCase):
    def test_valid_probe_records_the_pinned_semantics(self) -> None:
        facts = capture.parse_probe(probe())
        self.assertIs(facts["sourceRevisionMatches"], True)
        self.assertIs(facts["absoluteRemap"]["matchesPinned"], True)
        self.assertEqual(facts["absoluteRemap"]["inMin"], -32768)
        self.assertEqual(facts["absoluteRemap"]["inMax"], 32767)
        self.assertEqual(facts["mouseRange"], {"min": -32768, "max": 32767})
        self.assertIs(facts["reportFormats"]["matchesPinned"], True)
        self.assertEqual(facts["reportFormats"]["absolute"]["function"], "hid.usb1")
        self.assertEqual(facts["reportFormats"]["absolute"]["reportLength"], 7)
        self.assertEqual(facts["reportFormats"]["relative"]["function"], "hid.usb2")
        self.assertEqual(facts["reportFormats"]["relative"]["reportLength"], 5)
        atx = facts["atx"]
        self.assertEqual(atx["configuredType"], "gpio")
        self.assertIs(atx["pinsMatchPinned"], True)
        self.assertEqual(
            [pulse["durationMs"] for pulse in atx["pulses"]],
            [500, 5000, 500],
        )

    def test_unexpected_report_pack_is_flagged_not_guessed(self) -> None:
        facts = capture.parse_probe(probe(pack='("<BHHbb"ifabsoluteelse"<BBbb")'))
        self.assertIs(facts["reportFormats"]["matchesPinned"], False)

    def test_changed_pulse_length_fails_closed(self) -> None:
        with self.assertRaises(ValueError):
            capture.parse_probe(probe(second_pulse="0x1000"))

    def test_unknown_source_hash_is_reported_as_a_mismatch(self) -> None:
        text = probe().decode()
        text = text.replace(
            capture.EXPECTED_SOURCE_HASHES["/etc/kvmd/override.yaml"],
            "0" * 64,
        )
        facts = capture.parse_probe(text.encode())
        self.assertIs(facts["sourceRevisionMatches"], False)
        self.assertIs(facts["atx"]["pinsMatchPinned"], True)

    def test_unexpected_line_is_rejected(self) -> None:
        with self.assertRaises(ValueError):
            capture.parse_probe(probe() + b"surprise:1\n")


if __name__ == "__main__":
    unittest.main()
