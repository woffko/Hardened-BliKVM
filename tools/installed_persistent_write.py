#!/usr/bin/env python3
"""Prepare revocable persistent update wiring; commit requires explicit approval."""

import argparse
from contextlib import AbstractContextManager
from dataclasses import asdict
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True
if __name__ == "__main__":
    sys.modules.setdefault("installed_persistent_write", sys.modules[__name__])
if not __package__:
    import installed_application_write_gate as temporary
    import persistent_write_commit as commit_backend
    import persistent_write_contract as contract
    import persistent_write_decision as decisions
    import persistent_write_evidence_provider as evidence_provider
    import persistent_write_evidence_store as storage
    import persistent_write_fence as fence
    import persistent_write_files as files
    import persistent_write_lifecycle as lifecycle
    import persistent_write_pipeline as pipeline
    import persistent_write_snapshot as snapshot
    import persistent_write_systemd as systemd
else:
    from tools import installed_application_write_gate as temporary
    from tools import persistent_write_commit as commit_backend
    from tools import persistent_write_contract as contract
    from tools import persistent_write_decision as decisions
    from tools import persistent_write_evidence_provider as evidence_provider
    from tools import persistent_write_evidence_store as storage
    from tools import persistent_write_fence as fence
    from tools import persistent_write_files as files
    from tools import persistent_write_lifecycle as lifecycle
    from tools import persistent_write_pipeline as pipeline
    from tools import persistent_write_snapshot as snapshot
    from tools import persistent_write_systemd as systemd

u, s = snapshot.u, snapshot.s
require = contract.temporary.require
HELPERS = tuple(dict.fromkeys(("installed_application_write_gate.py", *temporary.HELPERS,
    "persistent_write_activation_receipt.py", "persistent_write_boot_proof.py", "persistent_write_commit.py",
    "persistent_write_component_view.py", "persistent_write_contract.py", "persistent_write_decision.py",
    "persistent_write_evidence_compatibility.py", "persistent_write_evidence_pins.py",
    "persistent_write_evidence_provider.py", "persistent_write_evidence_store.py", "persistent_write_fence.py",
    "persistent_write_files.py", "persistent_write_gui_failure_receipt.py", "persistent_write_host_receipt.py",
    "persistent_write_lifecycle.py", "persistent_write_pending.py", "persistent_write_pipeline.py",
    "persistent_write_recovery_receipt.py", "persistent_write_retained_snapshot.py",
    "persistent_write_snapshot.py", "persistent_write_systemd.py", "persistent_write_upload_receipt.py")))
ASSETS = ("bindings.json", "new.deb", *HELPERS)
PLAN_KEYS = {"schemaVersion", "phase", "binding", "boot", "started", "backup", "before", "files", "observed", "terminal",
             "cleanupStarted", "disableStarted"}
ARCHIVE_PREFIX = "hardened-blikvm-write-restored-"
EVIDENCE_ENVELOPE_LIMIT = 16 * 1024 * 1024


def boot_identity() -> str:
    return hashlib.sha256(temporary.guard.BOOT_ID.read_bytes()).hexdigest()


def private_file(path: Path, *, limit: int = 32 * 1024 * 1024) -> bytes:
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= limit, "unsafe controller data")
    return path.read_bytes()


def object_value(value: object) -> dict[str, object]:
    require(isinstance(value, dict), "invalid controller object")
    return cast(dict[str, object], value)


def binding(plan: dict[str, object]) -> contract.EvidenceBinding:
    value = object_value(plan["binding"])
    require(set(value) == {"transaction_id", "device_fingerprint", "configuration_sha256", "baseline_slot",
                           "baseline_floor", "baseline_generation"}, "invalid controller binding fields")
    require(all(isinstance(value[key], str) for key in ("transaction_id", "device_fingerprint", "configuration_sha256", "baseline_slot")),
            "invalid controller binding types")
    result = contract.EvidenceBinding(cast(str, value["transaction_id"]), cast(str, value["device_fingerprint"]),
        cast(str, value["configuration_sha256"]), cast(str, value["baseline_slot"]),
        cast(int, value["baseline_floor"]), cast(int, value["baseline_generation"]))
    result.validate()
    return result


