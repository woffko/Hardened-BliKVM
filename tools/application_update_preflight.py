"""Read-only device checks before the retained-status application transition."""

from __future__ import annotations

import os
from pathlib import Path
import stat
from typing import cast

from tools import installed_application_update as update
from tools import observe_service_resources as resources
from tools import probe_status_restore_timer as status_probe

require = update.require


def verify(bindings: dict[str, object], status_source: str) -> dict[str, object]:
    require(os.geteuid() == 0 and os.uname().machine == "aarch64", "unexpected preflight host")
    update.select_profile("seq5-to-seq6-mjpeg")
    require(not list(Path("/var/lib").glob(update.PREFIX + "*"))
            and not list(update.u.UNIT_ROOT.glob(update.PREFIX + "*")), "application transaction already present")
    require(not update.infrastructure.exists(update.DROPIN_ROOT), "candidate runtime override already present")
    require(not update.infrastructure.exists(update.u.STATE_ROOTS[0] / update.active_profile.target_slot), "target slot already present")
    before = resources.snapshot()
    protected = update.s.snapshot()
    tree = update.u.tree_fingerprint(update.u.STATE_ROOTS)
    expected = cast(dict[str, object], bindings["new"])
    require(expected.get("sha256") == update.u.PACKAGE_HASHES["new"]
            and expected.get("version") == update.u.VERSIONS["new"], "updater binding mismatch")
    require(update.checked(["dpkg-query", "-W", "-f=${db:Status-Abbrev}:${Version}", update.u.PACKAGE]).decode()
            == "ii :" + update.u.VERSIONS["new"], "installed updater version mismatch")
    files = cast(dict[str, list[object]], expected["files"])
    require(1 <= len(files) <= 64, "invalid updater file count")
    for name, values in files.items():
        path = Path(name)
        require(path.is_absolute() and ".." not in path.parts and len(values) == 2, "invalid updater file binding")
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == meta.st_gid == 0 and meta.st_nlink == 1
                and stat.S_IMODE(meta.st_mode) == values[1] and update.u.digest(path) == values[0], "installed updater file mismatch")
    old = cast(dict[str, object], bindings["old"])
    require(old.get("sha256") == update.u.PACKAGE_HASHES["old"], "old updater binding mismatch")
    extras = set(cast(dict[str, object], old["files"])) - set(files)
    require(all(not update.infrastructure.exists(Path(name)) for name in extras), "old updater left extra files")
    require(not update.checked(["dpkg", "--audit"]).strip(), "dpkg audit failed")
    require(not update.checked(["getcap", str(update.s.bootstrap.SUPERVISOR),
                                "/usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter"]).strip(), "unexpected file capabilities")
    update.s.bootstrap.trusted_code_and_config(expected_supervisor_sha=update.u.SUPERVISOR_HASHES["new"])
    update.infrastructure.audit_enabled(check_initial=False)
    update.validate_status_wiring()
    value = status_probe.status_read(status_source)
    pid = update.s.property_value(update.s.CURRENT_SERVER, "MainPID")
    require(pid.isdecimal() and int(pid) > 0 and (Path("/proc") / pid / "exe").resolve(strict=True)
            == update.u.STATE_ROOTS[0] / update.active_profile.source_slot / "bin/bli-kvm-server", "source serving slot mismatch")
    require(update.u.tree_fingerprint(update.u.STATE_ROOTS) == tree and update.s.snapshot() == protected,
            "protected state changed during preflight")
    status_probe.same_identity(before, resources.snapshot())
    require(status_probe.status_read(status_source) == value, "application status changed during preflight")
    return {"result": "passed", "profile": update.active_profile.name, "installedUpdaterVerified": True,
            "persistentStatusVerified": True, "sourceSlotVerified": True, "transactionAbsent": True,
            "stateAndPeersUnchanged": True, "status": value, "deviceWrites": False}
