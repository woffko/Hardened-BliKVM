"""On the board, read only: where in physical memory did the video engine put
the parameter sets its bit writer was given?

A lab run of the hardware H.264 plan found the engine writing its stream
somewhere else than the buffer it was told (docs/design/h616-ve-h264-encoder.md,
V2.1). This reads the whole of the board's memory through /dev/mem with
read(), which fails cleanly on what the kernel does not map, and prints every
run of the repeated SPS and PPS of a 320x240 stream. It writes nothing.

usage: python3 ve_memory_scan.py
"""

import os

# SPS and PPS of the lab tool's 320x240 stream, behind their start codes.
PATTERN = bytes.fromhex("000000016742c01e95a0507e80784423500000000168ce3112")
# Without the zeros of the first start code, which memory is full of.
NEEDLE = PATTERN[4:]
MEMORY_BASE, MEMORY_END, CHUNK = 0x4000_0000, 0x8000_0000, 1 << 20


def main() -> None:
    descriptor = os.open("/dev/mem", os.O_RDONLY)
    matches: list[int] = []
    unreadable = 0
    tail = b""
    address = MEMORY_BASE
    while address < MEMORY_END:
        try:
            os.lseek(descriptor, address, os.SEEK_SET)
            data = os.read(descriptor, CHUNK)
        except OSError:
            unreadable += 1
            tail = b""
            address += CHUNK
            continue
        block = tail + data
        start = 0
        while (at := block.find(NEEDLE, start)) >= 0:
            matches.append(address - len(tail) + at - 4)
            start = at + 1
        tail = data[-len(NEEDLE):]
        address += CHUNK
    os.close(descriptor)

    # Repeats lie one pattern apart.
    runs: list[list[int]] = []
    for match in matches:
        if runs and match == runs[-1][0] + runs[-1][1] * len(PATTERN):
            runs[-1][1] += 1
        else:
            runs.append([match, 1])
    print(f"unreadable MiB: {unreadable}; matches: {len(matches)}; runs: {len(runs)}")
    for start, count in runs[:64]:
        print(f"{start:#010x}: {count} repeats, {count * len(PATTERN)} bytes")


if __name__ == "__main__":
    main()
