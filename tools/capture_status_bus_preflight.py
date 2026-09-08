#!/usr/bin/env python3
"""Read selected public systemd properties before implementing an outage guard.

No timer creation, unit start/stop, configuration access or device file writes.
The existing tmpfiles timer is read only to observe the D-Bus wire format.
"""

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

from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.systemd_property import decode_property

UNIT_PATH = "/org/freedesktop/systemd1/unit/"
SOCKET_PATH = UNIT_PATH + "hardened_2dblikvm_2dupdate_2dstatus_2esocket"
TIMER_PATH = UNIT_PATH + "systemd_2dtmpfiles_2dclean_2etimer"
INTERFACE = "org.freedesktop.systemd1."
QUERIES = (
    ("version", "/org/freedesktop/systemd1", INTERFACE + "Manager", "Version"),
    ("socketActive", SOCKET_PATH, INTERFACE + "Unit", "ActiveState"),
    ("socketSubState", SOCKET_PATH, INTERFACE + "Unit", "SubState"),
    ("socketAccept", SOCKET_PATH, INTERFACE + "Socket", "Accept"),
    ("socketMaxConnections", SOCKET_PATH, INTERFACE + "Socket", "MaxConnections"),
    ("socketConnections", SOCKET_PATH, INTERFACE + "Socket", "NConnections"),
    ("referenceMonotonic", TIMER_PATH, INTERFACE + "Timer", "NextElapseUSecMonotonic"),
    ("referenceRealtime", TIMER_PATH, INTERFACE + "Timer", "NextElapseUSecRealtime"),
    ("referenceWakeSystem", TIMER_PATH, INTERFACE + "Timer", "WakeSystem"),
    ("referenceTimerSpec", TIMER_PATH, INTERFACE + "Timer", "TimersMonotonic"),
)
SIGNATURES = dict(zip((query[0] for query in QUERIES), ("s", "s", "s", "b", "u", "u", "t", "t", "b", "a(stt)")))
REMOTE = r'''
import json, os, subprocess, time
values = {}
phase = "host"
try:
    if os.geteuid() != 0 or os.uname().machine != "aarch64":
        raise ValueError("unexpected host")
    for name, path, interface, prop in queries:
        phase = name
        result = subprocess.run(
            ["busctl", "--system", "--json=short", "get-property",
             "org.freedesktop.systemd1", path, interface, prop],
            capture_output=True, timeout=5, check=True)
        if len(result.stdout) > 16384:
            raise ValueError("oversize property")
        value = json.loads(result.stdout)
        if not isinstance(value, dict) or set(value) != {"type", "data"}:
            raise ValueError("unexpected property envelope")
        values[name] = value
    output = {"result": "observed", "properties": values,
              "monotonicUsec": time.monotonic_ns() // 1000}
except (OSError, ValueError, subprocess.SubprocessError) as error:
    output = {"result": "failed", "properties": values, "phase": phase,
              "errorType": type(error).__name__}
print(json.dumps(output, sort_keys=True))
if output["result"] != "observed":
    raise SystemExit(1)
'''


def remote_source() -> str:
    return "queries = " + repr(QUERIES) + "\n" + REMOTE


def validate(raw: bytes) -> dict[str, object]:
    if len(raw) > 128 * 1024:
        raise ValueError("oversize preflight receipt")
    parsed: object = json.loads(raw)
    if not isinstance(parsed, dict):
        raise ValueError("invalid preflight receipt")
    value = cast(dict[str, object], parsed)
    if value.get("result") == "observed":
        if (set(value) != {"result", "properties", "monotonicUsec"}
                or type(value["monotonicUsec"]) is not int or value["monotonicUsec"] <= 0):
            raise ValueError("invalid preflight clock")
    elif value.get("result") == "failed":
        if (set(value) != {"result", "properties", "phase", "errorType"}
                or not isinstance(value["phase"], str) or not isinstance(value["errorType"], str)
                or value["phase"] not in {"host", *(query[0] for query in QUERIES)}
                or value["errorType"] not in {"OSError", "ValueError", "CalledProcessError", "TimeoutExpired",
                                               "FileNotFoundError", "PermissionError", "JSONDecodeError"}):
            raise ValueError("invalid preflight failure")
    else:
        raise ValueError("invalid preflight result")
    if not isinstance(value.get("properties"), dict):
        raise ValueError("invalid preflight properties")
    properties = cast(dict[str, object], value["properties"])
    names = {query[0] for query in QUERIES}
    if not set(properties) <= names or (value["result"] == "observed" and set(properties) != names):
        raise ValueError("unexpected preflight property set")
    for name, prop in properties.items():
        decode_property(prop, SIGNATURES[name])
    return value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    if output.exists():
        raise ValueError("new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    source = remote_source()
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "status-systemd-wire-preflight", "result": "failed",
                                "sourceSha256": hashlib.sha256(source.encode()).hexdigest()}
    try:
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=90)
        receipt["sshExitCode"] = result.returncode
        observation = validate(result.stdout)
        receipt["observation"] = observation
        if result.returncode == 0 and observation["result"] == "observed":
            receipt["result"] = "observed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "observed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