def counters(plan: dict[str, object]) -> pipeline.Counters:
    value = object_value(plan["observed"])
    require(set(value) == {"floor", "generation"}, "invalid counter marker")
    result = pipeline.Counters(cast(int, value["floor"]), cast(int, value["generation"]))
    result.validate()
    return result


def validate(root: Path, plan: dict[str, object]) -> None:
    transaction = decisions.validate_root(root)
    require(set(plan) == PLAN_KEYS and type(plan["schemaVersion"]) is int and plan["schemaVersion"] == 1,
            "invalid controller schema")
    contract.Phase(plan["phase"])
    require(type(plan["cleanupStarted"]) is bool and (not plan["cleanupStarted"] or plan["phase"] == contract.Phase.RESTORED.value),
            "invalid cleanup marker")
    require(type(plan["disableStarted"]) is bool and (not plan["disableStarted"]
            or plan["phase"] in (contract.Phase.COMMITTED.value, contract.Phase.DISABLED.value)), "invalid disable marker")
    bound = binding(plan)
    require(bound.transaction_id == transaction, "controller transaction differs")
    observed = counters(plan)
    require(observed.floor >= bound.baseline_floor and observed.generation >= bound.baseline_generation,
            "controller counters regressed")
    require(isinstance(plan["boot"], str) and re.fullmatch(r"[a-f0-9]{64}", plan["boot"]) is not None,
            "invalid controller boot identity")
    started = plan["started"]
    require(type(started) in (int, float) and isinstance(started, (int, float)) and 0 <= started < float("inf"),
            "invalid controller start time")
    object_value(plan["backup"])
    object_value(plan["before"])
    expected = object_value(plan["files"])
    require(set(expected) == {*ASSETS, "controller.py"}, "controller code manifest differs")
    for name, digest in expected.items():
        require(isinstance(digest, str) and hashlib.sha256(private_file(root / name)).hexdigest() == digest,
                "controller code or assets changed")
    if plan["terminal"] is not None:
        value = object_value(plan["terminal"])
        require(set(value) == {"slot", "floor", "generation"} and value["floor"] == observed.floor
                and value["generation"] == observed.generation, "terminal marker counters differ")
        slot = value["slot"]
        require(isinstance(slot, str) and re.fullmatch(r"app-[0-9]{20}-[a-f0-9]{64}", slot) is not None
                and int(slot.split("-", 2)[1]) == observed.floor, "terminal marker slot differs")


def load(root: Path) -> dict[str, object]:
    decisions.validate_root(root)
    value = object_value(json.loads(private_file(root / "plan.json", limit=2 * 1024 * 1024),
        object_pairs_hook=temporary.upload.probe.unique_object))
    validate(root, value)
    return value


def save(root: Path, plan: dict[str, object]) -> None:
    validate(root, plan)
    part = root / ("plan-" + secrets.token_hex(12) + ".part")
    u.new_file(part, json.dumps(plan, sort_keys=True, allow_nan=False).encode())
    os.replace(part, root / "plan.json")
    u.sync_directory(root)


def guard_files(root: Path) -> dict[Path, bytes]:
    name, data = contract.preparer_unit(root)
    return {files.UNIT_ROOT / key: value for key, value in {**contract.guard_units(root), name: data}.items()}


def guard_links(root: Path) -> dict[Path, Path]:
    return {files.UNIT_ROOT / target / (root.name + suffix): files.UNIT_ROOT / (root.name + suffix)
        for target, suffix in (("sysinit.target.wants", "-boot.service"), ("timers.target.wants", "-deadline.timer"))}


def validate_guards(root: Path, *, enabled: bool, allow_missing: bool = False) -> None:
    files.safe_directory(files.UNIT_ROOT)
    for path, data in guard_files(root).items():
        if files.exists(path) or not allow_missing:
            files.safe_file(path, data)
    for path, target in guard_links(root).items():
        files.safe_directory(path.parent)
        if files.exists(path):
            meta = path.lstat()
            require(stat.S_ISLNK(meta.st_mode) and meta.st_uid == 0 and path.readlink() == target, "foreign guard enable link")
        else:
            require(not enabled, "guard enable link missing")


