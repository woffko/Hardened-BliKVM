#!/usr/bin/env python3
"""Read-only readiness and boot ordering for the retained dev5 application."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import enable_update_services as infrastructure
else:
    from tools import enable_update_services as infrastructure

u = infrastructure.upgrade
s = u.serving
require = u.require
RECOVERY = "hardened-blikvm-update-recover.service"
SLOT = "app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53"


def capture(assets: Path, post: bool) -> dict[str, object]:
    u.installed(assets, "new")
    if u.selected_profile == "dev7-dev9":
        snapshot = u.snapshot("new")
    else:
        snapshot = infrastructure.enabled_snapshot(expected_slot=SLOT, expected_floor=2, check_initial=False)
    for directive in ("Requires", "After"):
        require(RECOVERY in s.property_value(s.CURRENT_SERVER, directive).split(), "mandatory recovery dependency missing")
    recovery = {field: s.property_value(RECOVERY, field) for field in
                ("ActiveState", "SubState", "Result", "ExecMainStatus", "ExecMainExitTimestampMonotonic")}
    started = s.property_value(s.CURRENT_SERVER, "ExecMainStartTimestampMonotonic")
    exited = recovery["ExecMainExitTimestampMonotonic"]
    if post:
        require(recovery["ActiveState"] == "active" and recovery["SubState"] == "exited"
                and recovery["Result"] == "success" and recovery["ExecMainStatus"] == "0", "startup recovery failed")
        require(exited.isdecimal() and started.isdecimal() and 0 < int(exited) <= int(started), "server started before verification completed")
        if u.selected_profile == "dev7-dev9":
            socket_started = s.property_value("hardened-blikvm-update-status.socket", "ActiveEnterTimestampMonotonic")
            require(socket_started.isdecimal() and 0 < int(socket_started) <= int(started), "server started before status socket")
    return {"result": "ready", "bootIdSha256": hashlib.sha256(Path("/proc/sys/kernel/random/boot_id").read_bytes()).hexdigest(),
            "uptimeSeconds": int(float(Path("/proc/uptime").read_text().split()[0])),
            "snapshot": snapshot, "status": infrastructure.current_status(), "recovery": recovery,
            "serverStartedMonotonic": started, "recoveryOrderVerified": post,
            "rootfsMode": "ro" if os.statvfs("/").f_flag & os.ST_RDONLY else "rw"}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--post", action="store_true")
    parser.add_argument("--profile", choices=u.PROFILES, default="dev6-dev7")
    args = parser.parse_args()
    u.select_profile(args.profile)
    require(os.geteuid() == 0, "root read-only observation required")
    try:
        result = capture(args.assets, args.post)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        result = {"result": "not_ready", "errorType": type(error).__name__, "reason": str(error)[:256],
                  "rootfsMode": "ro" if os.statvfs("/").f_flag & os.ST_RDONLY else "rw"}
    print(json.dumps(result, sort_keys=True))
    if result["result"] != "ready":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
