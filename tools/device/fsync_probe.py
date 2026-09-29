#!/usr/bin/env python3
"""Cost of one HID ledger store on the card and on tmpfs (review R9).

Runs on the board (copy it there, run it, delete it). One store is what
`HidReleaseLedger::store` does: a temporary file in the directory, fsync,
rename over the ledger, fsync of the directory. The HID lease is not taken and
nothing but small scratch files is written; they are removed at the end.

A key or button event stores twice (before and after the change), so its
cost is two stores.

Usage: fsync_probe.py COUNT DIR [DIR ...]
"""
import json
import os
import statistics
import sys
import tempfile
import time

count = int(sys.argv[1])
directories = sys.argv[2:]


def store(directory, payload, sync=True):
    fd, temporary = tempfile.mkstemp(dir=directory, prefix=".ledger-")
    with os.fdopen(fd, "wb") as handle:
        handle.write(payload)
        handle.flush()
        if sync:
            os.fsync(handle.fileno())
    os.rename(temporary, os.path.join(directory, "hid-ledger.json"))
    if sync:
        directory_fd = os.open(directory, os.O_RDONLY)
        try:
            os.fsync(directory_fd)
        finally:
            os.close(directory_fd)


result = {}
for directory in directories:
    scratch = tempfile.mkdtemp(dir=directory, prefix="fsync-probe-")
    payload = json.dumps({"schema_version": 1, "releases": [{"key": 4}]}).encode()
    for sync in (True, False):
        times = []
        for _ in range(count):
            started = time.monotonic()
            store(scratch, payload, sync)
            times.append((time.monotonic() - started) * 1000)
        times.sort()
        result[f"{directory} {'fsync' if sync else 'no fsync'}"] = {
            "storesMs": {
                "median": round(statistics.median(times), 2),
                "p90": round(times[int(len(times) * 0.9)], 2),
                "max": round(times[-1], 2),
            },
            "perKeyEventMs": round(2 * statistics.median(times), 2),
        }
    for name in os.listdir(scratch):
        os.remove(os.path.join(scratch, name))
    os.rmdir(scratch)
print(json.dumps(result, indent=1))
