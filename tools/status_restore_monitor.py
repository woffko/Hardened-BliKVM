"""Finite, exact-match busctl monitor; never starts or stops a systemd unit."""

from __future__ import annotations

import os
import select
import signal
import subprocess
import time
from typing import BinaryIO, cast

from tools.status_restore_event import MAX_EVENT_BYTES, monitor_command, parse_completed_job
from tools.status_outage_contract import GUARD_SECONDS
from tools.systemd_property import uint

LIFETIME_SECONDS = GUARD_SECONDS + 30
READY = b"Monitoring bus message stream.\n"


def read_record(stream: BinaryIO, deadline: float, limit: int) -> bytes:
    value = bytearray()
    while not value.endswith(b"\n"):
        remaining = deadline - time.monotonic()
        if remaining <= 0 or not select.select([stream], [], [], remaining)[0]:
            raise TimeoutError("restore monitor deadline exceeded")
        chunk = os.read(stream.fileno(), min(4096, limit + 1 - len(value)))
        if not chunk:
            raise ValueError("restore monitor ended before a complete record")
        value.extend(chunk)
        if len(value) > limit:
            raise ValueError("restore monitor record too large")
    return bytes(value)


class RestoreMonitor:
    def __init__(self, operation: str) -> None:
        self.operation = operation
        self.closed = False
        # GNU timeout also bounds the child if the Python observer disappears.
        # A new session confines all cleanup signals to this owned monitor.
        self.process = subprocess.Popen(
            ["/usr/bin/timeout", "--signal=TERM", "--kill-after=2s", str(LIFETIME_SECONDS) + "s",
             *monitor_command(operation)],
            stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            bufsize=0, start_new_session=True,
            env={"LC_ALL": "C", "SYSTEMD_LOG_LEVEL": "info", "SYSTEMD_LOG_TARGET": "console", "SYSTEMD_COLORS": "0"},
        )
        try:
            if self.process.stdout is None or self.process.stderr is None:
                raise ValueError("restore monitor pipes unavailable")
            if read_record(cast(BinaryIO, self.process.stderr), time.monotonic() + 5, 4096) != READY:
                raise ValueError("restore monitor registration not confirmed")
        except BaseException:
            self.close()
            raise

    def completed(self, expected_sender: str, not_before_usec: int) -> dict[str, object]:
        deadline_usec = uint(not_before_usec, 64)
        now = time.monotonic_ns() // 1000
        if self.closed or self.process.poll() is not None or not now < deadline_usec <= now + GUARD_SECONDS * 1_000_000:
            raise ValueError("restore observation started outside guard window")
        raw = read_record(cast(BinaryIO, self.process.stdout), (deadline_usec + 25_000_000) / 1_000_000, MAX_EVENT_BYTES)
        received = time.monotonic_ns() // 1000
        if not deadline_usec <= received <= deadline_usec + 25_000_000:
            raise ValueError("restore job arrived outside guard window")
        event = parse_completed_job(raw, self.operation, expected_sender)
        return {**event, "receivedMonotonicUsec": received, "timingSource": "observer-receive-clock"}

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            if self.process.poll() is None:
                try:
                    os.killpg(self.process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    self.process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    try:
                        os.killpg(self.process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    self.process.wait(timeout=3)
        finally:
            if self.process.stdout is not None:
                self.process.stdout.close()
            if self.process.stderr is not None:
                self.process.stderr.close()

    def __enter__(self) -> RestoreMonitor:
        return self

    def __exit__(self, *_exception: object) -> None:
        self.close()
