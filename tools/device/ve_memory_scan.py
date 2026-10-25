"""On the board, read only: where in physical memory did the video engine put
the parameter sets its bit writer was given?

A lab run of the hardware H.264 plan found the engine writing its stream
somewhere else than the buffer it was told (docs/design/h616-ve-h264-encoder.md,
V2.1). This reads the whole of the board's memory through /dev/mem with
read(), which fails cleanly on what the kernel does not map, and prints every
run of the repeated SPS and PPS of the lab tool's 320x240 stream, in both
forms the tool writes them. It writes nothing, and it does not find itself:
the patterns are kept with every bit flipped.

usage: python3 ve_memory_scan.py [HEX_PATTERN...]
       python3 ve_memory_scan.py snapshot FILE
       python3 ve_memory_scan.py compare FILE

`snapshot` copies the kernel's own image (what /proc/iomem calls "Kernel
code": its instructions and its read-only data, where the stray write of
V2.1 landed) to FILE; `compare` reads it again and says what differs. The
kernel changes its own instructions while it runs: a static key that is
switched turns single instructions from "no operation" into a branch or
back, in every function that asks the key. Such words are counted and not
held against anyone. Any other word that differs is a write that went
astray, and `compare` ends with status 3.
"""

import os
import struct
import sys

# SPS and PPS of the lab tool's 320x240 stream, behind their start codes:
# with the maps' chroma QP offset, and with none (the newer generation).
PATTERNS = [
    "000000016742c01e95a0507e80784423500000000168ce3112",
    "000000016742c01e95a0507e80784423500000000168ce3c80",
]
MEMORY_BASE, MEMORY_END, CHUNK = 0x4000_0000, 0x8000_0000, 1 << 20
# The scan must not find itself: the patterns exist in this process only
# with every bit flipped, and memory is searched flipped too.
FLIP = bytes(value ^ 0xFF for value in range(256))


def flipped(text: str) -> bytes:
    return bytes(int(text[at : at + 2], 16) ^ 0xFF for at in range(0, len(text), 2))


def scan(texts: list[str]) -> None:
    # Without the zeros of the first start code, which memory is full of.
    needles = [flipped(text)[4:] for text in texts]
    overlap = max(len(needle) for needle in needles)
    # One buffer for everything that is read, so that what was found in one
    # place is not met again in this process's own memory.
    buffer = bytearray(overlap + CHUNK)
    view = memoryview(buffer)
    matches: list[list[int]] = [[] for _ in needles]
    unreadable = 0
    kept = 0
    descriptor = os.open("/dev/mem", os.O_RDONLY)
    address = MEMORY_BASE
    while address < MEMORY_END:
        try:
            got = os.preadv(descriptor, [view[overlap:]], address)
        except OSError:
            unreadable += 1
            kept = 0
            address += CHUNK
            continue
        search = buffer.translate(FLIP)
        for index, needle in enumerate(needles):
            at = search.find(needle, overlap - kept, overlap + got)
            while at >= 0:
                matches[index].append(address - overlap + at - 4)
                at = search.find(needle, at + 1, overlap + got)
        # The end of this chunk goes before the next, for a pattern that
        # lies across both.
        kept = min(overlap - 1, got)
        buffer[overlap - kept : overlap] = buffer[overlap + got - kept : overlap + got]
        address += CHUNK
    os.close(descriptor)

    for text, found in zip(texts, matches):
        length = len(text) // 2
        # Repeats lie one pattern apart.
        runs: list[list[int]] = []
        for match in found:
            if runs and match == runs[-1][0] + runs[-1][1] * length:
                runs[-1][1] += 1
            else:
                runs.append([match, 1])
        print(
            f"pattern ...{text[-8:]}: unreadable MiB: {unreadable}; "
            f"matches: {len(found)}; runs: {len(runs)}"
        )
        for start, count in runs[:64]:
            print(f"{start:#010x}: {count} repeats, {count * length} bytes")


PIECE = 64 * 1024
NOP = 0xD503201F


def kernel_image() -> list[tuple[int, int]]:
    ranges = []
    with open("/proc/iomem", encoding="ascii") as iomem:
        for line in iomem:
            span, _, name = line.strip().partition(" : ")
            if name == "Kernel code":
                first, _, last = span.partition("-")
                ranges.append((int(first, 16), int(last, 16) + 1))
    if not ranges:
        raise SystemExit("/proc/iomem names no kernel image (not root?)")
    return ranges


def read_image() -> list[tuple[int, bytes]]:
    descriptor = os.open("/dev/mem", os.O_RDONLY)
    pieces = []
    for first, end in kernel_image():
        address = first - first % PIECE
        while address < end:
            os.lseek(descriptor, address, os.SEEK_SET)
            pieces.append((address, os.read(descriptor, PIECE)))
            address += PIECE
    os.close(descriptor)
    return pieces


def snapshot(path: str) -> None:
    with open(path, "wb") as out:
        for address, data in read_image():
            out.write(struct.pack("<QI", address, len(data)))
            out.write(data)


def branch(word: int) -> bool:
    return word >> 26 == 0b000101


def compare(path: str) -> int:
    before = {}
    with open(path, "rb") as saved:
        while header := saved.read(12):
            address, length = struct.unpack("<QI", header)
            before[address] = saved.read(length)
    pieces = read_image()
    changed = patched = 0
    other: list[tuple[int, int, int]] = []
    for address, data in pieces:
        old = before.get(address)
        if old == data:
            continue
        changed += 1
        if old is None or len(old) != len(data):
            other.append((address, 0, 0))
            continue
        for offset in range(0, len(data) - 3, 4):
            if old[offset : offset + 4] == data[offset : offset + 4]:
                continue
            was, now = struct.unpack_from("<I", old, offset)[0], struct.unpack_from("<I", data, offset)[0]
            # A jump label: no operation and a branch take each other's place.
            if (was == NOP and branch(now)) or (branch(was) and now == NOP):
                patched += 1
            else:
                other.append((address + offset, was, now))
    print(
        f"kernel image: {changed} of {len(pieces)} pieces changed: {patched} instruction "
        f"words the kernel switched itself, {len(other)} other words"
    )
    for address, was, now in other[:32]:
        print(f"{address:#010x}: {was:08x} became {now:08x}")
    return 3 if other else 0


def main() -> None:
    if len(sys.argv) == 3 and sys.argv[1] == "snapshot":
        snapshot(sys.argv[2])
        return
    if len(sys.argv) == 3 and sys.argv[1] == "compare":
        sys.exit(compare(sys.argv[2]))
    texts = sys.argv[1:] or PATTERNS
    for text in texts:
        if len(text) < 24 or len(text) % 2 or any(digit not in "0123456789abcdefABCDEF" for digit in text):
            raise SystemExit("a pattern is at least 12 bytes in hexadecimal")
    scan(texts)


if __name__ == "__main__":
    main()
