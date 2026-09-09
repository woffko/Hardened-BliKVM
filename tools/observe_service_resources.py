#!/usr/bin/env python3
"""Fixed, read-only resource sampling for the retained sequence-five stack.

Runs on the test device through SSH; no configuration, service or workload writes.
CPU percent is in single-core units (100% means one fully occupied core).
"""

from __future__ import annotations

import hashlib
import itertools
import json
import os
from pathlib import Path
import re
import subprocess
import time
from typing import NamedTuple, TypedDict


SAMPLES = 61
INTERVAL = 10
SLOT = "app-00000000000000000005-41cb7620d36a47c3416e748296fa1512fc59aaf9012f12f04c0f659098629ebc"
UNITS = (
    "hardened-blikvm-server-current.service",
    "hardened-blikvm-platform-broker.service",
    "hardened-blikvm-video-proxy.service",
    "hardened-blikvm-nginx.service",
)


class StatusProfile(NamedTuple):
    slot: str
    version: str
    sequence: int
    generation: int
    previous_slot: str | None
    previous_version: str | None


def status_profile(name: str = "seq5") -> StatusProfile:
    """Exact accepted baselines; unknown names never select a fallback."""
    if name == "seq5":
        return StatusProfile(SLOT, "0.1.0-dev4", 5, 22, None, None)
    if name == "seq6":
        return StatusProfile(
            "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac",
            "0.1.0-dev5", 6, 31, SLOT, "0.1.0-dev4",
        )
    raise ValueError("unknown status profile")


class UnitSample(TypedDict):
    mainPid: int
    mainStartTicks: int
    restarts: int
    cpuUsec: int
    rssKiB: int
    fileDescriptors: int
    threads: int
    processes: int


class Sample(TypedDict):
    monotonicNs: int
    bootIdSha256: str
    units: dict[str, UnitSample]


def require(condition: bool, reason: str) -> None:
    if not condition:
        raise ValueError(reason)


def natural(value: str) -> int:
    require(re.fullmatch(r"[0-9]{1,20}", value) is not None, "invalid counter")
    return int(value)


def read_bounded(path: Path, limit: int = 65536) -> str:
    with path.open("rb") as stream:
        raw = stream.read(limit + 1)
    require(len(raw) <= limit, "oversize resource input")
    return raw.decode("ascii")


def properties(unit: str) -> dict[str, str]:
    result = subprocess.run(
        ["systemctl", "show", unit,
         "--property=ActiveState,MainPID,NRestarts,ControlGroup"],
        capture_output=True, check=True, timeout=5,
    )
    require(len(result.stdout) <= 4096, "oversize unit properties")
    values = dict(line.split("=", 1) for line in result.stdout.decode("ascii").splitlines())
    require(set(values) == {"ActiveState", "MainPID", "NRestarts", "ControlGroup"}, "unit properties missing")
    require(values["ActiveState"] == "active", "service not active")
    return values


def start_ticks(value: str) -> int:
    # comm may contain spaces and parentheses; field 22 follows its final ')'.
    fields = value.rsplit(")", 1)[-1].split()
    require(len(fields) >= 20, "invalid process stat")
    return natural(fields[19])


def fd_count(path: Path) -> int:
    with os.scandir(path) as entries:
        count = sum(1 for _ in itertools.islice(entries, 4097))
    require(count <= 4096, "descriptor observation limit")
    return count


def unit_sample(unit: str) -> UnitSample:
    before = properties(unit)
    group = before["ControlGroup"]
    require(group == "/system.slice/" + unit, "unexpected service cgroup")
    root = Path("/sys/fs/cgroup") / group.lstrip("/")
    require(not any(item.is_dir() for item in root.iterdir()), "nested service cgroup")
    pids = sorted(natural(item) for item in read_bounded(root / "cgroup.procs").splitlines())
    main = natural(before["MainPID"])
    require(0 < len(pids) <= 128 and main > 0 and main in pids, "invalid cgroup membership")
    cpu = dict(line.split() for line in read_bounded(root / "cpu.stat").splitlines())
    rss = threads = descriptors = 0
    identities: dict[int, int] = {}
    for pid in pids:
        proc = Path("/proc") / str(pid)
        identities[pid] = start_ticks(read_bounded(proc / "stat"))
        status = dict(line.split(":", 1) for line in read_bounded(proc / "status").splitlines())
        memory = status["VmRSS"].split()
        require(len(memory) == 2 and memory[1] == "kB", "invalid RSS unit")
        rss += natural(memory[0])
        threads += natural(status["Threads"].strip())
        descriptors += fd_count(proc / "fd")
    require(properties(unit) == before, "unit changed during sample")
    require(sorted(natural(item) for item in read_bounded(root / "cgroup.procs").splitlines()) == pids,
            "membership changed during sample")
    for pid, identity in identities.items():
        require(start_ticks(read_bounded(Path("/proc") / str(pid) / "stat")) == identity,
                "process changed during sample")
    return {"mainPid": main, "mainStartTicks": identities[main],
            "restarts": natural(before["NRestarts"]), "cpuUsec": natural(cpu["usage_usec"]),
            "rssKiB": rss, "threads": threads, "fileDescriptors": descriptors, "processes": len(pids)}


