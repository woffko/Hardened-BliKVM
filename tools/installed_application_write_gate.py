#!/usr/bin/env python3
"""Temporary same-boot HTTP admission for the fixed seq7-to-seq8 acceptance."""

import argparse
import json
from pathlib import Path
import os
import stat
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    import application_write_contract as contract
    import application_write_guard as guard
    import application_write_observer as observer
    import application_write_overrides as overrides
    import application_write_pipeline as pipeline
    import application_write_state as state
else:
    from tools import application_write_contract as contract
    from tools import application_write_guard as guard
    from tools import application_write_observer as observer
    from tools import application_write_overrides as overrides
    from tools import application_write_pipeline as pipeline
    from tools import application_write_state as state

upload = observer.upload
HELPERS = tuple(dict.fromkeys(("installed_upload_gate.py", *upload.HELPERS,
    "application_write_contract.py", "application_write_guard.py", "application_write_observer.py",
    "application_write_overrides.py", "application_write_pipeline.py", "application_write_state.py")))


def audit_templates() -> None:
    upload.audit_upload_units()
    for unit in (contract.ACTIVATION, "hardened-blikvm-update-activation@.service"):
        path = Path("/lib/systemd/system") / unit
        raw = upload.checked(["systemctl", "cat", "--no-pager", unit], timeout=5)
        header, separator, body = raw.partition(b"\n")
        contract.require(len(raw) <= 16384 and separator == b"\n" and header.startswith(b"# ")
                         and Path(header[2:].decode()).resolve(strict=True) == path.resolve(strict=True)
                         and body.strip() == path.read_bytes().strip(), "activation transport has foreign overrides")


def before_snapshot(assets: Path) -> dict[str, object]:
    upload.u.installed(assets, "new")
    audit_templates()
    value = upload.snapshot(False)
    cast(dict[str, object], value["serving"]).pop("serverPid")
    terminal = contract.terminal_state(cast(dict[str, object], value["managerStatus"]))
    contract.require(terminal.floor == 7 and terminal.generation == 37, "HTTP gate baseline changed")
    tree = cast(dict[str, object], value["state"])
    slot_root = upload.u.STATE_ROOTS[0]
    slots = {Path(key).name for key in tree if Path(key).parent == slot_root}
    contract.require(len(slots) == 9 and slots == {"current", *(Path(key).name for key in tree
        if Path(key).parent == slot_root and Path(key).name.startswith("app-"))}
        and contract.SOURCE_SLOT in slots and contract.TARGET_SLOT in slots, "expected eight retained slots missing")
    target = slot_root / contract.TARGET_SLOT
    upload.verify_publication(assets / "application.tar", upload.probe.PROFILES["seq8-publish"],
                              {key: value for key, value in tree.items() if Path(key) == target or target in Path(key).parents})
    upload.empty_spool()
    return value


def await_snapshot(plan: dict[str, object], terminal: contract.TerminalState, *, wired: bool) -> None:
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        try:
            value = state.snapshot(wired, terminal)
            state.preserved(cast(dict[str, object], plan["before"]), value, allow_transition=not wired)
            return
        except (OSError, ValueError, subprocess.SubprocessError):
            time.sleep(0.5)
    raise TimeoutError("HTTP gate runtime verification did not converge")


class Restore:
    def __init__(self, root: Path, plan: dict[str, object]):
        self.root, self.plan = root, plan

    def close_admission(self) -> None:
        observer.close_admission()
        self.plan["phase"] = "restoring"
        guard.save(self.root, self.plan)

    def drain_naturally(self) -> None:
        # Settlement observes all workers; their installed timeouts, rather
        # than controller signals, own termination and OnSuccess delivery.
        observer.require_closed_admission()

    def await_terminal_pipeline(self) -> contract.TerminalState:
        def remember(floor: int) -> None:
            self.plan["observedFloor"] = floor
            guard.save(self.root, self.plan)
        terminal = pipeline.await_terminal(observer.observe, remember,
            observed_floor=cast(int, self.plan["observedFloor"]))
        upload.empty_spool()
        self.plan["terminal"] = {"slot": terminal.slot, "floor": terminal.floor, "generation": terminal.generation}
        guard.save(self.root, self.plan)
        return terminal

    def remove_owned_overrides(self) -> None:
        overrides.remove()

    def reload_and_restart_confirmed(self) -> None:
        upload.checked(["systemctl", "daemon-reload"], timeout=15)
        upload.checked(["systemctl", "restart", contract.CURRENT], timeout=30)

    def verify_closed(self, terminal: contract.TerminalState) -> None:
        observer.require_closed_admission()
        await_snapshot(self.plan, terminal, wired=False)
        self.plan["phase"] = "restored"
        guard.save(self.root, self.plan)


def restore(root: Path, plan: dict[str, object]) -> contract.TerminalState:
    terminal = contract.restore_runtime(Restore(root, plan))
    for name, content in contract.guard_units(root).items():
        path = overrides.UNIT_ROOT / name
        meta = path.lstat()
        contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                         and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == content,
                         "HTTP deadline unit changed")
    # Do not stop the restore service: this may be its own deadline invocation.
    upload.checked(["systemctl", "stop", root.name + "-deadline.timer"], timeout=15)
    return terminal


