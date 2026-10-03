#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Gives the H.264 encoder the board's choice of its engine on the board, as
the web server does on a viewer's demand connection, and leaves again
(docs/design/h264-engine-choice-and-quality-monitor-v1.md, gate C1).

usage: python3 ve_engine_choose.py SOCKET hardware|software KBPS STATUS

The connection names the codec, the bitrate and the engine, stays until
the status word STATUS names what encodes as chosen (x264 for software), at
most ten seconds, and closes: for that while the encoder has a viewer, as for
one that comes and goes. The status is 0 where the word followed.
"""

import socket
import sys
import time


def word(path):
    try:
        with open(path) as status:
            return status.read().strip()
    except OSError:
        return None


def main(arguments):
    if len(arguments) != 4 or arguments[1] not in ("hardware", "software"):
        print(__doc__, file=sys.stderr)
        return 2
    path, engine, kbps, status = arguments[0], arguments[1], int(arguments[2]), arguments[3]
    expected = "x264" if engine == "software" else "hardware"
    began = time.monotonic()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as demand:
        demand.connect(path)
        demand.sendall(f"h264\nkbps {kbps}\nengine {engine}\n".encode("ascii"))
        while time.monotonic() - began < 10.0 and word(status) != expected:
            time.sleep(0.01)
    seen = word(status)
    print(f"chose {engine}: the status word says {seen} after {time.monotonic() - began:.2f} s", flush=True)
    return 0 if seen == expected else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
