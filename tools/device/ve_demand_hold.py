#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Holds a viewer's demand on the H.264 encoder, on the board: what the web
server does for a WebRTC viewer, without a viewer and without a connection
from outside (docs/design/h616-ve-h264-encoder.md, V7.1).

usage: python3 ve_demand_hold.py SOCKET CODEC [KBPS]

The encoder works while somebody holds its demand socket: the first line
names the codec, a later `kbps N` line the bitrate, as the web server sends
them. The hold lasts until this process is ended or the encoder closes the
socket; nothing is written anywhere else.
"""

import signal
import socket
import sys


def main(arguments: list[str]) -> int:
    if len(arguments) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    path, codec = arguments[0], arguments[1]
    kbps = int(arguments[2]) if len(arguments) == 3 else None
    signal.signal(signal.SIGTERM, lambda *_: sys.exit(0))
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as demand:
        demand.connect(path)
        demand.sendall(codec.encode("ascii") + b"\n")
        if kbps is not None:
            demand.sendall(f"kbps {kbps}\n".encode("ascii"))
        print(f"holding the demand for {codec}" + (f" at {kbps} kbit/s" if kbps else ""), flush=True)
        # The encoder writes nothing back: an empty read is the socket closed.
        while demand.recv(64):
            pass
    print("the encoder closed the demand socket", flush=True)
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
