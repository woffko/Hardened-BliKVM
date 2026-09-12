#!/usr/bin/env python3
"""Read-only readiness and boot ordering for the retained dev5 application."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
from pathlib import Path
import subprocess
import sys
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import enable_update_services as infrastructure
    import status_api_wiring as status_api
    import persistent_status_wiring as persistent
else:
    from tools import enable_update_services as infrastructure
    from tools import status_api_wiring as status_api
    from tools import persistent_status_wiring as persistent

u = infrastructure.upgrade
s = u.serving
require = u.require
RECOVERY = "hardened-blikvm-update-recover.service"
SLOT = "app-00000000000000000002-5df0c19a152415011fe2fa3f9510fbd1d4c8d02d79c7db8a6619fa225cb41f53"
SEQ7_SLOT = "app-00000000000000000007-f2262e8f9929dda83b2baf1005b5ae9262aa075b7ddaa16f4ad6200b5daf0140"
SEQ8_SLOT = "app-00000000000000000008-375f9800a147b8f273073349af1613e3fb0cf9f7b97af914d7a75e9446ea694b"
SEQ9_SLOT = "app-00000000000000000009-5f1aee4802addd6c0c0b444224345624cd8b9b8c46950a2231ad7e2b42de37c2"
APPLICATION_PROFILES = ("seq2", "seq6", "seq7", "seq8", "seq9")


def resolve_application_profile(updater_profile: str, requested: str | None) -> str:
    allowed = ("seq2",) if updater_profile == "dev6-dev7" else ("seq6", "seq7", "seq8", "seq9") if updater_profile == "dev7-dev9" else ()
    if not allowed:
        raise ValueError("unknown updater profile")
    selected = requested if requested is not None else allowed[0]
    require(selected in allowed, "application and updater profile mismatch")
    return selected


def seq7_snapshot() -> dict[str, object]:
    require(u.selected_profile == "dev7-dev9", "sequence seven requires retained dev9")
    persistent.validate_dropins(True)
    u.write_transports_inert()
    value = status_api.snapshot(True, expected_version=u.VERSIONS["new"],
                                expected_supervisor_sha=u.SUPERVISOR_HASHES["new"],
                                expected_slot=SEQ7_SLOT, expected_floor=7, expected_generation=37)
    require(cast(dict[str, object], value["managerStatus"]).get("previousSlotId") == u.SEQ6_SLOT,
            "sequence-seven rollback anchor changed")
    cast(dict[str, object], value["serving"])["serverPid"] = s.property_value(s.CURRENT_SERVER, "MainPID")
    value["persistentDropinHashes"] = {str(path): u.digest(path) for path in persistent.dropins()}
    return value


def capture(assets: Path, post: bool, application_profile: str | None = None) -> dict[str, object]:
    application_profile = resolve_application_profile(u.selected_profile, application_profile)
    u.installed(assets, "new")
    if application_profile == "seq9":
        snapshot = retained_snapshot(SEQ9_SLOT, 9, 43, SEQ8_SLOT)
    elif application_profile == "seq8":
        snapshot = seq8_snapshot()
    elif application_profile == "seq7":
        snapshot = seq7_snapshot()
    elif u.selected_profile == "dev7-dev9":
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
    return {"result": "ready", "applicationProfile": application_profile,
            "bootIdSha256": hashlib.sha256(Path("/proc/sys/kernel/random/boot_id").read_bytes()).hexdigest(),
            "uptimeSeconds": int(float(Path("/proc/uptime").read_text().split()[0])),
            "snapshot": snapshot, "status": infrastructure.current_status(), "recovery": recovery,
            "serverStartedMonotonic": started, "recoveryOrderVerified": post,
            "rootfsMode": "ro" if os.statvfs("/").f_flag & os.ST_RDONLY else "rw"}


def seq8_snapshot() -> dict[str, object]:
    return retained_snapshot(SEQ8_SLOT, 8, 40, SEQ7_SLOT)


def retained_snapshot(slot: str, floor: int, generation: int, previous: str) -> dict[str, object]:
    require((slot, floor, generation, previous) in ((SEQ8_SLOT, 8, 40, SEQ7_SLOT), (SEQ9_SLOT, 9, 43, SEQ8_SLOT)),
            'unreviewed retained application profile')
    require(u.selected_profile == "dev7-dev9", "application profile requires retained dev9")
    persistent.validate_dropins(True)
    u.write_transports_inert()
    for root in (Path("/var/lib"), Path("/run/systemd/system")):
        require(not list(root.glob("hardened-blikvm-http-update-*")), "HTTP acceptance guard remains")
    value = status_api.snapshot(True, expected_version=u.VERSIONS["new"],
        expected_supervisor_sha=u.SUPERVISOR_HASHES["new"], expected_slot=slot,
        expected_floor=floor, expected_generation=generation)
    status = cast(dict[str, object], value["managerStatus"])
    require(status.get("previousSlotId") == previous and status.get("lastRollback") is None,
            "confirmation or rollback anchor changed")
    forbidden = {"BLI_KVM_UPDATE_UPLOAD_SOCKET", "BLI_KVM_UPDATE_ACTIVATION_SOCKET", "BLI_KVM_UPDATE_WRITES_ENABLED"}
    for unit in (s.CURRENT_SERVER, s.CANDIDATE_SERVER):
        require(not any(item.partition("=")[0] in forbidden for item in shlex.split(s.property_value(unit, "Environment"))),
                "HTTP write configuration remains")
    pid = s.property_value(s.CURRENT_SERVER, "MainPID")
    require(pid.isascii() and pid.isdecimal() and int(pid) > 0, "invalid serving PID")
    environment = (Path("/proc") / pid / "environ").read_bytes()
    require(len(environment) <= 65536 and not any(item.partition(b"=")[0] in {name.encode() for name in forbidden}
            for item in environment.split(b"\0")), "running HTTP write configuration remains")
    cast(dict[str, object], value["serving"])["serverPid"] = pid
    value["persistentDropinHashes"] = {str(path): u.digest(path) for path in persistent.dropins()}
    return value


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path, required=True)
    parser.add_argument("--post", action="store_true")
    parser.add_argument("--profile", choices=u.PROFILES, default="dev6-dev7")
    parser.add_argument("--application-profile", choices=APPLICATION_PROFILES)
    args = parser.parse_args()
    u.select_profile(args.profile)
    require(os.geteuid() == 0, "root read-only observation required")
    try:
        result = capture(args.assets, args.post, args.application_profile)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        result = {"result": "not_ready", "errorType": type(error).__name__, "reason": str(error)[:256],
                  "rootfsMode": "ro" if os.statvfs("/").f_flag & os.ST_RDONLY else "rw"}
    print(json.dumps(result, sort_keys=True))
    if result["result"] != "ready":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