def prepare(assets: Path, root: Path) -> dict[str, object]:
    contract.guard_units(root)
    for prefix in (contract.PREFIX, upload.PREFIX, upload.application.PREFIX):
        contract.require(not list(Path("/var/lib").glob(prefix + "*")), "unfinished update guard")
    before = before_snapshot(assets)
    root.mkdir(mode=0o700)
    os.chmod(root, 0o700)
    upload.u.sync_directory(root.parent)
    plan: dict[str, object] = {"schemaVersion": 1, "phase": "prepared", "bootId": guard.BOOT_ID.read_text().strip(),
        "startedMonotonic": time.monotonic(), "observedFloor": 7, "sourceSlot": contract.SOURCE_SLOT,
        "targetSlot": contract.TARGET_SLOT, "before": before}
    with guard.locked(root):
        for name in ("bindings.json", "new.deb", *HELPERS):
            upload.u.new_file(root / name, (assets / name).read_bytes())
        upload.u.new_file(root / "controller.py", Path(__file__).read_bytes())
        plan["files"] = {name: upload.u.digest(root / name) for name in ("bindings.json", "new.deb", "controller.py", *HELPERS)}
        guard.save(root, plan)
        try:
            for name, data in contract.guard_units(root).items():
                upload.u.new_file(overrides.UNIT_ROOT / name, data, 0o644)
            upload.checked(["systemctl", "daemon-reload"], timeout=15)
            upload.checked(["systemctl", "start", root.name + "-deadline.timer"], timeout=15)
            upload.checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"], timeout=5)
            guard.require_open_budget(root, plan)
            overrides.install()
            upload.checked(["systemctl", "daemon-reload"], timeout=15)
            guard.require_open_budget(root, plan)
            upload.checked(["systemctl", "start", contract.UPLOAD, contract.ACTIVATION], timeout=20)
            guard.require_open_budget(root, plan)
            upload.checked(["systemctl", "restart", contract.CURRENT], timeout=30)
            await_snapshot(plan, contract.TerminalState(contract.SOURCE_SLOT, 7, 37), wired=True)
            guard.require_open_budget(root, plan)
            plan["phase"] = "wired"
            guard.save(root, plan)
        except (OSError, ValueError, TimeoutError, subprocess.SubprocessError):
            # Failure never leaves known admission deliberately open while
            # waiting for the independent recovery action.
            observer.close_admission()
            raise
    return {"result": "prepared", "guardRoot": str(root), "phase": "wired", "deadlineArmed": True}


def cleanup(root: Path, plan: dict[str, object]) -> dict[str, object]:
    contract.require(plan["phase"] == "restored", "HTTP gate has not been restored")
    terminal = observer.observe().terminal(cast(int, plan["observedFloor"]))
    state.preserved(cast(dict[str, object], plan["before"]), state.snapshot(False, terminal), allow_transition=True)
    upload.s.inactive(root.name + "-restore.service")
    expected = {"bindings.json", "new.deb", "controller.py", *HELPERS}
    files = cast(dict[str, str], plan.get("files"))
    contract.require(isinstance(plan.get("files"), dict) and set(files) == expected, "guard file bindings missing")
    present = {path.name for path in root.iterdir()}
    contract.require(present in (expected | {"plan.json"}, expected | {"plan.json", "deadline-fired"}), "foreign guard contents")
    for name in present:
        path = root / name
        meta = path.lstat()
        contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                         and stat.S_IMODE(meta.st_mode) == 0o600, "unsafe guard cleanup file")
        if name in files:
            contract.require(upload.u.digest(path) == files[name], "guard helper changed")
        elif name == "deadline-fired":
            contract.require(path.read_bytes() == b"closed\n", "deadline marker changed")
    for name, content in contract.guard_units(root).items():
        path = overrides.UNIT_ROOT / name
        meta = path.lstat()
        contract.require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                         and stat.S_IMODE(meta.st_mode) == 0o644 and path.read_bytes() == content, "foreign guard unit")
    upload.checked(["systemctl", "stop", root.name + "-deadline.timer"], timeout=15)
    for name in contract.guard_units(root):
        (overrides.UNIT_ROOT / name).unlink()
    upload.u.sync_directory(overrides.UNIT_ROOT)
    upload.checked(["systemctl", "daemon-reload"], timeout=15)
    for name in present:
        (root / name).unlink()
    root.rmdir()
    upload.u.sync_directory(root.parent)
    return {"result": "cleaned", "guardRoot": str(root), "floor": terminal.floor,
            "generation": terminal.generation, "runtimeWritesClosed": True, "guardRetained": False}


def transition(action: str, root: Path) -> dict[str, object]:
    guard.load(root)
    if action == "deadline":
        # Validate namespace/marker first, close before acquiring the lock.
        try:
            guard.mark_deadline(root)
        finally:
            observer.close_admission()
    else:
        observer.close_admission()
    with guard.locked(root):
        plan = guard.load(root)
        upload.u.installed(root, "new")
        audit_templates()
        if action == "cleanup":
            return cleanup(root, plan)
        terminal = restore(root, plan)
        if action == "finalize":
            contract.terminal_state(upload.application.infrastructure.current_status(), observed_floor=terminal.floor,
                                    require_target=True)
        return {"result": "restored", "guardRoot": str(root), "phase": plan["phase"],
                "slot": terminal.slot, "floor": terminal.floor, "generation": terminal.generation,
                "runtimeWritesClosed": True, "guardRetained": True}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "restore", "finalize", "deadline", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    args = parser.parse_args()
    contract.require(os.geteuid() == 0, "root controller required")
    upload.u.select_profile("dev7-dev9")
    if args.action == "prepare":
        contract.require(args.assets is not None and args.guard_root is not None, "prepare requires assets and reserved guard root")
        assert args.assets is not None
        assert args.guard_root is not None
        result = prepare(args.assets, Path(args.guard_root))
    else:
        contract.require(args.guard_root is not None and args.assets is None, "transition requires only guard root")
        assert args.guard_root is not None
        result = transition(args.action, guard.root_path(args.guard_root))
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