def require_lease(root: Path, plan: dict[str, object]) -> None:
    require(plan["phase"] == contract.Phase.PREPARED.value and plan["boot"] == boot_identity()
            and 0 <= time.monotonic() - cast(float, plan["started"]) < 150
            and decisions.read(root) is None, "preparation lease expired or revoked")
    validate_guards(root, enabled=True)
    require(s.property_value(root.name + "-deadline.timer", "ActiveState") == "active", "deadline not active")
    require(s.property_value(root.name + "-boot.service", "ActiveState") == "active", "boot guard not armed")


def emergency_closed_locked(root: Path, *, boot: bool) -> None:
    """Best-effort closure, not restoration, with the controller lock held.

    Future native retention must hold this SAME lock from runtime/evidence
    verification through decision publication and detachment. There currently
    is no retention CLI. Never make a check-then-close fallback outside it.
    """
    decisions.validate_root(root)
    try:
        winner = decisions.read(root)
    except (OSError, ValueError):
        winner = None  # Preserve damaged decision bytes for manual inspection.
    if winner is not None and winner.outcome == "committed":
        return
    systemd.Admission(root).install_owned_fence(boot=boot)


def emergency_close(root: Path, *, boot: bool) -> None:
    # A failed publication has no immutable winner that would exclude a
    # concurrent acceptance. Acquire the common lock before the fresh check.
    with temporary.guard.locked(root):
        emergency_closed_locked(root, boot=boot)


class Restore:
    def __init__(self, root: Path, plan: dict[str, object]):
        self.root, self.plan = root, plan
        self.admission = systemd.Admission(root)

    def fence_admission(self, *, boot: bool) -> None:
        self.admission.fence_admission(boot=boot)

    def lock(self) -> AbstractContextManager[object]:
        return temporary.guard.locked(self.root)

    def phase(self) -> contract.Phase:
        self.plan = load(self.root)  # Reload after acquiring the preparer's lock.
        return contract.Phase(self.plan["phase"])

    def close_admission(self) -> None:
        self.admission.stop_sockets(boot=False)
        self.admission.require_masked_and_closed()

    def await_terminal_pipeline(self) -> None:
        def remember(value: pipeline.Counters) -> None:
            self.plan["observed"] = asdict(value)
            self.plan["terminal"] = None
            save(self.root, self.plan)
        terminal = pipeline.await_terminal(temporary.observer.observe, remember, observed=counters(self.plan))
        temporary.upload.empty_spool()
        self.plan["terminal"] = asdict(terminal)
        save(self.root, self.plan)

    def remove_owned_configuration(self) -> None:
        files.remove(self.root, object_value(self.plan["backup"]))

    def reload_manager(self) -> None:
        self.admission.reload_manager()

    def reload_and_restart_confirmed(self) -> None:
        self.reload_manager()
        u.checked(["systemctl", "restart", contract.temporary.CURRENT], timeout=30)

    def verify_closed_and_preserved(self) -> None:
        self.admission.require_masked_and_closed()
        if self.plan["terminal"] is None:
            self.await_terminal_pipeline()
        value = object_value(self.plan["terminal"])
        terminal = contract.temporary.TerminalState(cast(str, value["slot"]), cast(int, value["floor"]), cast(int, value["generation"]))
        verify_runtime(self.root, self.plan, terminal, wired=False, masked=True)

    def cancel_socket_jobs_nonblocking(self) -> None:
        self.admission.stop_sockets(boot=True)

    def verify_owned_configuration_absent(self) -> None:
        snapshot.configuration(self.root, object_value(self.plan["backup"]), wired=False)
        before = object_value(self.plan["before"])
        runtime = object_value(before["runtime"])
        require(all(u.digest(path) == runtime[name] for name, path in s.CHECKED_FILES.items()), "boot protected configuration changed")
        require(u.digest(s.UNIT) == before["nginxUnit"], "boot Nginx unit changed")

    def record_restored(self, *, boot: bool) -> None:
        self.plan["phase"] = contract.Phase.RESTORED.value
        if boot:
            self.plan["terminal"] = None  # Post-boot workers may settle newer state.
        save(self.root, self.plan)

    def release_admission_fence(self) -> None:
        self.admission.release_admission_fence(object_value(self.plan["backup"]))

    def disarm_recovery(self) -> None:
        validate_guards(self.root, enabled=False)
        u.checked(["systemctl", "stop", self.root.name + "-deadline.timer"], timeout=15)
        # Keep code, unit definitions and backup for idempotent inspection and
        # explicit cleanup. Never stop the currently running restore service.
        for path in guard_links(self.root):
            if files.exists(path):
                path.unlink()
                u.sync_directory(path.parent)
        self.reload_manager()


