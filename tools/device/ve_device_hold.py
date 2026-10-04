#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Holds the board's video engine device open for a time, and does nothing else
(the busy-engine check of docs/baselines/work-plan-20261004-review-fixes.md, F3).

usage: python3 ve_device_hold.py SECONDS [DEVICE]

The engine's driver gives its device to one opener at a time (a second open is
refused with EBUSY), so while this process holds the node the encoder cannot
have the engine, x264 encodes, and the encoder's report and the video menu
say why. DEVICE is /dev/hardened-blikvm-ve-enc unless it is named. The process
opens the node for reading and writing, as the encoder does, and asks for
nothing: no ioctl, no mapping, no write. It closes the node when SECONDS have
passed or on SIGTERM or SIGINT, and says so. Run it on the board, as a file
that was copied there, with no viewer watching.
"""

import errno
import os
import signal
import sys
import time

DEVICE = "/dev/hardened-blikvm-ve-enc"
LONGEST = 600


def main(arguments: list[str]) -> int:
    if len(arguments) not in (1, 2) or not arguments[0].isdigit():
        print(__doc__, file=sys.stderr)
        return 2
    seconds = int(arguments[0])
    if not 1 <= seconds <= LONGEST:
        print(f"SECONDS is 1 to {LONGEST}", file=sys.stderr)
        return 2
    device = arguments[1] if len(arguments) == 2 else DEVICE
    try:
        descriptor = os.open(device, os.O_RDWR | os.O_CLOEXEC)
    except OSError as error:
        busy = " (somebody else holds it)" if error.errno == errno.EBUSY else ""
        print(f"could not open {device}: {error.strerror}{busy}", flush=True)
        return 1
    stop = []
    signal.signal(signal.SIGTERM, lambda *_: stop.append("SIGTERM"))
    signal.signal(signal.SIGINT, lambda *_: stop.append("SIGINT"))
    print(f"holding {device} for {seconds} s", flush=True)
    deadline = time.monotonic() + seconds
    while not stop and time.monotonic() < deadline:
        time.sleep(min(0.2, max(0.0, deadline - time.monotonic())))
    os.close(descriptor)
    print(f"released {device}" + (f" on {stop[0]}" if stop else ""), flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
