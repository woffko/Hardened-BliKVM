"""Run the reviewed native Subscribe worker with hard setup/lifetime bounds."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from typing import BinaryIO, cast

from tools.status_outage_contract import GUARD_SECONDS
from tools.status_restore_monitor import read_record

STARTUP_SECONDS = 5
LIFETIME_SECONDS = GUARD_SECONDS + 30


class SubscriptionProcess:
    def __init__(self, worker_source: str) -> None:
        # Source must be the reviewed in-memory module bundle ending in
        # subscription_worker(), never a value supplied by a device or user API.
        if not worker_source or len(worker_source.encode()) > 100000:
            raise ValueError("invalid subscription worker source size")
        self.closed = False
        self.process = subprocess.Popen(
            ["/usr/bin/timeout", "--signal=TERM", "--kill-after=2s", str(LIFETIME_SECONDS) + "s",
             sys.executable, "-B", "-c", worker_source],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            start_new_session=True, bufsize=0, env={"LC_ALL": "C"},
        )
        try:
            if self.process.stdin is None or self.process.stdout is None:
                raise RuntimeError("subscription worker pipes unavailable")
            marker = read_record(cast(BinaryIO, self.process.stdout), time.monotonic() + STARTUP_SECONDS, 32)
            if marker != b"SUBSCRIBED\n":
                raise RuntimeError("subscription worker did not confirm readiness")
            self.ensure_alive()
        except BaseException:
            self.close()
            raise

    def ensure_alive(self) -> None:
        if self.closed or self.process.poll() is not None:
            raise RuntimeError("subscription worker is not alive")

    def close(self) -> None:
        if self.closed:
            return
        self.closed = True
        try:
            if self.process.stdin is not None:
                self.process.stdin.close()
            try:
                self.process.wait(timeout=1)
            except subprocess.TimeoutExpired:
                for sig in (signal.SIGTERM, signal.SIGKILL):
                    try:
                        os.killpg(self.process.pid, sig)
                    except ProcessLookupError:
                        pass
                    try:
                        self.process.wait(timeout=2)
                        break
                    except subprocess.TimeoutExpired:
                        if sig == signal.SIGKILL:
                            raise
        finally:
            if self.process.stdout is not None:
                self.process.stdout.close()

    def __enter__(self) -> SubscriptionProcess:
        return self

    def __exit__(self, *_exception: object) -> None:
        self.close()
