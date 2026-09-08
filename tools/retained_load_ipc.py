"""Bounded read-only IPC clients for the retained-stack resource probe.

No credential access, device configuration writes or video persistence. The
caller owns peer-UID selection, overall workload lifetime and socket cleanup.
Video byte counts are transport observations, not decoded-frame evidence.
"""

from __future__ import annotations

import json
import re
import socket
import struct
import time
from typing import cast


STATUS_SOCKET = "/run/hardened-blikvm-update-api/status.sock"
VIDEO_SOCKET = "/run/hardened-blikvm-video/mjpeg.sock"
VIDEO_REQUEST = b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n"
MAX_STATUS = 16384
MAX_HEADERS = 8192
REQUEST_TIMEOUT = 5.0


def require(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def deadline_timeout(stream: socket.socket, deadline: float) -> None:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise TimeoutError("IPC deadline exceeded")
    stream.settimeout(remaining)


def receive(stream: socket.socket, size: int, deadline: float) -> bytes:
    deadline_timeout(stream, deadline)
    return stream.recv(size)


def exact(stream: socket.socket, size: int, deadline: float) -> bytes:
    data = bytearray()
    while len(data) < size:
        chunk = receive(stream, size - len(data), deadline)
        require(bool(chunk), "IPC response truncated")
        data.extend(chunk)
    return bytes(data)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, "duplicate status field")
        result[key] = value
    return result


def reject_constant(_value: str) -> object:
    raise ValueError("non-finite status number")


def object_value(value: object) -> dict[str, object]:
    require(isinstance(value, dict), "invalid status object")
    return cast(dict[str, object], value)


def decode_status(data: bytes, request_id: int) -> dict[str, object]:
    # The workload's retained-state validator must check the returned status;
    # this function validates only transport and the closed response envelope.
    value = object_value(json.loads(data.decode("utf-8"), object_pairs_hook=unique_object,
                                   parse_constant=reject_constant))
    require(set(value) == {"schemaVersion", "requestId", "response"},
            "invalid status envelope")
    require(type(value["schemaVersion"]) is int and value["schemaVersion"] == 1
            and type(value["requestId"]) is int and value["requestId"] == request_id,
            "status envelope mismatch")
    response = object_value(value["response"])
    require(set(response) == {"type", "status"}
            and response["type"] == "status",
            "status read unsuccessful")
    return object_value(response["status"])


def read_status() -> dict[str, object]:
    request_id = 1
    payload = b'{"schemaVersion":1,"requestId":1,"request":{"type":"status"}}'
    deadline = time.monotonic() + REQUEST_TIMEOUT
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        deadline_timeout(stream, deadline)
        stream.connect(STATUS_SOCKET)
        deadline_timeout(stream, deadline)
        stream.sendall(struct.pack(">I", len(payload)) + payload)
        # The status worker rejects trailing bytes and waits for request EOF.
        stream.shutdown(socket.SHUT_WR)
        length = struct.unpack(">I", exact(stream, 4, deadline))[0]
        require(0 < length <= MAX_STATUS, "invalid status frame length")
        data = exact(stream, length, deadline)
        require(receive(stream, 1, deadline) == b"", "trailing status response")
    return decode_status(data, request_id)


def validate_video_headers(data: bytes) -> None:
    require(len(data) <= MAX_HEADERS and data.endswith(b"\r\n\r\n")
            and b"\r\n\r\n" not in data[:-4], "invalid video header framing")
    lines = data.decode("ascii").split("\r\n")
    require(lines[0] in ("HTTP/1.0 200 OK", "HTTP/1.1 200 OK"), "video HTTP request unsuccessful")
    headers: dict[str, str] = {}
    for line in lines[1:-2]:
        name, separator, value = line.partition(":")
        require(bool(separator) and re.fullmatch(r"[A-Za-z0-9-]+", name) is not None,
                "invalid video response header")
        name = name.lower()
        require(name not in headers, "duplicate video response header")
        require(all(32 <= ord(char) <= 126 or char == "\t" for char in value), "invalid video header value")
        headers[name] = value.strip()
    require("transfer-encoding" not in headers and "content-length" not in headers,
            "video response is not an unframed live stream")
    require(re.fullmatch(r'multipart/x-mixed-replace;\s*boundary=(?:[A-Za-z0-9_-]{1,70}|"[A-Za-z0-9_-]{1,70}")',
                         headers.get("content-type", ""), flags=re.IGNORECASE) is not None,
            "video response is not multipart MJPEG")


def open_video() -> socket.socket:
    deadline = time.monotonic() + REQUEST_TIMEOUT
    stream = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        deadline_timeout(stream, deadline)
        stream.connect(VIDEO_SOCKET)
        deadline_timeout(stream, deadline)
        stream.sendall(VIDEO_REQUEST)
        # Do NOT shutdown(SHUT_WR): video-proxy treats downstream EOF as cancel.
        headers = bytearray()
        while not headers.endswith(b"\r\n\r\n"):
            require(len(headers) < MAX_HEADERS, "video response headers too large")
            headers.extend(exact(stream, 1, deadline))
        validate_video_headers(bytes(headers))
        return stream
    except BaseException:
        stream.close()
        raise


def drain_video_chunk(stream: socket.socket, deadline: float) -> int:
    """Consume at most 64 KiB without returning or retaining video contents."""
    count = len(receive(stream, 65536, deadline))
    require(count > 0, "video stream ended")
    return count
