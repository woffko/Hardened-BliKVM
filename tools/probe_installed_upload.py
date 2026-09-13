#!/usr/bin/env python3
"""Fixed stage-only IPC probes; never select or activate an application slot."""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import select
import socket
import stat
import struct
import sys
import tarfile
import time
from typing import cast

ENDPOINT = "/run/hardened-blikvm-update-api/upload.sock"
BUNDLE_SHA = "ad36542bf330440717bf5b2ac0279258dfbd98e32834a6cf2a47f39722045a0f"
BUNDLE_BYTES = 8724480
MANIFEST_SHA = "f2262e8f9929dda83b2baf1005b5ae9262aa075b7ddaa16f4ad6200b5daf0140"
SLOT = "app-00000000000000000007-" + MANIFEST_SHA
MODES = ("publish", "retry", "invalid-archive", "short", "surplus", "malformed", "lost-reply", "unauthorized", "idle")


@dataclass(frozen=True)
class UploadProfile:
    name: str
    bundle_sha: str
    manifest_sha: str
    sequence: int
    publish: bool
    version: str = '0.1.0-dev6'
    payload_bytes: int = 8683648

    @property
    def slot(self) -> str:
        return f"app-{self.sequence:020d}-{self.manifest_sha}"


PROFILES = {
    "seq7-retry": UploadProfile("seq7-retry", BUNDLE_SHA, MANIFEST_SHA, 7, False),
    "seq8-publish": UploadProfile("seq8-publish",
        "d2acf270fa2aa9cb924c362066ece50f4b0fbd4c4c8b2975648dab5ac0b4f849",
        "375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b", 8, True),
    "seq9-retry": UploadProfile("seq9-retry",
        "8f858ec697928f39bcfc6a7242aa2d1fdbb061b8cf9232048d9468d03dff247f",
        "5f1aee4802addd6c0c0b444224345624cd8b9b8c46950a2231ad7e2b42de37c2", 9, False),
    "seq10-retry": UploadProfile("seq10-retry",
        "85e06d1b68e54480bab89c3cdc5e0d9f80cc3ee007e7ba1e425432af4b250f1f",
        "8a30162d0b620c2f7a6679061d2c9c4bf863aa8ac1be2d2e6dfa77caacc6ae01", 10, False, '0.1.0-dev7', 8683976),
}
DEFAULT_PROFILE = PROFILES["seq7-retry"]


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    result: dict[str, object] = {}
    for key, value in pairs:
        require(key not in result, "duplicate response field")
        result[key] = value
    return result


def frame(length: int) -> bytes:
    require(type(length) is int and 1 <= length <= 128 * 1024 * 1024, "invalid declared size")
    value = {"schemaVersion": 1, "requestId": 1, "request": {"type": "stage", "archiveBytes": length}}
    raw = json.dumps(value, separators=(",", ":")).encode()
    return struct.pack(">I", len(raw)) + raw


def receive(stream: socket.socket) -> dict[str, object] | None:
    def exact(size: int) -> bytes:
        value = bytearray()
        while len(value) < size:
            try:
                chunk = stream.recv(size - len(value))
            except ConnectionResetError:
                chunk = b""
            if not chunk:
                break
            value.extend(chunk)
        return bytes(value)
    prefix = exact(4)
    if not prefix:
        return None
    require(len(prefix) == 4, "partial response prefix")
    size = struct.unpack(">I", prefix)[0]
    require(1 <= size <= 16384, "oversize response")
    raw = exact(size)
    require(len(raw) == size and not exact(1), "partial or surplus response")
    decoded: object = json.loads(raw, object_pairs_hook=unique_object)
    require(isinstance(decoded, dict), "invalid response envelope")
    value = cast(dict[str, object], decoded)
    require(set(value) == {"schemaVersion", "requestId", "response"}, "invalid response envelope")
    require(type(value["schemaVersion"]) is int and value["schemaVersion"] == 1
            and type(value["requestId"]) is int and value["requestId"] == 1, "uncorrelated response")
    response = value["response"]
    require(isinstance(response, dict), "invalid response object")
    return cast(dict[str, object], response)


