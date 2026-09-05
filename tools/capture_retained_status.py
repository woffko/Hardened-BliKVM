#!/usr/bin/env python3
"""Read-only observation of retained status wiring after guard cleanup."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys
import traceback
import time

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import persistent_status_wiring as wiring
else:
    from tools import persistent_status_wiring as wiring


def capture(value: str) -> dict[str, object]:
    root = Path(value)
    wiring.require(root.parent == Path("/var/lib") and re.fullmatch(wiring.PREFIX + "[a-f0-9]{24}", root.name) is not None,
                   "invalid retained guard reference")
    wiring.require(not wiring.exists(root) and not list(Path("/var/lib").glob(wiring.PREFIX + "*")), "persistent guard remains")
    wiring.require(not list(wiring.UNIT_ROOT.glob(wiring.PREFIX + "*"))
                   and not list(wiring.UNIT_ROOT.glob("*.wants/" + wiring.PREFIX + "*")), "persistent recovery unit remains")
    wiring.validate_dropins(True)
    snapshot = wiring.common.snapshot(True)
    start = wiring.s.property_value(wiring.s.CURRENT_SERVER, "ExecMainStartTimestampMonotonic")
    socket = wiring.s.property_value(wiring.common.SOCKET, "ActiveEnterTimestampMonotonic")
    recovery = wiring.s.property_value("hardened-blikvm-update-recover.service", "ExecMainExitTimestampMonotonic")
    wiring.require(all(value.isdecimal() and int(value) > 0 for value in (start, socket, recovery))
                   and int(start) >= max(int(socket), int(recovery)), "status or recovery did not precede server")
    uptime = float(Path("/proc/uptime").read_text().split()[0])
    wiring.require(uptime >= 0, "invalid uptime")
    return {"result": "inspect-retained", "guardRoot": str(root), "guardRemoved": True,
            "persistentWiringVerified": True, "startupOrderVerified": True,
            "bootIdSha256": wiring.boot_identity(), "uptimeSeconds": int(uptime), "snapshot": snapshot,
            "dropinHashes": {str(path): wiring.u.digest(path) for path in wiring.dropins()}}


def await_capture(value: str) -> dict[str, object]:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            return capture(value)
        except (OSError, ValueError):
            time.sleep(0.5)
    raise ValueError("serving observation did not converge after single start")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guard-root", required=True)
    parser.add_argument("--recover-serving", action="store_true")
    args = parser.parse_args()
    wiring.require(os.geteuid() == 0, "root read-only observation required")
    try:
        if args.recover_serving:
            wiring.validate_dropins(True)
            wiring.s.bootstrap.trusted_code_and_config(expected_supervisor_sha=wiring.u.SUPERVISOR_HASHES["new"])
            wiring.require(wiring.s.property_value(wiring.s.CURRENT_SERVER, "MainPID") == "0"
                           and wiring.s.property_value("hardened-blikvm-update-recover.service", "ExecMainStatus") == "20",
                           "unexpected serving recovery precondition")
            status = wiring.common.infrastructure.current_status()
            wiring.require(status.get("floorSequence") == 5 and status.get("generation") == 22
                           and status.get("pending") is None
                           and status.get("activeSlotId") == status.get("confirmedSlotId") == wiring.SLOT,
                           "cannot recover serving from unexpected application state")
            wiring.checked(["systemctl", "start", "hardened-blikvm-update-recover.service"])
            wiring.checked(["systemctl", "start", wiring.s.CURRENT_SERVER])
            observed = await_capture(args.guard_root)
            observed["result"] = "recover-serving"
            observed["beforeStatus"] = status
            print(json.dumps(observed, sort_keys=True))
        else:
            print(json.dumps(capture(args.guard_root), sort_keys=True))
    except (OSError, ValueError) as error:
        known = ("persistent guard remains", "persistent recovery unit remains", "runtime overrides remain",
                 "foreign persistent override directory", "persistent override changed", "unexpected installed updater version",
                 "installed status unit changed", "loaded status environment mismatch", "running status environment mismatch",
                 "status socket was enabled", "status socket not active", "unsafe status socket",
                 "application journal generation changed", "status or recovery did not precede server",
                 "operational application state changed", "updater service not enabled/active", "watchdog invocation failed",
                 "unsafe updater runtime ownership/mode", "unexpected active update transaction", "tmpfiles configuration changed",
                 "serving service unavailable", "serving executable changed")
        reason = str(error)
        allowed = set(wiring.HELPERS) | {"capture_retained_status.py"}
        locations = [{"file": Path(frame.filename).name, "line": frame.lineno}
                     for frame in traceback.extract_tb(error.__traceback__)
                     if Path(frame.filename).name in allowed][-6:]
        print(json.dumps({"result": "failed", "errorType": type(error).__name__,
                          "checkLocations": locations,
                          "errorClass": reason if reason in known else "unclassified-observer-failure"}, sort_keys=True))
        raise SystemExit(1) from None


if __name__ == "__main__":
    main()
