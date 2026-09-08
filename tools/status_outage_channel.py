"""Canonical bounded public phase messages for an inherited Unix socket."""

import json
import socket
from typing import cast


class PhaseChannel:
    def __init__(self, connection: socket.socket) -> None:
        self.connection = connection
        self.next_id = 1
        self.restores = 0
        self.pending: tuple[int, str] | None = None

    def receive(self) -> tuple[int, str] | None:
        if self.pending is not None:
            raise ValueError("phase acknowledgement still pending")
        raw = bytearray()
        while b"\n" not in raw:
            chunk = self.connection.recv(257 - len(raw))
            if not chunk:
                if raw:
                    raise ValueError("truncated phase request")
                return None
            raw.extend(chunk)
            if len(raw) > 256:
                raise ValueError("oversize phase request")
        value: object = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError("invalid phase request")
        item = cast(dict[str, object], value)
        phase = item.get("phase")
        if (phase not in ("stop", "restore") or type(item.get("id")) is not int
                or item["id"] != self.next_id):
            raise ValueError("phase request order mismatch")
        phase = cast(str, phase)
        expected = json.dumps({"v": 1, "id": self.next_id, "phase": phase}, separators=(",", ":")).encode() + b"\n"
        # Byte equality also rejects duplicate/unknown fields and noncanonical
        # numbers, whitespace, reordered keys or multiple coalesced frames.
        if bytes(raw) != expected:
            raise ValueError("noncanonical phase request")
        if phase == "stop":
            if self.next_id != 1:
                raise ValueError("duplicate outage stop")
        else:
            if self.next_id == 1 or self.restores >= 2:
                raise ValueError("outage restore order mismatch")
            self.restores += 1
        request_id = self.next_id
        self.next_id += 1
        self.pending = request_id, phase
        return self.pending

    def reply(self, request_id: int, phase: str, accepted: bool) -> None:
        if (type(request_id) is not int or (request_id, phase) != self.pending
                or phase not in ("stop", "restore") or type(accepted) is not bool):
            raise ValueError("invalid phase acknowledgement")
        self.connection.sendall(json.dumps({"v": 1, "id": request_id, "phase": phase, "ok": accepted},
                                           separators=(",", ":")).encode() + b"\n")
        self.pending = None
