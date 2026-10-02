#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Reads a trace of the register window (`trace.bin`, see log.h).

usage: ve_trace_report.py OUTPUT_DIR [--all] [--json FILE]
       ve_trace_report.py --compare OUTPUT_DIR OUTPUT_DIR

The report follows the run: every call into the traced library, what the
library asked of our side, the buffers it took, and its register accesses.
A value written to a register is set against the buffers' made-up
addresses, under every form in which this engine's blocks are known to
take an address, so that the form the library uses shows itself. Bits put
through the bit writer are put together again into bytes. At every start
of an encode the registers' state is printed.

`--compare` prints, encode by encode, the registers that two runs left
different: one parameter is changed per run.
"""

import json
import struct
import sys
from pathlib import Path

RECORD = struct.Struct("<B39sQQQ")
READ, WRITE, CALL, RETURN, OP, ALLOC, FREE, NOTE = range(1, 9)

NAMES = {
    0x000: "mode",
    0x004: "reset",
    0x0E0: "decoder_ip",
    0x0E4: "encoder_ip",
    0x0F0: "version",
    0xA00: "isp.size",
    0xA04: "isp.stride",
    0xA08: "isp.ctrl",
    0xA10: "isp.status",
    0xA2C: "isp.scaler_size",
    0xA70: "isp.output_luma",
    0xA74: "isp.output_chroma",
    0xA78: "isp.input_luma",
    0xA7C: "isp.input_chroma",
    0xA80: "isp.input_chroma2",
    0xB04: "avc.para0",
    0xB08: "avc.para1",
    0xB0C: "avc.para2",
    0xB10: "avc.me",
    0xB14: "avc.int_en",
    0xB18: "avc.trigger",
    0xB1C: "avc.status",
    0xB20: "avc.put_data",
    0xB24: "avc.overtime_mb",
    0xB28: "avc.intra_refresh",
    0xB2C: "avc.rc_init",
    0xB30: "avc.rc_mad_th0",
    0xB34: "avc.rc_mad_th1",
    0xB38: "avc.rc_mad_th2",
    0xB3C: "avc.rc_mad_th3",
    0xB40: "avc.rc_mad_sum",
    0xB44: "avc.temporal_filter",
    0xB48: "avc.dynamic_me0",
    0xB4C: "avc.dynamic_me1",
    0xB50: "avc.mad",
    0xB54: "avc.residual_bits",
    0xB58: "avc.header_bits",
    0xB5C: "avc.me_info",
    0xB60: "avc.mv_buffer",
    0xB80: "avc.stream_start",
    0xB84: "avc.stream_end",
    0xB88: "avc.stream_offset",
    0xB8C: "avc.stream_max",
    0xB90: "avc.stream_length",
    0xBA0: "avc.ref_luma",
    0xBA4: "avc.ref_chroma",
    0xBA8: "avc.ref1_luma",
    0xBAC: "avc.ref1_chroma",
    0xBB0: "avc.rec_luma",
    0xBB4: "avc.rec_chroma",
    0xBB8: "avc.subpixel_last",
    0xBBC: "avc.subpixel_new",
    0xBC0: "avc.mb_info",
    0xBC4: "avc.deblock",
    0xBC8: "avc.temporal_filter_counts",
}

TRIGGER, STATUS, PUT_DATA, PARA0 = 0xB18, 0xB1C, 0xB20, 0xB04
NO_PREVENTION = 1 << 31


def register(offset):
    name = NAMES.get(offset)
    return f"{offset:#05x} {name}" if name else f"{offset:#05x}"


def records(path):
    data = Path(path).read_bytes()
    if len(data) % RECORD.size:
        raise SystemExit(f"{path}: not a whole number of records")
    for at in range(0, len(data), RECORD.size):
        kind, name, a, b, c = RECORD.unpack_from(data, at)
        yield kind, name.split(b"\0", 1)[0].decode("utf-8", "replace"), a, b, c


# The forms in which a block of this engine is known to take an address:
# as it is (the encoder of the older generations, the H.264 decoder's
# pictures), in units of 256 bytes (the H.265 decoder, Linux' cedrus), and
# with its top four bits moved to the bottom (the H.264 decoder's stream).
FORMS = {
    "plain": lambda value: value,
    "units of 256 bytes": lambda value: value << 8,
    "top four bits at the bottom": lambda value: (value & 0x0FFFFFF0) | ((value & 0xF) << 28),
}


class Buffers:
    def __init__(self):
        self.list = {}

    def add(self, number, size, address):
        self.list[number] = (address, size)

    def find(self, address):
        for number, (start, size) in self.list.items():
            # An end register names the last byte or the one behind it.
            if start <= address <= start + size:
                return number, address - start
        return None

    def meaning(self, value, offset_taken):
        """Every form under which `value` names a place in a buffer."""
        found = []
        for form, undo in FORMS.items():
            for back in (0, offset_taken) if offset_taken else (0,):
                place = self.find(undo(value) + back)
                if place and value:
                    label = form + (f", {back:#x} taken off" if back else "")
                    found.append((label, place[0], place[1]))
        return found


class BitWriter:
    """The bytes that puts through the bit writer make, as the maps say."""

    def __init__(self):
        self.bytes = bytearray()
        self.bits = 0
        self.count = 0
        self.puts = 0

    def put(self, value, count):
        for bit in range(count - 1, -1, -1):
            self.bits = (self.bits << 1) | ((value >> bit) & 1)
            self.count += 1
            if self.count == 8:
                self.bytes.append(self.bits)
                self.bits = 0
                self.count = 0

    def golomb(self, code):
        length = (code + 1).bit_length() - 1
        self.put(0, length)
        self.put(code + 1, length + 1)

    def text(self):
        tail = f" and {self.count} bits ({self.bits:#x})" if self.count else ""
        return f"{len(self.bytes)} bytes{tail} in {self.puts} puts: {self.bytes.hex(' ')}"


def settings(directory):
    found = {}
    run = Path(directory) / "run.txt"
    if run.exists():
        for line in run.read_text(errors="replace").splitlines():
            parts = line.split()
            if len(parts) == 4 and parts[0] == "setting" and parts[2] == "=":
                found[parts[1]] = int(parts[3], 16)
    return found


def walk(directory, everything=False, out=None):
    """Prints the report; returns the registers' state at each encode."""

    def say(text=""):
        if out is not None:
            print(text, file=out)

    buffers = Buffers()
    offset_taken = settings(directory).get("offset", 0)
    state = {}
    encodes = []
    writer = None
    forms_seen = {}
    counts = {}
    pending = None  # (offset, reads, writes, first read, last written)

    def flush():
        nonlocal pending
        if pending is None:
            return
        offset, reads, writes, first, last = pending
        pending = None
        if not everything:
            # The bit writer's data is told with its bytes, and the status
            # is worth a line only when it says more than "ready".
            if offset == PUT_DATA or (offset == STATUS and not writes and not first & 0xF):
                return
        parts = []
        if reads:
            parts.append(f"read {first:#010x}" + (f" ({reads} times)" if reads > 1 else ""))
        if writes:
            parts.append(f"written {last:#010x}" + (f" ({writes} times)" if writes > 1 else ""))
            for label, number, inside in buffers.meaning(last, offset_taken):
                parts.append(f"= buffer {number} + {inside:#x} [{label}]")
                forms_seen.setdefault(offset, set()).add(label)
        say(f"    {register(offset):32} " + ", ".join(parts))

    def end_writer():
        nonlocal writer
        if writer is not None and writer.puts:
            say(f"    bit writer: {writer.text()}")
        writer = None

    for kind, name, a, b, c in records(Path(directory) / "trace.bin"):
        if kind in (READ, WRITE):
            offset, value = a, b
            counts.setdefault(offset, [0, 0])[kind - 1] += 1
            if kind == WRITE:
                state[offset] = value
            if kind == WRITE and offset == TRIGGER:
                flush()
                what = value & 0xF
                bits = (value >> 8) & 0x3F
                if what in (1, 2, 4, 5):
                    if writer is None:
                        writer = BitWriter()
                    data = state.get(PUT_DATA, 0)
                    writer.puts += 1
                    if what == 1:
                        writer.put(data, min(bits, 32))
                    elif what == 2 and writer.count:
                        writer.put(0, 8 - writer.count)
                    elif what == 4:
                        signed = data - (1 << 32) if data & (1 << 31) else data
                        writer.golomb(signed * 2 - 1 if signed > 0 else -signed * 2)
                    elif what == 5:
                        writer.golomb(data)
                    if everything:
                        say(f"    put: type {what}, {bits} bits of {data:#x}")
                    continue
                end_writer()
                if what in (8, 9):
                    say(f"    {register(offset):32} written {value:#010x}: AN ENCODE STARTS")
                    encodes.append(dict(state))
                    say("    the registers at this start:")
                    for at in sorted(state):
                        if at >= 0xA00 or at < 0x10:
                            meanings = buffers.meaning(state[at], offset_taken)
                            note = "".join(
                                f"  = buffer {number} + {inside:#x} [{label}]"
                                for label, number, inside in meanings
                            )
                            say(f"      {register(at):32} {state[at]:#010x}{note}")
                else:
                    say(f"    {register(offset):32} written {value:#010x}")
                continue
            if pending is not None and pending[0] != offset:
                flush()
            if pending is None:
                pending = (offset, 0, 0, value, value)
            offset, reads, writes, first, last = pending
            if kind == READ:
                pending = (offset, reads + 1, writes, first if reads else value, last)
            else:
                pending = (offset, reads, writes + 1, first, value)
            continue
        flush()
        if kind == CALL:
            end_writer()
            say(f"\n{name}")
        elif kind == RETURN:
            end_writer()
            say(f"  -> {a if a < 1 << 63 else a - (1 << 64)}")
        elif kind == OP:
            say(f"    [{name}] {a:#x} {b:#x} {c:#x}")
        elif kind == ALLOC:
            buffers.add(a, b, c)
            say(f"    [buffer {a}: {b} bytes at {c:#x}]")
        elif kind == FREE:
            say(f"    [buffer {a} freed]")
        elif kind == NOTE:
            say(f"    ({name}: {a:#x})")
    flush()
    end_writer()

    say("\nRegisters touched (reads, writes):")
    for offset in sorted(counts):
        reads, writes = counts[offset]
        say(f"  {register(offset):32} {reads:6} {writes:6}")
    say("\nAddress forms seen:")
    for offset in sorted(forms_seen):
        say(f"  {register(offset):32} {', '.join(sorted(forms_seen[offset]))}")
    return {
        "encodes": [{f"{at:#05x}": value for at, value in sorted(encode.items())} for encode in encodes],
        "forms": {f"{at:#05x}": sorted(labels) for at, labels in sorted(forms_seen.items())},
        "buffers": {str(number): {"address": start, "bytes": size} for number, (start, size) in buffers.list.items()},
    }


def compare(first, second):
    one = walk(first)["encodes"]
    two = walk(second)["encodes"]
    print(f"{first}: {len(one)} encodes; {second}: {len(two)} encodes")
    for index, (left, right) in enumerate(zip(one, two)):
        print(f"encode {index}:")
        for key in sorted(set(left) | set(right)):
            if left.get(key) != right.get(key):
                show = lambda value: "-" if value is None else f"{value:#010x}"
                name = NAMES.get(int(key, 16), "")
                print(f"  {key} {name:28} {show(left.get(key))}  {show(right.get(key))}")


def main(arguments):
    if len(arguments) == 3 and arguments[0] == "--compare":
        compare(arguments[1], arguments[2])
        return 0
    if not arguments or arguments[0].startswith("-"):
        print(__doc__, file=sys.stderr)
        return 2
    summary = walk(arguments[0], everything="--all" in arguments, out=sys.stdout)
    if "--json" in arguments:
        target = arguments[arguments.index("--json") + 1]
        Path(target).write_text(json.dumps(summary, indent=1) + "\n")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
