#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Plays JPEG pictures as a capture stream, on the board: what uStreamer
gives the encoder, without a target that shows anything
(docs/design/h616-ve-h264-encoder.md, V7.1).

usage: python3 ve_capture_play.py SOCKET PICTURES FPS [SECONDS]

PICTURES is one file of JPEG pictures, one after the other (ffmpeg's
`-f image2pipe -c:v mjpeg`). They are played forth and back for ever, FPS
of them a second, to whoever asks the Unix socket SOCKET for `GET /stream`,
in the framing of uStreamer's stream: one client at a time, and the same
part headers. A client that reads slower than the pictures come loses
pictures, as with a capture: the newest is sent when it is ready for one.
With SECONDS the player ends by itself.

The pictures are read into memory once; nothing else is written anywhere.
"""

import os
import socket
import sys
import time

BOUNDARY = b"boundarydonotcross"


def pictures(data):
    """The JPEG pictures of a file: from every start marker to its end."""
    found = []
    at = 0
    while True:
        start = data.find(b"\xff\xd8\xff", at)
        if start < 0:
            return found
        end = data.find(b"\xff\xd9", start + 2)
        # An end marker can occur inside a picture's data only after a
        # stuffed byte, which a JPEG encoder never leaves as FF D9.
        if end < 0:
            return found
        found.append(data[start:end + 2])
        at = end + 2


def serve(client, ring, fps, deadline):
    request = b""
    while b"\r\n\r\n" not in request and len(request) < 4096:
        chunk = client.recv(4096)
        if not chunk:
            return 0
        request += chunk
    if not request.startswith(b"GET /stream "):
        client.sendall(b"HTTP/1.0 404 Not Found\r\n\r\n")
        return 0
    client.sendall(
        b"HTTP/1.0 200 OK\r\nCache-Control: no-store\r\n"
        b"Content-Type: multipart/x-mixed-replace;boundary=" + BOUNDARY + b"\r\n\r\n"
    )
    sent = 0
    began = time.monotonic()
    index = 0
    period = 2 * len(ring) - 2 if len(ring) > 1 else 1
    while deadline is None or time.monotonic() < deadline:
        # The picture whose time it is: a slow reader skips pictures.
        due = int((time.monotonic() - began) * fps)
        if due < index:
            time.sleep(max(0.0, began + index / fps - time.monotonic()))
            continue
        index = due
        place = index % period
        picture = ring[place if place < len(ring) else period - place]
        part = (
            b"--" + BOUNDARY + b"\r\nContent-Type: image/jpeg\r\nContent-Length: "
            + str(len(picture)).encode() + b"\r\nX-Timestamp: " + f"{time.time():.6f}".encode()
            + b"\r\n\r\n" + picture + b"\r\n"
        )
        try:
            client.sendall(part)
        except OSError:
            return sent
        sent += 1
        index += 1
    return sent


def main(arguments):
    if len(arguments) not in (3, 4):
        print(__doc__, file=sys.stderr)
        return 2
    path, source, fps = arguments[0], arguments[1], float(arguments[2])
    deadline = time.monotonic() + float(arguments[3]) if len(arguments) == 4 else None
    with open(source, "rb") as file:
        ring = pictures(file.read())
    if not ring or fps <= 0:
        print("no pictures, or no rate", file=sys.stderr)
        return 1
    if os.path.exists(path):
        os.unlink(path)
    listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    listener.bind(path)
    os.chmod(path, 0o660)
    listener.listen(1)
    listener.settimeout(1.0)
    print(f"playing {len(ring)} pictures at {fps:g} a second on {path}", flush=True)
    total = 0
    try:
        while deadline is None or time.monotonic() < deadline:
            try:
                client, _ = listener.accept()
            except socket.timeout:
                continue
            with client:
                client.settimeout(None)
                sent = serve(client, ring, fps, deadline)
            total += sent
            print(f"a client took {sent} pictures", flush=True)
    finally:
        listener.close()
        if os.path.exists(path):
            os.unlink(path)
    print(f"played {total} pictures", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