def snapshot(profile_name: str = "seq5") -> Sample:
    slot = status_profile(profile_name).slot
    boot = read_bounded(Path("/proc/sys/kernel/random/boot_id"), 64).strip()
    require(re.fullmatch(r"[a-f0-9-]{36}", boot) is not None, "invalid boot identifier")
    slot_root = Path("/var/lib/hardened-blikvm-slots")
    require((slot_root / "current").resolve(strict=True) == slot_root / slot,
            "unexpected selected slot")
    units = {unit: unit_sample(unit) for unit in UNITS}
    require((Path("/proc") / str(units[UNITS[0]]["mainPid"]) / "exe").resolve(strict=True)
            == slot_root / slot / "bin/bli-kvm-server", "unexpected server executable")
    require((slot_root / "current").resolve(strict=True) == slot_root / slot,
            "selected slot changed during sample")
    require(read_bounded(Path("/proc/sys/kernel/random/boot_id"), 64).strip() == boot, "boot changed during sample")
    return {"monotonicNs": time.monotonic_ns(),
            "bootIdSha256": hashlib.sha256(boot.encode()).hexdigest(), "units": units}


def summarize(samples: list[Sample]) -> dict[str, object]:
    require(len(samples) == SAMPLES, "incomplete sample sequence")
    first = samples[0]
    elapsed = samples[-1]["monotonicNs"] - first["monotonicNs"]
    require(elapsed >= (SAMPLES - 1) * INTERVAL * 1_000_000_000, "observation too short")
    peaks: dict[str, dict[str, float | int]] = {}
    for unit in UNITS:
        initial = first["units"][unit]
        values = [sample["units"][unit] for sample in samples]
        for sample, value in zip(samples, values):
            require(sample["bootIdSha256"] == first["bootIdSha256"], "boot changed")
            for key in ("mainPid", "mainStartTicks", "restarts"):
                require(value[key] == initial[key], "service identity changed")
        percents: list[float] = []
        for index in range(1, len(samples)):
            ns = samples[index]["monotonicNs"] - samples[index - 1]["monotonicNs"]
            cpu = values[index]["cpuUsec"] - values[index - 1]["cpuUsec"]
            require(ns > 0 and cpu >= 0, "resource clock regressed")
            percents.append(cpu * 100_000 / ns)
        peaks[unit] = {
            "cpuMeanCorePercent": round((values[-1]["cpuUsec"] - initial["cpuUsec"]) * 100_000 / elapsed, 3),
            "cpuMaxIntervalCorePercent": round(max(percents), 3),
            "rssMaxKiB": max(value["rssKiB"] for value in values),
            "rssGrowthKiB": values[-1]["rssKiB"] - initial["rssKiB"],
            "fdMax": max(value["fileDescriptors"] for value in values),
            "fdGrowth": values[-1]["fileDescriptors"] - initial["fileDescriptors"],
            "threadsMax": max(value["threads"] for value in values),
            "processesMax": max(value["processes"] for value in values),
        }
    return {"samples": len(samples), "elapsedSeconds": round(elapsed / 1_000_000_000, 3),
            "bootAndMainProcessesStable": True, "units": peaks}


def main() -> None:
    samples: list[Sample] = []
    try:
        require(os.geteuid() == 0, "observer requires root")
        require(os.uname().machine == "aarch64", "unexpected architecture")
        for index in range(SAMPLES):
            if index:
                time.sleep(INTERVAL)
            samples.append(snapshot())
            # Fail early for changed boot, main process, restart or CPU counter.
            if index:
                require(samples[-1]["bootIdSha256"] == samples[0]["bootIdSha256"], "boot changed")
                for unit in UNITS:
                    value, initial = samples[-1]["units"][unit], samples[0]["units"][unit]
                    require(all(value[key] == initial[key] for key in ("mainPid", "mainStartTicks", "restarts")),
                            "service identity changed")
                    require(value["cpuUsec"] >= samples[-2]["units"][unit]["cpuUsec"], "resource clock regressed")
        result = {"result": "observed", "workload": "no-observer-generated-workload",
                  "summary": summarize(samples), "samples": samples}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        # No raw process, command, path or exception text leaves the device.
        result = {"result": "failed", "completedSamples": len(samples), "errorType": type(error).__name__}
    print(json.dumps(result, sort_keys=True))
    if result["result"] != "observed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
