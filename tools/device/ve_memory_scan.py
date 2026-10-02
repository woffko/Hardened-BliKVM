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
       python3 ve_memory_scan.py digest

`digest` prints a checksum of every 64 KiB of the kernel's own image (what
/proc/iomem calls "Kernel code": its instructions and its read-only data,
where the stray write of V2.1 landed). Two digests, one before and one
after a session, show whether anything wrote there, and where.
"""

import hashlib
import os
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


def digest() -> None:
    piece = 64 * 1024
    ranges = []
    with open("/proc/iomem", encoding="ascii") as iomem:
        for line in iomem:
            span, _, name = line.strip().partition(" : ")
            if name == "Kernel code":
                first, _, last = span.partition("-")
                ranges.append((int(first, 16), int(last, 16) + 1))
    if not ranges:
        raise SystemExit("/proc/iomem names no kernel image (not root?)")
    descriptor = os.open("/dev/mem", os.O_RDONLY)
    for first, end in ranges:
        address = first - first % piece
        while address < end:
            os.lseek(descriptor, address, os.SEEK_SET)
            data = os.read(descriptor, piece)
            print(f"{address:#010x} {hashlib.sha256(data).hexdigest()[:16]}")
            address += piece
    os.close(descriptor)


def main() -> None:
    if sys.argv[1:] == ["digest"]:
        digest()
        return
    texts = sys.argv[1:] or PATTERNS
    for text in texts:
        if len(text) < 24 or len(text) % 2 or any(digit not in "0123456789abcdefABCDEF" for digit in text):
            raise SystemExit("a pattern is at least 12 bytes in hexadecimal")
    scan(texts)


if __name__ == "__main__":
    main()