def verify_runtime(root: Path, plan: dict[str, object], terminal: contract.temporary.TerminalState, *, wired: bool, masked: bool) -> None:
    deadline = time.monotonic() + 25
    while time.monotonic() < deadline:
        try:
            u.installed(root, "new")
            after = snapshot.capture(root, object_value(plan["backup"]), terminal, wired=wired, masked=masked)
            snapshot.preserve(object_value(plan["before"]), after, new_boot=plan["boot"] != boot_identity(), allow_transition=not wired)
            return
        except (OSError, ValueError, subprocess.SubprocessError):
            time.sleep(0.5)
    raise TimeoutError("persistent write runtime verification failed; guard retained")


def wire(root: Path) -> dict[str, object]:
    name, _ = contract.preparer_unit(root)
    require(any(line.endswith("/" + name) for line in Path("/proc/self/cgroup").read_text().splitlines()), "not running in owned preparer")
    with temporary.guard.locked(root):
        plan = load(root)
        try:
            require_lease(root, plan)
            files.install(root, object_value(plan["backup"]), transaction_only=True)
            systemd.Admission(root).reload_manager()
            require_lease(root, plan)
            files.install(root, object_value(plan["backup"]), transaction_only=False)
            systemd.Admission(root).reload_manager()
            require_lease(root, plan)
            u.checked(["systemctl", "start", *contract.SOCKETS], timeout=20)
            require_lease(root, plan)
            u.checked(["systemctl", "restart", contract.temporary.CURRENT], timeout=30)
            bound = binding(plan)
            verify_runtime(root, plan, contract.temporary.TerminalState(bound.baseline_slot, bound.baseline_floor,
                bound.baseline_generation), wired=True, masked=False)
            require_lease(root, plan)
            plan["phase"] = contract.Phase.WIRED.value
            save(root, plan)
        except (OSError, ValueError, subprocess.SubprocessError):
            try:
                decisions.publish(root, decisions.Decision(binding(plan), "revoked", {}))
                systemd.Admission(root).fence_admission(boot=False)
            finally:
                # An I/O failure publishing revocation must not skip the
                # independent best-effort closure of known write transports.
                emergency_closed_locked(root, boot=False)
            raise
    return {"result": "wired", "guardRoot": str(root), "retentionApproved": False}


def prepare(assets: Path, root: Path, *, application_profile: str = 'seq8') -> dict[str, object]:
    slot, floor, generation, previous = temporary.upload.boot.retained_profile(application_profile)
    contract.guard_units(root)
    require(not files.exists(root) and assets.resolve(strict=True) == assets, "invalid new preparation namespace")
    meta = assets.lstat()
    require(stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0 and stat.S_IMODE(meta.st_mode) == 0o700,
            "unsafe controller assets directory")
    require(not list(Path("/var/lib").glob(contract.PREFIX + "*")), "persistent write guard already exists")
    for path in (*guard_files(root), *guard_links(root)):
        require(not files.exists(path), "guard namespace occupied")
        files.safe_directory(path.parent)
    fence.preflight(root)
    for name in ASSETS:
        private_file(assets / name)
    u.installed(assets, "new")
    temporary.audit_templates()
    before = temporary.upload.boot.retained_snapshot(slot, floor, generation, previous)
    object_value(before["serving"]).pop("serverPid")
    temporary.upload.empty_spool()
    backup = files.preflight(root)
    identity = Path("/etc/machine-id").read_bytes().strip()
    require(re.fullmatch(rb"[a-f0-9]{32}", identity) is not None, "invalid device identity")
    bound = contract.EvidenceBinding(root.name[len(contract.PREFIX):], hashlib.sha256(identity).hexdigest(),
        contract.CONFIG_SHA256, slot, floor, generation)
    root.mkdir(mode=0o700)
    root.chmod(0o700)
    u.sync_directory(root.parent)
    for name in ASSETS:
        u.new_file(root / name, private_file(assets / name))
    u.new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "phase": contract.Phase.PREPARED.value,
        "binding": asdict(bound), "boot": boot_identity(), "started": time.monotonic(), "backup": backup,
        "before": before, "files": {name: u.digest(root / name) for name in (*ASSETS, "controller.py")},
        "observed": asdict(pipeline.Counters(floor, generation)), "terminal": None, "cleanupStarted": False,
        "disableStarted": False}
    save(root, plan)
    # Nothing writable is installed before all recovery files and enable links.
    for path, data in guard_files(root).items():
        u.new_file(path, data, 0o644)
    for path, target in guard_links(root).items():
        path.symlink_to(target)
        u.sync_directory(path.parent)
    admission = systemd.Admission(root)
    admission.reload_manager()
    u.checked(["systemctl", "start", root.name + "-deadline.timer"], timeout=15)
    u.checked(["systemctl", "start", root.name + "-boot.service"], timeout=20)
    require_lease(root, plan)
    # No parent-held transaction lock: the child is its sole wiring owner.
    try:
        u.checked(["systemctl", "start", contract.preparer_unit(root)[0]], timeout=200)
    except (OSError, ValueError, subprocess.SubprocessError):
        emergency_close(root, boot=False)
        raise
    current = load(root)
    require(current["phase"] == contract.Phase.WIRED.value and decisions.read(root) is None, "preparation did not finish")
    admission.require_preparer_quiescent()
    return {"result": "prepared", "guardRoot": str(root), "phase": current["phase"],
        "deadlineArmed": True, "binding": asdict(bound), "retentionApproved": False}


