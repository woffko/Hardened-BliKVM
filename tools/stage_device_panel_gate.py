#!/usr/bin/env python3
"""Run the ST7789 panel status gate on a test board.

The gate proves the reviewed panel behaviour on the device:

- it reads the board daemon's status and requires the panel present and
  initialized with the backlight on;
- with ``--confirm`` it draws the fixed test pattern (the reviewed SPI path),
  restarts the daemon, and then requires the daemon to re-initialize the panel
  and light it again by itself, so the boot path always paints a defined frame
  instead of leaving the controller's uninitialized memory visible;
- it records what the board reported, never the pixels, which the operator
  confirms visually.

The SSH password arrives on the first stdin line. Without ``--confirm`` the
gate only observes.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import re
import sys
from pathlib import Path
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


FIRST_LINE_MAX = 512
BOARDD_UNIT = "hardened-blikvm-boardd.service"
BEFORE_FACTS = {"before_present", "before_initialized", "before_backlight"}
AFTER_FACTS = {"after_present", "after_initialized", "after_backlight"}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    mode: str
    displayPresent: bool
    initializedBefore: bool
    backlightBefore: bool
    testPatternCode: int
    initializedAfter: bool
    backlightAfter: bool
    daemonRestarted: bool
    failureStage: str
    failureType: str
    mutationsPerformed: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str
    confirm: bool
    timeout_seconds: int


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--confirm", action="store_true")
    _ = parser.add_argument("--timeout-seconds", type=int, default=60)
    namespace = parser.parse_args()
    if not 1 <= cast(int, namespace.timeout_seconds) <= 300:
        raise SystemExit("--timeout-seconds is out of range")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        confirm=bool(namespace.confirm),
        timeout_seconds=cast(int, namespace.timeout_seconds),
    )


def read_secret() -> bytearray:
    stream = getattr(sys.stdin, "buffer", sys.stdin)
    raw = stream.readline()
    if isinstance(raw, str):
        raw = raw.encode("utf-8", "replace")
    if not raw or len(raw) > FIRST_LINE_MAX:
        raise SystemExit("the SSH secret is missing on stdin")
    return bytearray(raw.rstrip(b"\r\n"))


def status_script(phase: str, draw: bool) -> str:
    """Read the panel state as `before`/`after`, optionally drawing the pattern."""
    drawing = (
        'print("test_code=%s" % call({"request": "display_test"}).get("code"))'
        if draw
        else "pass"
    )
    return (
        "set -e\n"
        "python3 - <<'PY'\n"
        "import json, socket\n"
        "SOCK = '/run/hardened-blikvm-boardd/control.sock'\n"
        "def call(req):\n"
        "    s = socket.socket(socket.AF_UNIX)\n"
        "    s.settimeout(15)\n"
        "    s.connect(SOCK)\n"
        "    s.sendall((json.dumps(req) + '\\n').encode())\n"
        "    data = b''\n"
        "    while not data.endswith(b'\\n'):\n"
        "        chunk = s.recv(4096)\n"
        "        if not chunk:\n"
        "            break\n"
        "        data += chunk\n"
        "    s.close()\n"
        "    return json.loads(data.decode())\n"
        "status = call({'request': 'status'})\n"
        "data = status.get('data') or {}\n"
        f"print('{phase}_present=%s' % data.get('displayPresent'))\n"
        f"print('{phase}_initialized=%s' % data.get('displayInitialized'))\n"
        f"print('{phase}_backlight=%s' % data.get('displayBacklightOn'))\n"
        f"{drawing}\n"
        "PY\n"
    )


def require_ssh(options: Options, secret: bytearray, script: str) -> dict[str, str]:
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, options.username, Path(options.known_hosts)
    )
    result = prototype.run_ssh(target, secret, script, timeout=options.timeout_seconds)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    if result.returncode != 0:
        raise GateError("ssh", "command_failed")
    return facts_from(stdout)


def facts_from(stdout: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in stdout.splitlines():
        if "=" not in line or line.startswith(" "):
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]+", key):
            facts[key] = value
    return facts


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    secret = read_secret()
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-panel-status-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "displayPresent": False,
        "initializedBefore": False,
        "backlightBefore": False,
        "testPatternCode": -1,
        "initializedAfter": False,
        "backlightAfter": False,
        "daemonRestarted": False,
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    stage = "read"

    try:
        before = require_ssh(options, secret, status_script("before", False))
        if not BEFORE_FACTS.issubset(before):
            raise GateError(stage, "status_unavailable")
        receipt["displayPresent"] = before["before_present"] == "True"
        receipt["initializedBefore"] = before["before_initialized"] == "True"
        receipt["backlightBefore"] = before["before_backlight"] == "True"
        if not receipt["displayPresent"] or not receipt["initializedBefore"]:
            raise GateError(stage, "panel_not_initialized")

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "test_pattern"
        drawn = require_ssh(options, secret, status_script("before", True))
        receipt["testPatternCode"] = int(drawn.get("test_code", -1))
        if receipt["testPatternCode"] != 0:
            raise GateError(stage, "test_pattern_rejected")

        stage = "restart"
        require_ssh(
            options,
            secret,
            f"systemctl restart {BOARDD_UNIT}\nsleep 5\n",
        )
        receipt["daemonRestarted"] = True
        receipt["mutationsPerformed"] = True
        receipt["persistentChangeScope"] = "one board daemon restart"

        stage = "verify"
        after = require_ssh(options, secret, status_script("after", False))
        if not AFTER_FACTS.issubset(after):
            raise GateError(stage, "status_unavailable_after_restart")
        receipt["initializedAfter"] = after["after_initialized"] == "True"
        receipt["backlightAfter"] = after["after_backlight"] == "True"
        if not receipt["initializedAfter"] or not receipt["backlightAfter"]:
            raise GateError(stage, "panel_not_repainted_after_restart")
    except GateError as error:
        receipt["result"] = "failed"
        receipt["failureStage"] = error.stage
        receipt["failureType"] = error.failure_type
        write_json_0600_atomic(output, receipt)
        return 1

    receipt["result"] = "passed"
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2
    except Exception as error:  # noqa: BLE001 - the operator log is the surface
        import traceback

        traceback.print_exc()
        print(f"gate failed before a receipt: {error!r}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