def validate_retry(response: dict[str, object] | None, profile: UploadProfile = DEFAULT_PROFILE, *, publish: bool = False) -> None:
    require(not publish or profile.publish, "profile does not permit publication")
    expected = {"type": "staged", "receipt": {
        "slot": {"slotId": profile.slot, "version": profile.version, "sequence": profile.sequence, "minimumSystemSequence": 1},
        "disposition": "published" if publish else "already_present", "manifestSha256": profile.manifest_sha, "payloadBytes": profile.payload_bytes}}
    # Canonical JSON also distinguishes booleans from integer counters.
    require(json.dumps(response, sort_keys=True) == json.dumps(expected, sort_keys=True), "unexpected staging receipt")


def tamper_archive(archive: bytes) -> bytes:
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as source:
        member = source.getmember("payload/share/web/index.html")
        require(member.isfile() and member.size > 0, "signed payload entry missing")
        position = member.offset_data
    altered = bytearray(archive)
    altered[position] ^= 1
    return bytes(altered)


def await_release(stream: socket.socket, body: bytes) -> bytes:
    require(len(body) > 256, "held body too small")
    stream.sendall(body[:1])
    position = 1
    print("UPLOAD_BODY_HELD", flush=True)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if select.select([sys.stdin.buffer], [], [], 0.25)[0]:
            require(sys.stdin.buffer.read(1) == b"G", "invalid probe release")
            return body[position:]
        # Preserve the worker idle deadline without completing the archive.
        stream.sendall(body[position:position + 1])
        position += 1
    raise TimeoutError("upload observer did not release body")


def exchange(stream: socket.socket, archive: bytes, mode: str, *, hold: bool = False,
             profile: UploadProfile = DEFAULT_PROFILE) -> dict[str, object]:
    require(mode in MODES, "unknown probe mode")
    require(mode != "publish" or profile.publish, "profile does not permit publication")
    if mode == "malformed":
        wire, body = struct.pack(">I", 1) + b"{", b""
    elif mode == "unauthorized":
        wire, body = frame(1), b"x"
    else:
        body = tamper_archive(archive) if mode == "invalid-archive" else archive
        wire = frame(len(body))
        if mode == "short":
            body = body[:-1]
        elif mode == "surplus":
            body += b"x"
    try:
        stream.sendall(wire)
        if hold:
            require(mode not in ("malformed", "unauthorized"), "cannot hold early rejection")
            body = await_release(stream, body)
        if mode == "idle":
            if not hold:
                stream.sendall(body[:1])
        else:
            stream.sendall(body)
            stream.shutdown(socket.SHUT_WR)
    except (BrokenPipeError, ConnectionResetError):
        require(mode in ("malformed", "unauthorized"), "worker disconnected during body")
    if mode == "lost-reply":
        # A send is not evidence of publication; the root controller must wait
        # for worker exit and require a subsequent exact idempotent retry.
        return {"mode": mode, "result": "reply-unobserved"}
    started = time.monotonic()
    response = receive(stream)
    if mode == "idle":
        require(response is None and 4 <= time.monotonic() - started <= 12, "worker idle deadline differs")
    elif mode in ("short", "surplus", "malformed", "unauthorized"):
        require(response is None, "invalid transport or peer received a receipt")
    elif mode == "invalid-archive":
        require(response == {"type": "error", "code": "invalid_archive"}, "archive rejection differs")
    else:
        validate_retry(response, profile, publish=mode == "publish")
    return {"mode": mode, "result": "passed"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=MODES)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--hold", action="store_true")
    parser.add_argument("--profile", choices=tuple(PROFILES), default=DEFAULT_PROFILE.name)
    args = parser.parse_args()
    profile = PROFILES[args.profile]
    expected_uid = 0 if args.mode == "unauthorized" else pwd.getpwnam("hardened-blikvm-server").pw_uid
    require(os.geteuid() == expected_uid and (expected_uid > 0 or args.mode == "unauthorized"), "wrong probe identity")
    meta = args.archive.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and not meta.st_mode & 0o022 and meta.st_size == BUNDLE_BYTES, "unsafe bound archive")
    archive = args.archive.read_bytes()
    require(hashlib.sha256(archive).hexdigest() == profile.bundle_sha, "bound archive differs")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
        stream.settimeout(20)
        stream.connect(ENDPOINT)
        result = exchange(stream, archive, args.mode, hold=args.hold, profile=profile)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
