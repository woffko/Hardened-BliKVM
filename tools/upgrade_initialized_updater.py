#!/usr/bin/env python3
"""Native, profile-bound updater replacement with retained status preserved."""

from __future__ import annotations

import argparse
import fcntl
import json
import os
from pathlib import Path
import re
import secrets
import stat
import subprocess
import sys
from typing import cast

sys.dont_write_bytecode = True
if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent))
    import switch_signed_slot_server as serving
else:
    from tools import switch_signed_slot_server as serving

require = serving.require
checked = serving.checked
digest = serving.digest
new_file = serving.new_file
sync_directory = serving.sync_directory
PREFIX = "hardened-blikvm-updater-upgrade-"
PACKAGE = "hardened-blikvm-update-runtime"
PACKAGE_HASHES = {
    "old": "1634b7e42ff7ef8f10894b212bf635c6e23851d58bab5f69096705aed52cb939",
    "new": "41cef0e7c5d8839a343ba2bf6a16f255e250659510f3cdb89b7f38870202e2a1",
}
SUPERVISOR_HASHES = {
    "old": "2a1fe1ca3b255c6b93b8b89b8cb2f85476fbed7453344cb0ec60fbe64033c181",
    "new": "120add7f7086164709d5b1f9c5be7280b060a7c86e537741aa060bc85052471b",
}
VERSIONS = {"old": "0.1.0~dev6", "new": "0.1.0~dev7"}
selected_profile = "dev6-dev7"
PROFILES = ("dev6-dev7", "dev7-dev9")
LEGACY_BINDINGS = (dict(PACKAGE_HASHES), dict(SUPERVISOR_HASHES), dict(VERSIONS))
SEQ6_SLOT = "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac"
HELPERS = ("switch_signed_slot_server.py", "initialize_application_state.py", "application_bootstrap_roots.py",
           "enable_update_services.py", "upgrade_initialized_updater.py", "status_api_wiring.py",
           "persistent_status_wiring.py", "persistent_status_contract.py")
STATE_ROOTS = (Path("/var/lib/hardened-blikvm-slots"), Path("/var/lib/hardened-blikvm-update"))
UNIT_ROOT = Path("/etc/systemd/system")


def select_profile(name: str) -> None:
    """Select a fixed CLI profile before orchestration; legacy imports stay unchanged."""
    global selected_profile
    require(name in PROFILES, "unknown upgrade profile")
    selected_profile = name
    packages, supervisors, versions = LEGACY_BINDINGS
    if name == "dev6-dev7":
        PACKAGE_HASHES.update(packages)
        SUPERVISOR_HASHES.update(supervisors)
        VERSIONS.update(versions)
    else:
        PACKAGE_HASHES.update({"old": packages["new"], "new": "15f744ed21f4029cc5d913fd66a98d3e029ba19be0b3c60a6fe62fb2b0bd556d"})
        SUPERVISOR_HASHES.update({"old": supervisors["new"], "new": "2a524958c5e20f859213935553b5b0c02e9dc6b33e88fef066666153684e3ac0"})
        VERSIONS.update({"old": "0.1.0~dev7", "new": "0.1.0~dev9"})


def write_transports_inert() -> None:
    for name in ("upload", "activation"):
        socket = f"hardened-blikvm-update-{name}.socket"
        load_state = serving.property_value(socket, "LoadState")
        require(load_state in ("loaded", "not-found"), "unexpected write socket load state")
        serving.inactive(socket)
        require(serving.enabled(socket) == "disabled" if load_state == "loaded" else
                serving.enabled(socket) in ("", "not-found"), "write socket unexpectedly enabled")
        endpoint = Path(f"/run/hardened-blikvm-update-api/{name}.sock")
        require(not endpoint.exists() and not endpoint.is_symlink(), "write socket unexpectedly present")
        workers = checked(["systemctl", "list-units", "--all", "--plain", "--no-legend",
                           "--state=active,activating,deactivating,reloading", f"hardened-blikvm-update-{name}@*.service"])
        require(not workers.strip(), "write worker unexpectedly active")


def tree_fingerprint(roots: tuple[Path, ...]) -> dict[str, object]:
    result: dict[str, object] = {}
    for root in roots:
        require(root.is_dir() and not root.is_symlink(), "unsafe state root")
        paths = [root, *sorted(root.rglob("*"))]
        require(len(paths) <= 5000, "state snapshot too large")
        for path in paths:
            meta = path.lstat()
            entry: dict[str, object] = {"mode": meta.st_mode, "uid": meta.st_uid, "gid": meta.st_gid,
                                        "device": meta.st_dev, "inode": meta.st_ino}
            if stat.S_ISREG(meta.st_mode):
                require(meta.st_nlink == 1 and meta.st_size <= 32 * 1024 * 1024, "unsafe state file")
                entry["sha256"] = digest(path)
            elif stat.S_ISLNK(meta.st_mode):
                entry["target"] = os.readlink(path)
            else:
                require(stat.S_ISDIR(meta.st_mode), "unexpected state inode")
            result[str(path)] = entry
    return result


