"""Fixed backend IPC workload with supervised service-resource observation.

Only the workload child runs as the server UID. No service, configuration or
device file is changed. This is not an HTTPS/browser or decoded-video gate.
"""

from __future__ import annotations

import grp
import json
import os
import pwd
import re
import select
import subprocess
import sys
import threading
import time
from typing import BinaryIO, cast

from tools import observe_service_resources as resources
from tools import retained_load_ipc as ipc

WORKLOAD = "one-mjpeg-ipc-stream-and-61-serial-status-reads"
LIFETIME = 850
MAX_REPLY = 20000
require = ipc.require


def retained_status(value: object, profile_name: str = "seq5") -> dict[str, object]:
    profile = resources.status_profile(profile_name)
    status = ipc.object_value(value)
    require(set(status) == {"generation", "floorSequence", "systemSequence", "selected",
                            "confirmed", "previous", "pending", "lastRollback"}, "invalid status keys")
    for key, expected in (("generation", profile.generation), ("floorSequence", profile.sequence), ("systemSequence", 1)):
        require(type(status[key]) is int and status[key] == expected, "retained status changed")
    require(status["pending"] is None and status["lastRollback"] is None, "update transaction present")
    selected = {"slotId": profile.slot, "version": profile.version, "sequence": profile.sequence, "minimumSystemSequence": 1}
    for key in ("selected", "confirmed"):
        slot = ipc.object_value(status[key])
        require(slot == selected and type(slot["sequence"]) is int
                and type(slot["minimumSystemSequence"]) is int, "unexpected retained application")
    # Previous metadata is captured and must remain byte-for-byte equivalent
    # between observations; no unverified previous version/digest is guessed.
    previous = ipc.object_value(status["previous"])
    require(set(previous) == set(selected) and type(previous["sequence"]) is int
            and previous["sequence"] == profile.sequence - 1 and type(previous["minimumSystemSequence"]) is int
            and previous["minimumSystemSequence"] == 1, "invalid previous application")
    slot_id, version = previous["slotId"], previous["version"]
    require(isinstance(slot_id, str) and re.fullmatch(rf"app-{profile.sequence - 1:020d}-[a-f0-9]{{64}}", slot_id) is not None
            and slot_id[-64:] != "0" * 64, "invalid previous slot")
    require(isinstance(version, str) and re.fullmatch(r"[A-Za-z0-9.+-]{1,64}", version) is not None,
            "invalid previous version")
    if profile.previous_slot is not None:
        require(slot_id == profile.previous_slot and version == profile.previous_version,
                "unexpected previous application")
    return status


class VideoReader:
    def __init__(self) -> None:
        self.stream = ipc.open_video()
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.count = 0
        self.failed = False
        self.last_byte = time.monotonic()
        self.deadline = self.last_byte + LIFETIME
        self.thread = threading.Thread(target=self.run)
        self.thread.start()

    def run(self) -> None:
        try:
            while not self.stop.is_set():
                require(time.monotonic() < self.deadline, "video lifetime exceeded")
                ready, _, _ = select.select([self.stream], [], [], 0.25)
                if ready:
                    count = ipc.drain_video_chunk(self.stream, time.monotonic() + 1)
                    with self.lock:
                        self.count += count
                        self.last_byte = time.monotonic()
                with self.lock:
                    require(time.monotonic() - self.last_byte < 15, "video stalled")
        except (OSError, ValueError):
            with self.lock:
                self.failed = True

    def sample(self) -> int:
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            with self.lock:
                require(not self.failed, "video reader failed")
                if self.count:
                    require(time.monotonic() - self.last_byte < 15, "video stalled")
                    return self.count
            time.sleep(0.01)
        raise TimeoutError("video payload missing")

    def close(self) -> None:
        self.stop.set()
        self.thread.join(2)
        self.stream.close()
        require(not self.thread.is_alive(), "video reader did not stop")


def worker() -> None:
    require(os.geteuid() == pwd.getpwnam("hardened-blikvm-server").pw_uid != 0,
            "workload requires server UID")
    require(os.getegid() == grp.getgrnam("hardened-blikvm").gr_gid and os.getgroups() == [],
            "unexpected workload groups")
    video: VideoReader | None = None
    result: dict[str, object]
    try:
        video = VideoReader()
        deadline = time.monotonic() + LIFETIME
        requests = 0
        while True:
            require(time.monotonic() < deadline, "workload lifetime exceeded")
            ready, _, _ = select.select([0], [], [], 1)
            if not ready:
                continue
            command = os.read(0, 16)
            if command in (b"", b"stop\n"):
                break
            require(command == b"sample\n" and requests < resources.SAMPLES, "invalid workload command")
            started = time.monotonic_ns()
            status = retained_status(ipc.read_status())
            elapsed = time.monotonic_ns() - started
            requests += 1
            result = {"result": "sample", "request": requests, "status": status,
                      "statusDurationNs": elapsed, "videoBytes": video.sample()}
            print(json.dumps(result, sort_keys=True), flush=True)
    except (OSError, ValueError, KeyError) as error:
        print(json.dumps({"result": "failed", "errorType": type(error).__name__}), flush=True)
        raise SystemExit(1) from None
    finally:
        if video is not None:
            video.close()


