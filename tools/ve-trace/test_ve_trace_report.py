# SPDX-License-Identifier: GPL-3.0-only
"""The trace report against a trace made by hand."""

import io
import struct
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ve_trace_report as report  # noqa: E402

RECORD = struct.Struct("<B39sQQQ")


def record(kind, name="", a=0, b=0, c=0):
    return RECORD.pack(kind, name.encode(), a, b, c)


def write(offset, value, old=0):
    return record(report.WRITE, a=offset, b=value, c=old)


def read(offset, value):
    return record(report.READ, a=offset, b=value)


class ReportTest(unittest.TestCase):
    def run_report(self, records, settings=""):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "trace.bin").write_bytes(b"".join(records))
            Path(directory, "run.txt").write_text(settings)
            out = io.StringIO()
            summary = report.walk(directory, out=out)
            return summary, out.getvalue()

    def test_addresses_bits_and_the_state_at_an_encode(self):
        stream = 0x6B002000
        summary, text = self.run_report(
            [
                record(report.ALLOC, a=0, b=4096, c=0x6B000000),
                record(report.ALLOC, a=1, b=1 << 20, c=stream),
                record(report.CALL, "Encode"),
                record(report.OP, "ve.enableVe", 1),
                read(0xB1C, 0x260),
                # The stream buffer in units of 256 bytes, a picture as it is,
                # and one with its top four bits at the bottom.
                write(0xB80, stream >> 8),
                write(0xB84, (stream + (1 << 20) - 1) >> 8),
                write(0xBA0, 0x6B000000),
                write(0xBA4, (0x6B000010 & 0x0FFFFFF0) | 0x6),
                write(0xB08, 0x1E),
                # A start code byte, an unsigned Exp-Golomb 5, the alignment.
                write(0xB04, 1 << 31),
                read(0xB1C, 0x260),
                write(0xB20, 0x67),
                write(0xB18, 1 | 8 << 8),
                write(0xB20, 5),
                write(0xB18, 5),
                write(0xB18, 2),
                write(0xB18, 8),
                record(report.OP, "ve.waitInterrupt", 1),
                read(0xB1C, 0x261),
                record(report.RETURN, "Encode", 0),
            ]
        )
        self.assertEqual(summary["forms"]["0xb80"], ["units of 256 bytes"])
        self.assertEqual(summary["forms"]["0xb84"], ["units of 256 bytes"])
        self.assertEqual(summary["forms"]["0xba0"], ["plain"])
        self.assertEqual(summary["forms"]["0xba4"], ["top four bits at the bottom"])
        self.assertNotIn("0xb08", summary["forms"])
        self.assertEqual(len(summary["encodes"]), 1)
        self.assertEqual(summary["encodes"][0]["0xb80"], stream >> 8)
        self.assertIn("bit writer: 2 bytes in 3 puts: 67 30", text)
        self.assertIn("AN ENCODE STARTS", text)
        self.assertIn("= buffer 1 + 0x0 [units of 256 bytes]", text)

    def test_an_offset_taken_off_an_address_is_recognised(self):
        summary, _ = self.run_report(
            [
                record(report.ALLOC, a=0, b=4096, c=0x6B000000),
                write(0xB80, 0x2B000000),
            ],
            "setting offset = 0x40000000\n",
        )
        self.assertEqual(summary["forms"]["0xb80"], ["plain, 0x40000000 taken off"])

    def test_a_truncated_trace_is_refused(self):
        with tempfile.TemporaryDirectory() as directory:
            Path(directory, "trace.bin").write_bytes(b"\0" * 65)
            with self.assertRaises(SystemExit):
                report.walk(directory)


if __name__ == "__main__":
    unittest.main()