def package_binding(assets: Path, variant: str) -> dict[str, object]:
    require(variant in VERSIONS, "invalid package selection")
    require(digest(assets / (variant + ".deb")) == PACKAGE_HASHES[variant], "package hash mismatch")
    # This public map is authored by the reviewed host package validator, then
    # retained beneath the root-owned guard. It never selects an install path.
    decoded = cast(dict[str, object], json.loads((assets / "bindings.json").read_bytes()))
    binding = cast(dict[str, object], decoded[variant])
    require(binding.get("sha256") == PACKAGE_HASHES[variant] and binding.get("version") == VERSIONS[variant],
            "package binding mismatch")
    files = cast(dict[str, list[object]], binding["files"])
    require(1 <= len(files) <= 64, "invalid installed file map")
    for name, values in files.items():
        path = Path(name)
        require(path.is_absolute() and ".." not in path.parts and len(values) == 2,
                "unsafe installed file binding")
        require(re.fullmatch(r"[a-f0-9]{64}", cast(str, values[0])) is not None
                and values[1] in (0o644, 0o755), "invalid installed file binding")
    require(files[str(serving.bootstrap.SUPERVISOR)][0] == SUPERVISOR_HASHES[variant], "supervisor binding mismatch")
    return binding


def installed(assets: Path, variant: str) -> None:
    binding = package_binding(assets, variant)
    status = checked(["dpkg-query", "-W", "-f=${db:Status-Abbrev}:${Version}", PACKAGE]).decode()
    require(status == "ii :" + VERSIONS[variant], "installed version mismatch")
    for name, values in cast(dict[str, list[object]], binding["files"]).items():
        path = Path(name)
        meta = path.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_gid == 0
                and meta.st_nlink == 1 and stat.S_IMODE(meta.st_mode) == values[1]
                and digest(path) == values[0], "installed package file mismatch")
    other = "new" if variant == "old" else "old"
    bindings = cast(dict[str, dict[str, object]], json.loads((assets / "bindings.json").read_bytes()))
    extra_paths = set(cast(dict[str, object], bindings[other]["files"])) - set(cast(dict[str, object], binding["files"]))
    require(all(not Path(name).exists() and not Path(name).is_symlink() for name in extra_paths), "other package version left an extra file")
    require(not checked(["dpkg", "--audit"]).strip(), "dpkg audit is not clean")
    require(not checked(["getcap", str(serving.bootstrap.SUPERVISOR),
                         "/usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter"]).strip(), "unexpected capabilities")


def status_transport_inert() -> None:
    socket = "hardened-blikvm-update-status.socket"
    serving.inactive(socket)
    require(serving.enabled(socket) not in ("enabled", "enabled-runtime", "linked", "linked-runtime"),
            "status socket unexpectedly enabled")
    require(not Path("/run/hardened-blikvm-update-api/status.sock").exists()
            and not Path("/run/hardened-blikvm-update-api/status.sock").is_symlink(), "status socket unexpectedly present")
    instances = checked(["systemctl", "list-units", "--all", "--plain", "--no-legend",
                         "--state=active,activating,deactivating,reloading", "hardened-blikvm-update-status@*.service"])
    require(not instances.strip(), "status worker unexpectedly active")


def snapshot(variant: str) -> dict[str, object]:
    # Lazy import avoids a module-initialization cycle; the guard carries both
    # this module and its operational observer as reviewed local helpers.
    if not __package__:
        import status_api_wiring as status_api
        import persistent_status_wiring as persistent
    else:
        from tools import status_api_wiring as status_api
        from tools import persistent_status_wiring as persistent
    persistent.validate_dropins(True)
    if selected_profile == "dev7-dev9":
        write_transports_inert()
        value = status_api.snapshot(True, expected_version=VERSIONS[variant], expected_supervisor_sha=SUPERVISOR_HASHES[variant],
                                    expected_slot=SEQ6_SLOT, expected_floor=6, expected_generation=31)
        # A package-only replacement must not restart the retained server.
        cast(dict[str, object], value["serving"])["serverPid"] = serving.property_value(serving.CURRENT_SERVER, "MainPID")
    else:
        value = status_api.snapshot(True, expected_version=VERSIONS[variant], expected_supervisor_sha=SUPERVISOR_HASHES[variant])
    value["persistentDropinHashes"] = {str(path): digest(path) for path in persistent.dropins()}
    return value


