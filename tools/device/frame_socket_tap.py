#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""The frames an H.264 encoder hands to the QUIC gateway, read from its
frame socket beside the gateway, and their rate over a QUIC probe's span
(docs/design/h264-engine-choice-and-quality-monitor-v1.md, gate C5).

usage: python3 frame_socket_tap.py tap SOCKET SECONDS OUTPUT
       python3 frame_socket_tap.py rate OUTPUT RECEIPT

`tap`, on the board: connects to the frame socket as one more client, as
the gateway does, and writes a line to OUTPUT for every record that comes
within SECONDS: when it came (the board's monotonic clock), its kind,
stream and frame, the frame's size and its capture time. It asks for no
key frame (the encoder makes one for every client that connects, as for
the gateway) and holds no demand: the encoder encodes only while somebody
watches. It reads all the time, so that its queue never fills.

`rate`, on the host: the frames whose capture time lies within the span of
`quic_video_device_probe.mjs`'s receipt (after the newest frame drawn at
its start, up to the newest drawn at its end), their bytes a second over
that span by the board's clock, beside the quality monitor's Speed and
the relays' count of the QUIC packets. The status is 0 where the monitor
is within 10% of the frames' rate, as gate C4 held it to the encoder's
frames on the host.
"""

import json
import socket
import struct
import sys
import time

HEADER = struct.Struct(">BBBBHHIIIIQ")
VERSION, CODEC_H264 = 1, 1
KEY, DELTA = 1, 2
WITHIN = 0.10


def tap(path, seconds, output):
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.connect(path)
    connection.settimeout(0.5)
    end = time.monotonic() + seconds
    pending = bytearray()
    records = []
    while time.monotonic() < end:
        try:
            chunk = connection.recv(1 << 16)
        except socket.timeout:
            continue
        if not chunk:
            break
        came = time.monotonic()
        pending += chunk
        while len(pending) >= HEADER.size:
            version, kind, codec, _, _, _, stream, frame, _, size, capture = (
                HEADER.unpack_from(pending)
            )
            if version != VERSION or codec != CODEC_H264 or kind not in (KEY, DELTA):
                raise SystemExit("a record of another form")
            if len(pending) < HEADER.size + size:
                break
            del pending[: HEADER.size + size]
            records.append(
                {
                    "at": round(came, 6),
                    "kind": "key" if kind == KEY else "delta",
                    "stream": stream,
                    "frame": frame,
                    "size": size,
                    "capture": capture,
                }
            )
    connection.close()
    with open(output, "w") as lines:
        for record in records:
            lines.write(json.dumps(record) + "\n")
    print(f"{len(records)} records")
    return 0


def rate(output, receipt_path):
    with open(output) as lines:
        records = [json.loads(line) for line in lines if line.strip()]
    with open(receipt_path) as receipt_file:
        receipt = json.load(receipt_file)
    span = receipt["picture"]["captureMicros"]
    seconds = (span["last"] - span["first"]) / 1e6
    within = [
        record for record in records if span["first"] < record["capture"] <= span["last"]
    ]
    frames_bytes = sum(record["size"] for record in within)
    measured = round(frames_bytes * 8 / 1e6 / seconds, 4)
    shown = receipt["monitor"]["megabitsPerSecond"]
    result = {
        "boardSeconds": round(seconds, 2),
        "frames": len(within),
        "keyFrames": sum(1 for record in within if record["kind"] == "key"),
        "framesBytes": frames_bytes,
        "framesMegabitsPerSecond": measured,
        "monitorMegabitsPerSecond": shown,
        "transportMegabitsPerSecond": receipt["monitorCounted"].get(
            "transportMegabitsPerSecond"
        ),
        "monitorWithin10": shown is not None
        and measured > 0
        and abs(shown - measured) <= WITHIN * measured,
    }
    print(json.dumps(result))
    return 0 if result["monitorWithin10"] else 1


def main(arguments):
    if len(arguments) == 4 and arguments[0] == "tap":
        return tap(arguments[1], float(arguments[2]), arguments[3])
    if len(arguments) == 3 and arguments[0] == "rate":
        return rate(arguments[1], arguments[2])
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