def restore(root: Path, *, boot: bool) -> dict[str, object]:
    if boot:
        require(any(line.endswith("/" + root.name + "-boot.service")
                    for line in Path("/proc/self/cgroup").read_text().splitlines()), "not executing in bound boot guard")
    try:
        plan = load(root)
    except (OSError, ValueError):
        emergency_close(root, boot=boot)
        raise
    require(not plan["cleanupStarted"], "cleanup already started; resume cleanup instead")
    if boot:
        try:
            if plan["phase"] == contract.Phase.PREPARED.value and plan["boot"] == boot_identity() and decisions.read(root) is None:
                snapshot.configuration(root, object_value(plan["backup"]), wired=False)
                return {"result": "armed", "guardRoot": str(root), "retentionApproved": False}
        except (OSError, ValueError):
            emergency_close(root, boot=True)
            raise
    actions = Restore(root, plan)
    try:
        lifecycle.recover(root, actions, binding=binding(plan), boot=boot)
    except (OSError, ValueError, subprocess.SubprocessError):
        emergency_close(root, boot=boot)
        raise  # A closed fence is not evidence of completed restoration.
    winner = decisions.read(root)
    require(winner is not None, "missing recovery decision")
    assert winner is not None
    return {"result": "retained" if winner.outcome == "committed" else "restored", "guardRoot": str(root),
        "boot": boot, "runtimeVerified": not boot and winner.outcome == "revoked", "retentionApproved": winner.outcome == "committed"}