def verify(root: Path, plan: dict[str, object], variant: str, boot: bool = False) -> None:
    installed(root, variant)
    before = cast(dict[str, object], plan["before"])
    if boot:
        require(tree_fingerprint(STATE_ROOTS) == before["state"], "persistent update state changed")
        runtime = cast(dict[str, str], before["runtime"])
        require(all(digest(path) == runtime[name] for name, path in serving.CHECKED_FILES.items()),
                "preserved configuration changed")
        require(digest(serving.UNIT) == before["nginxUnit"], "local Nginx unit changed")
        require(all(digest(Path(name)) == value for name, value in cast(dict[str, str], before["persistentDropinHashes"]).items()),
                "persistent status wiring changed")
    else:
        require(snapshot(variant) == before, "serving runtime or persistent state changed")


def guard_units(root: Path) -> dict[str, str]:
    common = "[Service]\nType=oneshot\nUser=root\nUMask=0077\nTimeoutStartSec=180\nNoNewPrivileges=yes\nProtectHome=yes\n"
    command = f"ExecStart=/usr/bin/python3 -B {root}/controller.py rollback --guard-root {root}"
    # The legacy infrastructure controller shares these units but has no
    # profile argument. Preserve its original command and default semantics.
    if selected_profile != "dev6-dev7":
        command += f" --profile {selected_profile}"
    return {
        root.name + "-boot.service": ("[Unit]\nDefaultDependencies=no\nAfter=local-fs.target\n"
            f"Before=multi-user.target {serving.CURRENT_SERVER} {serving.CANDIDATE_SERVER}\n"
            + common + command + " --boot\n[Install]\nWantedBy=sysinit.target\n"),
        root.name + "-rollback.service": "[Unit]\nDescription=Updater package deadline recovery\n" + common + command + "\n",
        root.name + "-deadline.timer": f"[Timer]\nOnActiveSec=600\nUnit={root.name}-rollback.service\n[Install]\nWantedBy=timers.target\n",
    }


def save(root: Path, plan: dict[str, object]) -> None:
    temporary = root / "plan.json.part"
    if temporary.exists() or temporary.is_symlink():
        meta = temporary.lstat()
        require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
                and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024,
                "unsafe incomplete plan")
        partial = temporary.read_bytes()
        require(any(json.dumps({**plan, "phase": phase}, sort_keys=True).encode().startswith(partial)
                    for phase in ("prepared", "applied", "rolled_back", "committed")), "unknown incomplete plan")
        temporary.unlink()
        sync_directory(root)
    new_file(temporary, json.dumps(plan, sort_keys=True).encode())
    os.replace(temporary, root / "plan.json")
    sync_directory(root)


def arm(root: Path) -> None:
    for name, content in guard_units(root).items():
        new_file(UNIT_ROOT / name, content.encode(), 0o644)
    checked(["systemctl", "daemon-reload"])
    checked(["systemctl", "enable", root.name + "-boot.service"])
    checked(["systemctl", "enable", "--now", root.name + "-deadline.timer"])
    checked(["systemctl", "is-active", "--quiet", root.name + "-deadline.timer"])
    os.sync()


def disarm(root: Path, boot: bool) -> None:
    for name, content in guard_units(root).items():
        path = UNIT_ROOT / name
        if path.exists() or path.is_symlink():
            require(not path.is_symlink() and path.read_bytes() == content.encode(), "guard unit changed")
    for suffix in ("-deadline.timer", "-boot.service"):
        if (UNIT_ROOT / (root.name + suffix)).exists():
            if suffix.endswith("timer"):
                checked(["systemctl", "stop", "--no-block", root.name + suffix] if boot
                        else ["systemctl", "stop", root.name + suffix])
            checked(["systemctl", "disable", root.name + suffix])
    for name in guard_units(root):
        (UNIT_ROOT / name).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    sync_directory(UNIT_ROOT)


def install(root: Path, variant: str) -> None:
    package_binding(root, variant)
    # Exact pinned packages have no maintainer scripts and do not start units.
    result = subprocess.run(["dpkg", "--force-confold", "-i", str(root / (variant + ".deb"))],
                            stdin=subprocess.DEVNULL, capture_output=True, timeout=120, check=False)
    require(result.returncode == 0, "bound package installation failed")
    checked(["systemctl", "daemon-reload"])
    installed(root, variant)


