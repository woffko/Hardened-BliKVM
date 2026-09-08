#!/usr/bin/env python3
"""Observe retained service resources over SSH without creating device files."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import observe_service_resources as observer
from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic


def object_map(value: object) -> dict[str, object]:
    observer.require(isinstance(value, dict), "invalid observer object")
    return cast(dict[str, object], value)


def validate(raw: bytes) -> dict[str, object]:
    observer.require(len(raw) <= 256 * 1024, "oversize observer result")
    value = object_map(json.loads(raw))
    observer.require(set(value) == {"result", "workload", "summary", "samples"}
                     and value["result"] == "observed"
                     and value["workload"] == "no-observer-generated-workload", "incomplete observation")
    observer.require(isinstance(value["samples"], list), "invalid sample sequence")
    samples = cast(list[object], value["samples"])
    observer.require(len(samples) == observer.SAMPLES, "invalid sample count")
    for raw_sample in samples:
        sample = object_map(raw_sample)
        observer.require(set(sample) == {"monotonicNs", "bootIdSha256", "units"},
                         "invalid sample fields")
        clock = sample["monotonicNs"]
        observer.require(type(clock) is int and clock > 0, "invalid sample clock")
        boot = sample["bootIdSha256"]
        observer.require(isinstance(boot, str) and len(boot) == 64 and all(c in "0123456789abcdef" for c in boot),
                         "invalid boot digest")
        units = object_map(sample["units"])
        observer.require(set(units) == set(observer.UNITS), "invalid observed units")
        for raw_unit in units.values():
            unit = object_map(raw_unit)
            observer.require(set(unit) == set(observer.UnitSample.__annotations__),
                             "invalid unit sample")
            observer.require(all(type(item) is int and 0 <= item <= 2**64 - 1 for item in unit.values()),
                             "invalid resource number")
            numbers = cast(dict[str, int], unit)
            observer.require(numbers["mainPid"] > 0 and numbers["mainStartTicks"] > 0 and numbers["processes"] > 0,
                             "invalid process identity")
    observer.require(value["summary"] == observer.summarize(cast(list[observer.Sample], samples)),
                     "resource summary mismatch")
    return value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    observer.require(not output.exists(), "new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    source = Path(observer.__file__).read_bytes()
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"schemaVersion": 1, "probe": "retained-service-resource-baseline",
                                "observerSha256": hashlib.sha256(source).hexdigest(), "result": "failed"}
    try:
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source.decode()]), timeout=900)
        receipt["sshExitCode"] = result.returncode
        if result.returncode == 0:
            receipt["observation"] = validate(result.stdout)
            receipt["result"] = "observed"
        else:
            receipt["errorType"] = "remote-observation-failed"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "observed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