def read_reply(stream: BinaryIO) -> dict[str, object]:
    deadline = time.monotonic() + 15
    data = bytearray()
    while not data.endswith(b"\n"):
        remaining = deadline - time.monotonic()
        require(remaining > 0, "workload reply timed out")
        ready, _, _ = select.select([stream], [], [], remaining)
        require(bool(ready), "workload reply timed out")
        chunk = os.read(stream.fileno(), MAX_REPLY + 1 - len(data))
        require(bool(chunk), "workload child ended")
        data.extend(chunk)
        require(len(data) <= MAX_REPLY, "oversize workload reply")
    return ipc.object_value(json.loads(data, object_pairs_hook=ipc.unique_object, parse_constant=ipc.reject_constant))


def validate_load(samples: list[dict[str, object]]) -> dict[str, object]:
    require(len(samples) == resources.SAMPLES, "incomplete workload samples")
    first_status: dict[str, object] | None = None
    last_bytes = 0
    durations: list[int] = []
    for index, sample in enumerate(samples):
        require(set(sample) == {"result", "request", "status", "statusDurationNs", "videoBytes"}
                and sample["result"] == "sample", "invalid workload sample")
        require(type(sample["request"]) is int and sample["request"] == index + 1, "invalid request count")
        status = retained_status(sample["status"])
        if first_status is None:
            first_status = status
        require(status == first_status, "application status changed")
        count, duration = sample["videoBytes"], sample["statusDurationNs"]
        require(type(count) is int and last_bytes < count <= 2**64 - 1, "video bytes did not progress")
        require(type(duration) is int and 0 < duration <= 5_000_000_000, "invalid status duration")
        last_bytes = cast(int, count)
        durations.append(cast(int, duration))
    return {"statusReads": len(samples), "statusStable": True, "videoBytes": last_bytes,
            "videoBytesIncreasedEverySample": True, "maxStatusDurationNs": max(durations)}


def main(worker_source: str) -> None:
    samples: list[resources.Sample] = []
    load: list[dict[str, object]] = []
    child: subprocess.Popen[bytes] | None = None
    result: dict[str, object] = {"result": "failed"}
    phase = "preflight"
    try:
        require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected observer host")
        uid = pwd.getpwnam("hardened-blikvm-server").pw_uid
        gid = grp.getgrnam("hardened-blikvm").gr_gid
        require(uid > 0 and gid > 0, "invalid workload identity")
        resources.snapshot()  # Fail before starting any workload on a foreign slot.
        phase = "workload"
        child = subprocess.Popen([sys.executable, "-B", "-c", worker_source],
                                 user=uid, group=gid, extra_groups=[], env={},
                                 stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                 bufsize=0)
        require(child.stdin is not None and child.stdout is not None, "workload pipes unavailable")
        for index in range(resources.SAMPLES):
            if index:
                time.sleep(resources.INTERVAL)
            require(child.poll() is None, "workload child ended")
            cast(BinaryIO, child.stdin).write(b"sample\n")
            reply = read_reply(cast(BinaryIO, child.stdout))
            require(reply.get("result") == "sample", "workload read failed")
            # Bind state immediately, rather than waiting until the soak ends.
            retained_status(reply.get("status"))
            if load:
                require(reply["status"] == load[0]["status"], "application status changed")
                require(type(reply["videoBytes"]) is int and reply["videoBytes"]
                        > cast(int, load[-1]["videoBytes"]), "video stopped progressing")
            load.append(reply)
            samples.append(resources.snapshot())
        phase = "validation"
        summary = resources.summarize(samples)
        workload_summary = validate_load(load)
        cast(BinaryIO, child.stdin).write(b"stop\n")
        cast(BinaryIO, child.stdin).close()
        require(child.wait(timeout=5) == 0, "workload cleanup failed")
        result = {"result": "observed", "workload": WORKLOAD, "samples": samples, "summary": summary,
                  "loadSamples": load, "loadSummary": workload_summary, "workloadChildExited": True}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        result = {"result": "failed", "phase": phase, "completedSamples": len(samples),
                  "errorType": type(error).__name__}
    finally:
        if child is not None:
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=3)
            if child.stdin is not None:
                child.stdin.close()
            if child.stdout is not None:
                child.stdout.close()
        print(json.dumps(result, sort_keys=True))
    if result["result"] != "observed":
        raise SystemExit(1)
