#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Watches the product's H.264 encoder on the board while its engine is
switched from outside (docs/design/h264-engine-choice-and-quality-monitor-v1.md,
gate C1): every change of its status word, and the encoder unit's processor
time and memory while each engine encodes. Reads only.

usage: python3 ve_engine_watch.py SECONDS

Every 20 ms for SECONDS the status word and the unit's control group are
read. The result, one JSON object on stdout: the changes of the word with
the seconds since the start, the unit's processor time and wall time under
each word, the most memory it held, and its processes at the start and at
the end (a restart would change them).
"""

import json
import sys
import time

STATUS = "/run/hardened-blikvm-h264/encoder"
GROUP = "/sys/fs/cgroup/system.slice/hardened-blikvm-video-encoder-h264.service"


def read(path):
    try:
        with open(path) as file:
            return file.read()
    except OSError:
        return None


def usage_micros():
    text = read(f"{GROUP}/cpu.stat") or ""
    for line in text.splitlines():
        name, _, value = line.partition(" ")
        if name == "usage_usec":
            return int(value)
    return None


def main(arguments):
    if len(arguments) != 1:
        print(__doc__, file=sys.stderr)
        return 2
    seconds = float(arguments[0])
    began = time.monotonic()
    processes_at_start = (read(f"{GROUP}/cgroup.procs") or "").split()
    word = (read(STATUS) or "").strip() or None
    changes = [{"at": 0.0, "word": word}]
    cpu = {}
    wall = {}
    most_memory = 0
    last_usage = usage_micros()
    last_at = began
    while time.monotonic() - began < seconds:
        time.sleep(0.02)
        now = time.monotonic()
        usage = usage_micros()
        memory = read(f"{GROUP}/memory.current")
        if memory:
            most_memory = max(most_memory, int(memory))
        if usage is not None and last_usage is not None:
            cpu[word] = cpu.get(word, 0) + usage - last_usage
        wall[word] = wall.get(word, 0.0) + now - last_at
        last_usage, last_at = usage, now
        current = (read(STATUS) or "").strip() or None
        if current != word:
            word = current
            changes.append({"at": round(now - began, 3), "word": word})
    result = {
        "seconds": round(time.monotonic() - began, 1),
        "changes": changes,
        "processorPercent": {
            str(key): round(100 * cpu.get(key, 0) / 1e6 / wall[key], 2) for key in wall if wall[key] > 0
        },
        "wallSeconds": {str(key): round(value, 1) for key, value in wall.items()},
        "mostMemoryBytes": most_memory,
        "peakMemoryBytes": int((read(f"{GROUP}/memory.peak") or "0").strip() or 0),
        "processesAtStart": processes_at_start,
        "processesAtEnd": (read(f"{GROUP}/cgroup.procs") or "").split(),
    }
    print(json.dumps(result))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