def cleanup(root: Path) -> dict[str, object]:
    """Remove dormant owned units and archive, rather than erase, the backup."""
    with temporary.guard.locked(root):
        plan = load(root)
        winner = decisions.read(root)
        require(winner is not None and winner.outcome == "revoked" and plan["phase"] == contract.Phase.RESTORED.value
                and plan["terminal"] is not None, "verified runtime restoration required before cleanup")
        require(all(not files.exists(path) for path in guard_links(root)), "recovery enable links remain")
        require(not fence.exists(fence.root_for(root)) and all(not fence.exists(fence.UNIT_ROOT / unit)
                for unit in contract.SOCKETS), "runtime admission fence remains")
        validate_guards(root, enabled=False, allow_missing=plan["cleanupStarted"] is True)
        for unit in (root.name + "-prepare.service", root.name + "-restore.service"):
            require(not temporary.observer.unit_busy(unit), "owned recovery/preparer unit still busy")
        timer = temporary.upload.properties(root.name + "-deadline.timer", ("ActiveState", "Job"))
        require(timer["ActiveState"] in ("inactive", "failed") and timer["Job"] in ("", "0"), "deadline timer still busy")
        # Before stopping the boot guard, prove no protected unit retains a
        # Requires edge to it. Otherwise stop propagation could stop the KVM.
        for unit in (*contract.SERVERS, *contract.SOCKETS):
            require(root.name + "-boot.service" not in s.property_value(unit, "Requires").split(),
                    "loaded transaction dependency remains")
        value = object_value(plan["terminal"])
        terminal = contract.temporary.TerminalState(cast(str, value["slot"]), cast(int, value["floor"]), cast(int, value["generation"]))
        verify_runtime(root, plan, terminal, wired=False, masked=False)
        expected = {*ASSETS, "controller.py", "plan.json", "decision.json"}
        require({path.name for path in root.iterdir()} == expected, "unexpected recovery data; manual inspection required")
        destination = root.parent / (ARCHIVE_PREFIX + root.name[len(contract.PREFIX):])
        require(not files.exists(destination), "restoration archive already exists")
        plan["cleanupStarted"] = True
        save(root, plan)  # Durable retry permission before deleting any unit.
        boot_unit = root.name + "-boot.service"
        loaded = s.property_value(boot_unit, "LoadState")
        require(loaded in ("loaded", "not-found"), "unexpected boot guard load state")
        if loaded == "loaded":
            require(s.property_value(boot_unit, "FragmentPath") == str(files.UNIT_ROOT / boot_unit)
                    and s.property_value(boot_unit, "DropInPaths") == "", "boot guard loaded configuration differs")
            u.checked(["systemctl", "stop", boot_unit], timeout=20)
        for path in guard_files(root):
            if files.exists(path):
                path.unlink()
        u.sync_directory(files.UNIT_ROOT)
        systemd.Admission(root).reload_manager()
        verify_runtime(root, plan, terminal, wired=False, masked=False)
        # Metadata and original-absence backup remain recoverable. The archive
        # namespace is deliberately not eligible for controller operations.
        os.rename(root, destination)
        u.sync_directory(destination.parent)
        return {"result": "cleaned", "guardRoot": str(root), "archiveRoot": str(destination),
            "runtimeWritesClosed": True, "floor": terminal.floor, "generation": terminal.generation}


def disable(root: Path) -> dict[str, object]:
    """Permanently close a committed retained policy; keep decision and backup.

    Unlike cleanup this never requires a revoked winner: the immutable committed
    decision is preserved as the historical record, the owned policy files and
    enable links are removed, and the plan is marked disabled for idempotent
    inspection. The dormant guard units remain for explicit later removal.
    """
    with temporary.guard.locked(root):
        plan = load(root)
        winner = decisions.read(root)
        require(winner is not None and winner.outcome == "committed" and winner.binding == binding(plan),
                "committed retained policy required before disable")
        require(plan["cleanupStarted"] is False, "cleanup already started")
        require(plan["phase"] in (contract.Phase.COMMITTED.value, contract.Phase.DISABLED.value),
                "retained policy phase cannot be disabled")
        actions = Restore(root, plan)
        admission = systemd.Admission(root)
        if plan["phase"] != contract.Phase.DISABLED.value:
            plan["disableStarted"] = True
            save(root, plan)  # Durable retry permission before stopping admission.
            admission.install_owned_fence(boot=False)
            actions.await_terminal_pipeline()
            files.remove(root, object_value(plan["backup"]))
            actions.reload_and_restart_confirmed()
            fence.remove(root)
            admission.reload_manager()
            plan["phase"] = contract.Phase.DISABLED.value
            save(root, plan)
        else:
            require(plan["disableStarted"] is True, "invalid disabled policy state")
        temporary.observer.require_closed_admission()
        value = object_value(plan["terminal"])
        terminal = contract.temporary.TerminalState(cast(str, value["slot"]), cast(int, value["floor"]),
            cast(int, value["generation"]))
        verify_runtime(root, plan, terminal, wired=False, masked=False)
        return {"result": "disabled", "guardRoot": str(root), "runtimeWritesClosed": True,
            "floor": terminal.floor, "generation": terminal.generation, "retentionApproved": False}