def guard_root(value: str) -> Path:
    root = Path(value)
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid guard namespace")
    meta = root.lstat()
    require(root.resolve(strict=True) == root and stat.S_ISDIR(meta.st_mode) and meta.st_uid == 0
            and stat.S_IMODE(meta.st_mode) == 0o700, "unsafe guard root")
    return root


def load_plan(root: Path) -> dict[str, object]:
    path = root / "plan.json"
    meta = path.lstat()
    require(stat.S_ISREG(meta.st_mode) and meta.st_uid == 0 and meta.st_nlink == 1
            and stat.S_IMODE(meta.st_mode) == 0o600 and meta.st_size <= 2 * 1024 * 1024, "unsafe plan")
    plan = cast(dict[str, object], json.loads(path.read_bytes()))
    require(plan.get("schemaVersion") == 1 and plan.get("phase") in ("prepared", "applied", "rolled_back", "committed"),
            "invalid plan")
    require(plan.get("profile", "dev6-dev7") == selected_profile, "guard profile mismatch")
    return plan


def prepare(assets: Path) -> dict[str, object]:
    require(not list(Path("/var/lib").glob(PREFIX + "*")) and not list(UNIT_ROOT.glob(PREFIX + "*")),
            "previous updater guard requires inspection")
    installed(assets, "old")
    package_binding(assets, "new")
    before = snapshot("old")
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o700)
    for name in ("old.deb", "new.deb", "bindings.json", *HELPERS):
        new_file(root / name, (assets / name).read_bytes())
    new_file(root / "controller.py", Path(__file__).read_bytes())
    plan: dict[str, object] = {"schemaVersion": 1, "profile": selected_profile, "phase": "prepared", "before": before}
    save(root, plan)
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        arm(root)
        install(root, "new")
        verify(root, plan, "new")
        plan["phase"] = "applied"
        save(root, plan)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        # Leave the already armed guard available for independent recovery.
        # No command output or package contents enter the public failure receipt.
        return {"result": "failed", "guardRoot": str(root), "errorType": type(error).__name__}
    finally:
        os.close(descriptor)
    return {"result": "prepared", "guardRoot": str(root), "stateAndServingPreserved": True}


def transition(action: str, root: Path, boot: bool) -> dict[str, object]:
    descriptor = os.open(root, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        plan = load_plan(root)
        if action == "rollback":
            variant = "new" if plan["phase"] == "committed" else "old"
            if variant == "old":
                install(root, "old")
            verify(root, plan, variant, boot)
            if variant == "old":
                plan["phase"] = "rolled_back"
                save(root, plan)
            disarm(root, boot)
        elif action == "commit":
            require(plan["phase"] in ("applied", "committed"), "upgrade is not ready for retention")
            verify(root, plan, "new")
            plan["phase"] = "committed"
            save(root, plan)
            disarm(root, False)
        else:
            variant = "new" if plan["phase"] in ("applied", "committed") else "old"
            verify(root, plan, variant)
            if action == "cleanup":
                require(plan["phase"] in ("rolled_back", "committed"), "guard is still pending")
                disarm(root, False)
                expected = {"controller.py", "bindings.json", "old.deb", "new.deb", "plan.json", *HELPERS}
                require({path.name for path in root.iterdir()} == expected, "unexpected guard contents")
                for path in root.iterdir():
                    require(path.is_file() and not path.is_symlink() and path.stat().st_uid == 0, "unsafe guard artifact")
                for name in expected:
                    (root / name).unlink()
                root.rmdir()
                sync_directory(root.parent)
        return {"result": action, "phase": plan["phase"], "guardRoot": str(root),
                "stateAndServingPreserved": True, "bootRecovery": boot}
    finally:
        os.close(descriptor)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "rollback", "commit", "cleanup"))
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--guard-root")
    parser.add_argument("--boot", action="store_true")
    parser.add_argument("--variant", choices=("old", "new"), default="old")
    parser.add_argument("--profile", choices=PROFILES, default="dev6-dev7")
    args = parser.parse_args()
    select_profile(args.profile)
    require(os.geteuid() == 0, "root orchestration required")
    require(not args.boot or args.action == "rollback", "boot is rollback-only")
    if args.action == "preflight":
        installed(args.assets, args.variant)
        package_binding(args.assets, "new")
        result = {"result": "preflight", "before": snapshot(args.variant)}
    elif args.action == "prepare":
        result = prepare(args.assets)
    else:
        result = transition(args.action, guard_root(args.guard_root), args.boot)
    print(json.dumps(result, sort_keys=True))
    if result.get("result") == "failed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
