#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Switches an H.264 encoder on the board between the video engine and x264
through its demand socket, as the web server does with the board's choice,
and measures every switch
(docs/design/h264-engine-choice-and-quality-monitor-v1.md, gate C1).

usage: python3 ve_engine_switch.py DEMAND FRAMES STATUS PID SWITCHES PERIOD KBPS OUTPUT

DEMAND, FRAMES and STATUS are the encoder's demand socket, frame socket and
status word, PID its process. The script holds a viewer's demand at KBPS
with the engine chosen, reads every record of the frame socket, and after
PERIOD seconds sends the other choice: SWITCHES times in all, x264 first.
For every switch it notes when the status word named what was chosen, when
the first key frame came after the line, how many records came between, and
the encoder's processor time over the period that followed. The records
must go on one after the other in one stream. The summary is written to
OUTPUT as JSON; the status is 0 where every switch took effect within two
seconds with a key frame within one, and the records never broke.
"""

import json
import os
import socket
import statistics
import struct
import sys
import threading
import time

HEADER = struct.Struct(">BBBBHHIIIIQ")
KEY, DELTA = 1, 2
WARM_UP = 10.0
EFFECT_LIMIT = 2.0
KEY_LIMIT = 1.0


class Records:
    """Every record of the frame socket, with when it came."""

    def __init__(self, path):
        self.connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.connection.connect(path)
        self.seen = []
        self.broken = []
        self.ended = False
        threading.Thread(target=self.read, daemon=True).start()

    def exactly(self, count):
        data = bytearray()
        while len(data) < count:
            chunk = self.connection.recv(count - len(data))
            if not chunk:
                raise EOFError
            data += chunk
        return bytes(data)

    def read(self):
        last = None
        try:
            while True:
                fields = HEADER.unpack(self.exactly(HEADER.size))
                version, kind, codec, _, width, height, stream, frame, reference, size, _ = fields
                came = time.monotonic()
                self.exactly(size)
                if version != 1 or codec != 1 or kind not in (KEY, DELTA):
                    self.broken.append(f"a record of another form at {came:.3f}")
                elif last is not None and (stream != last[0] or frame != last[1] + 1):
                    self.broken.append(
                        f"stream {last[0]} frame {last[1]} then stream {stream} frame {frame}"
                    )
                last = (stream, frame)
                self.seen.append((came, kind, frame, size, width, height))
        except (EOFError, OSError):
            self.ended = True


def processor_seconds(pid):
    with open(f"/proc/{pid}/stat") as stat:
        fields = stat.read().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def word(path):
    try:
        with open(path) as status:
            return status.read().strip()
    except OSError:
        return None


def summary(values):
    if not values:
        return None
    ordered = sorted(values)
    return {
        "median": round(statistics.median(ordered), 3),
        "highest": round(ordered[-1], 3),
        "lowest": round(ordered[0], 3),
    }


def main(arguments):
    if len(arguments) != 8:
        print(__doc__, file=sys.stderr)
        return 2
    demand_path, frames_path, status_path = arguments[:3]
    pid, switches, period, kbps = (int(value) for value in arguments[3:7])
    output = arguments[7]
    demand = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    demand.connect(demand_path)
    demand.sendall(f"h264\nkbps {kbps}\nengine hardware\n".encode("ascii"))
    records = Records(frames_path)
    time.sleep(WARM_UP)
    began = word(status_path)
    rows = []
    for index in range(switches):
        chosen = "software" if index % 2 == 0 else "hardware"
        expected = "x264" if chosen == "software" else "hardware"
        cpu_before = processor_seconds(pid)
        sent = time.monotonic()
        demand.sendall(f"engine {chosen}\n".encode("ascii"))
        effect = None
        while time.monotonic() - sent < 5.0:
            if word(status_path) == expected:
                effect = time.monotonic() - sent
                break
            time.sleep(0.002)
        time.sleep(max(0.0, sent + period - time.monotonic()))
        cpu_after = processor_seconds(pid)
        after = [record for record in records.seen if record[0] >= sent]
        key_at = next((at for at, (came, kind, *_) in enumerate(after) if kind == KEY), None)
        rows.append(
            {
                "chosen": chosen,
                "statusSeconds": None if effect is None else round(effect, 4),
                "keySeconds": None if key_at is None else round(after[key_at][0] - sent, 4),
                "recordsBeforeKey": key_at,
                "records": len(after),
                "cores": round((cpu_after - cpu_before) / (time.monotonic() - sent), 3),
            }
        )
    demand.close()
    time.sleep(1.0)
    by_choice = {}
    for chosen in ("software", "hardware"):
        mine = [row for row in rows if row["chosen"] == chosen]
        by_choice[chosen] = {
            "switches": len(mine),
            "statusSeconds": summary([row["statusSeconds"] for row in mine if row["statusSeconds"] is not None]),
            "keySeconds": summary([row["keySeconds"] for row in mine if row["keySeconds"] is not None]),
            "recordsBeforeKey": summary([row["recordsBeforeKey"] for row in mine if row["recordsBeforeKey"] is not None]),
            "framesPerSecond": summary([row["records"] / period for row in mine]),
            "cores": summary([row["cores"] for row in mine]),
            "missed": sum(
                1
                for row in mine
                if row["statusSeconds"] is None
                or row["statusSeconds"] > EFFECT_LIMIT
                or row["keySeconds"] is None
                or row["keySeconds"] > KEY_LIMIT
            ),
        }
    sizes = sorted({(width, height) for *_, width, height in records.seen})
    result = {
        "switches": len(rows),
        "periodSeconds": period,
        "kbps": kbps,
        "statusAtStart": began,
        "records": len(records.seen),
        "keyRecords": sum(1 for record in records.seen if record[1] == KEY),
        "sizes": [f"{width}x{height}" for width, height in sizes],
        "recordsBroken": records.broken[:20],
        "frameSocketEnded": records.ended,
        "byChoice": by_choice,
        "rows": rows,
    }
    passed = (
        len(rows) == switches
        and not records.broken
        and all(choice["missed"] == 0 for choice in by_choice.values())
    )
    result["passed"] = passed
    with open(output, "w") as out:
        json.dump(result, out, indent=1)
    print(json.dumps({key: result[key] for key in ("switches", "records", "keyRecords", "recordsBroken", "byChoice", "passed")}))
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
