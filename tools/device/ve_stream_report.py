#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Reads what `ve_stream_run.sh` left of a lab session and prints its table.

usage: ve_stream_report.py RESULTS_DIR [--markdown] [--endurance]

For every run: the frames, the rate (mean, and the lowest and highest of
all windows of ten seconds), the engine's and the reference's mean luma
PSNR, the time of an encode and of a whole frame, second passes, frames
left out, where the search was chosen for every frame the frames encoded
with both searches and those sent with the wide one, the frames that took
three passes, and for the drift check the mean PSNR of frames 30 to 60
against the last 30.

With --endurance, what a long run is judged by instead: its length, the
frames a second it held and the frames that began late, the time of a
whole frame and the frames that took longer than the time between two, the
rate and its windows, the frames that took two passes and three, of the
frames that were decoded and
compared the lowest PSNR and the number below the run's floor, and of a
stream that repeats itself the frames unlike the frame a period before.
"""

import json
import sys
from pathlib import Path


def psnr_lines(path):
    found = {}
    if path.exists():
        for line in path.read_text().splitlines():
            name, _, rest = line.partition(": ")
            if rest.startswith("{"):
                try:
                    found[name] = json.loads(rest)
                except json.JSONDecodeError:
                    pass
            elif name.startswith("ffmpeg complaints"):
                found["complaints"] = rest
    return found


def mean(values):
    numbers = [value for value in values if isinstance(value, (int, float))]
    return sum(numbers) / len(numbers) if numbers else None


def show(value, digits=2):
    return "-" if value is None else f"{value:.{digits}f}"


def count(value):
    return "-" if value is None else str(value)


def rows(directory):
    for receipt in sorted(Path(directory).glob("*.json")):
        try:
            run = json.loads(receipt.read_text())
        except json.JSONDecodeError:
            continue
        if run.get("experiment") != "stream":
            continue
        psnr = psnr_lines(receipt.with_suffix(".psnr.txt"))
        engine = psnr.get("engine", {})
        reference = psnr.get("x264", {})
        reference_run = psnr.get("x264 run", {})
        # A long run has no list of every frame's value.
        db = engine.get("db") or []
        windows = run.get("kbpsTenSecondWindows") or {}
        rate = run["rate"]
        yield {
            "run": receipt.stem,
            "sequence": run["sequence"],
            "size": f"{run['width']}x{run['height']}",
            "rate": f"qp{rate['qp']}" if "qp" in rate else f"{rate['kbps']}",
            "frames": f"{run['framesDone']}/{run['framesAsked']}",
            "decoded": engine.get("frames"),
            "complaints": psnr.get("complaints", "-"),
            "kbps": run["kbpsMean"],
            "window_low": windows.get("lowest"),
            "window_high": windows.get("highest"),
            "engine_db": engine.get("meanDb"),
            "engine_lowest_db": engine.get("lowestDb"),
            "x264_db": reference.get("meanDb"),
            "x264_kbps": reference_run.get("kbpsMean"),
            "x264_ms": (reference_run.get("encodeMicrosMean") or 0) / 1000 or None,
            "encode_ms": (run["encodeMicros"] or {}).get("mean", 0) / 1000,
            "encode_max_ms": (run["encodeMicros"] or {}).get("max", 0) / 1000,
            "frame_ms": (run["frameMicros"] or {}).get("mean", 0) / 1000,
            "fps": run["framesPerSecond"],
            "second_passes": len(run["secondPasses"]),
            "left_out": run.get("framesLeftOut", 0),
            "overflows": run.get("overflows"),
            "late": run.get("lateFrames"),
            "seconds": run.get("seconds"),
            "frames_done": run["framesDone"],
            "frame_p99_ms": (run["frameMicros"] or {}).get("p99", 0) / 1000,
            "frame_max_ms": (run["frameMicros"] or {}).get("max", 0) / 1000,
            "windows": windows.get("count"),
            "resets": run.get("resetsAfterTimeout"),
            "floor": engine.get("floorDb"),
            "below_floor": engine.get("belowFloor"),
            "slow": run.get("framesLongerThanAnInterval"),
            "period": (run.get("repeat") or run).get("period"),
            "unlike": (run.get("repeat") or run).get("framesUnlikeAPeriodBefore"),
            "search": run.get("search") or {},
            "passes": run.get("passes") or {},
            "early_db": mean(db[30:60]),
            "late_db": mean(db[-30:]) if len(db) >= 90 else None,
            "error": run.get("error"),
        }


def endurance(row):
    return [
        row["run"],
        str(row["frames_done"]),
        show(row["seconds"], 0),
        show(row["fps"], 2),
        "-" if row["late"] is None else str(row["late"]),
        show(row["frame_ms"], 1),
        show(row["frame_p99_ms"], 1),
        show(row["frame_max_ms"], 1),
        "-" if row["slow"] is None else str(row["slow"]),
        str(row["kbps"]),
        show(row["window_low"], 0),
        show(row["window_high"], 0),
        "-" if row["windows"] is None else str(row["windows"]),
        str(row["second_passes"]),
        str(row["left_out"]),
        count(row["search"].get("framesWithBoth")),
        count(row["passes"].get("framesWithTwo")),
        count(row["passes"].get("framesWithThree")),
        "-" if row["decoded"] is None else f"{row['decoded']}, {row['complaints']}",
        show(row["engine_db"]),
        show(row["engine_lowest_db"]),
        "-" if row["below_floor"] is None else f"{row['below_floor']} below {row['floor']:g}",
        "-" if row["unlike"] is None else f"{row['unlike']} unlike {row['period']} before",
    ]


ENDURANCE_HEADER = [
    "run", "frames", "seconds", "frames/s", "late", "frame ms", "p99", "max", "over 1/fps", "kbit/s",
    "10 s low", "10 s high", "windows", "2nd", "left out", "both", "2 passes", "3 passes",
    "decoded, ffmpeg",
    "mean dB", "lowest dB", "wrong frames", "repeats",
]


def print_table(header, table, markdown):
    if markdown:
        print("| " + " | ".join(header) + " |")
        print("|" + " --- |" * len(header))
        for line in table:
            print("| " + " | ".join(line) + " |")
        return
    widths = [
        max(len(header[i]), *(len(line[i]) for line in table)) if table else len(header[i])
        for i in range(len(header))
    ]
    print("  ".join(header[i].ljust(widths[i]) for i in range(len(header))))
    for line in table:
        print("  ".join(line[i].ljust(widths[i]) for i in range(len(header))))


def main(arguments):
    if not arguments or arguments[0].startswith("-"):
        print(__doc__, file=sys.stderr)
        return 2
    markdown = "--markdown" in arguments
    if "--endurance" in arguments:
        table = []
        for row in rows(arguments[0]):
            table.append(endurance(row))
            if row["error"]:
                table.append([f"  error: {row['error']}"] + [""] * (len(ENDURANCE_HEADER) - 1))
        print_table(ENDURANCE_HEADER, table, markdown)
        return 0
    header = [
        "run", "frames", "ffmpeg", "kbit/s", "10 s low", "10 s high", "engine dB",
        "x264 dB", "diff", "x264 kbit/s", "encode ms", "max", "frame ms", "x264 ms",
        "2nd", "left out", "both", "wide", "3 passes", "30-60 dB", "last 30 dB",
    ]
    table = []
    for row in rows(arguments[0]):
        difference = (
            row["engine_db"] - row["x264_db"]
            if row["engine_db"] is not None and row["x264_db"] is not None
            else None
        )
        table.append([
            row["run"],
            row["frames"],
            f"{row['decoded']}, {row['complaints']}" if row["decoded"] is not None else "-",
            str(row["kbps"]),
            show(row["window_low"], 0),
            show(row["window_high"], 0),
            show(row["engine_db"]),
            show(row["x264_db"]),
            show(difference),
            show(row["x264_kbps"], 0),
            show(row["encode_ms"], 1),
            show(row["encode_max_ms"], 1),
            show(row["frame_ms"], 1),
            show(row["x264_ms"], 1),
            str(row["second_passes"]),
            str(row["left_out"]),
            count(row["search"].get("framesWithBoth")),
            count(row["search"].get("framesWithTheWide")),
            count(row["passes"].get("framesWithThree")),
            show(row["early_db"]),
            show(row["late_db"]),
        ])
        if row["error"]:
            table.append([f"  error: {row['error']}"] + [""] * (len(header) - 1))
    print_table(header, table, markdown)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
