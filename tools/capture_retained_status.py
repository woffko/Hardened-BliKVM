#!/usr/bin/env python3
"""Read-only observation of retained status wiring after guard cleanup."""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import sys

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


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--guard-root", required=True)
    args = parser.parse_args()
    wiring.require(os.geteuid() == 0, "root read-only observation required")
    print(json.dumps(capture(args.guard_root), sort_keys=True))


if __name__ == "__main__":
    main()