def evidence(root: Path, *, envelope: bytes | None = None) -> dict[str, object]:
    """Stage and seal original gate receipts for this wired transaction.

    Custody only: bytes are hash-addressed and sealed under the plan binding.
    Semantic qualification stays with the privileged commit provider.
    """
    with temporary.guard.locked(root):
        plan = load(root)
        require(plan["phase"] == contract.Phase.WIRED.value, "evidence requires an undecided wired policy")
        require(decisions.read(root) is None, "policy is already decided")
        bound = binding(plan)
        raw = sys.stdin.buffer.read(EVIDENCE_ENVELOPE_LIMIT + 1) if envelope is None else envelope
        require(0 < len(raw) <= EVIDENCE_ENVELOPE_LIMIT, "invalid evidence envelope size")
        value = object_value(json.loads(raw, object_pairs_hook=temporary.upload.probe.unique_object))
        require(set(value) == {"schemaVersion", "gates"} and type(value["schemaVersion"]) is int
                and value["schemaVersion"] == 1, "invalid evidence envelope schema")
        gates = object_value(value["gates"])
        require(set(gates) == set(contract.REQUIRED_GATES), "complete gate receipts required")
        receipts: dict[str, bytes] = {}
        for gate in sorted(contract.REQUIRED_GATES):
            encoded = gates[gate]
            require(isinstance(encoded, str), "invalid gate receipt encoding")
            text = cast(str, encoded)
            receipt = base64.b64decode(text, validate=True)
            require(base64.b64encode(receipt).decode("ascii") == text, "noncanonical gate receipt encoding")
            require(0 < len(receipt) <= storage.MAX_BYTES, "gate receipt size outside bounds")
            receipts[gate] = receipt
        try:
            store = storage.Store.create(bound)
        except FileExistsError:
            store = storage.Store(bound)
        digests = {gate: hashlib.sha256(receipts[gate]).hexdigest() for gate in sorted(receipts)}
        if "seal.json" not in store.names():
            for gate in sorted(receipts):
                store.put(receipts[gate])
        seal = store.seal(digests)
        return {"result": "sealed", "guardRoot": str(root), "evidenceRoot": str(store.root),
                "sealSha256": seal, "receipts": digests}


def commit(root: Path, *, evidence_seal: str, approve_retention: bool) -> dict[str, object]:
    """Commit a wired policy to permanent admission with sealed gate evidence.

    The commit backend holds the common lock across evidence reading,
    publication and detachment; this action only supplies the operator anchor
    and the explicit approval, and never revokes or repairs a decision.
    """
    require(approve_retention is True, "explicit retention approval required")
    require(re.fullmatch(r"[a-f0-9]{64}", evidence_seal) is not None, "invalid expected evidence seal")

    def provider(root_arg: Path, bound: contract.EvidenceBinding) -> tuple[contract.GateEvidence, ...]:
        return evidence_provider.read_verified_evidence(root_arg, bound, expected_seal=evidence_seal)

    winner = commit_backend.commit(root, provider, retention_approved=True)
    return {"result": "committed", "guardRoot": str(root), "retentionApproved": True,
            "outcome": winner.outcome, "gateReceiptHashes": winner.receipt_hashes}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("prepare", "wire", "restore", "cleanup", "disable", "evidence", "commit"))
    parser.add_argument("--guard-root", type=Path, required=True)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--boot", action="store_true")
    parser.add_argument("--application-profile", choices=('seq8', 'seq9', 'seq10'))
    parser.add_argument("--evidence-seal")
    parser.add_argument("--approve-retention", action="store_true")
    args = parser.parse_args()
    require(os.geteuid() == 0, "root controller required")
    contract.guard_units(args.guard_root)
    require(not args.boot or args.action == "restore", "boot flag is only for restoration")
    require((args.assets is not None) == (args.action == "prepare"), "assets required only for preparation")
    require(args.application_profile is None or args.action == 'prepare', 'profile is only for new preparation')
    require((args.evidence_seal is not None) == (args.action == "commit"), "evidence seal is only for commit")
    require(args.approve_retention == (args.action == "commit"), "retention approval is only for commit")
    u.select_profile("dev7-dev9")
    if args.action == "prepare":
        result = prepare(cast(Path, args.assets), args.guard_root, application_profile=args.application_profile or 'seq8')
    elif args.action == "wire":
        result = wire(args.guard_root)
    elif args.action == "cleanup":
        result = cleanup(args.guard_root)
    elif args.action == "disable":
        result = disable(args.guard_root)
    elif args.action == "evidence":
        result = evidence(args.guard_root)
    elif args.action == "commit":
        result = commit(args.guard_root, evidence_seal=cast(str, args.evidence_seal),
                        approve_retention=args.approve_retention)
    else:
        result = restore(args.guard_root, boot=args.boot)
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
