#!/usr/bin/env python3
"""Exercise real packaged-template activation/confirmation/timeout in isolated roots."""

from __future__ import annotations

import argparse
import grp
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import shutil
import socket
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True
PREFIX = "blikvm-activation-flow-"
PORT = 18482
UNIT_ROOT = Path("/run/systemd/system")
TEMPLATES = {
    "recover": "hardened-blikvm-update-recover.service",
    "current": "hardened-blikvm-server-current.service",
    "candidate": "hardened-blikvm-server-candidate.service",
    "apply-confirmed": "hardened-blikvm-update-apply-confirmed.service",
    "apply-candidate": "hardened-blikvm-update-apply-candidate.service",
    "watchdog": "hardened-blikvm-update-watchdog.service",
    "timer": "hardened-blikvm-update-watchdog.timer",
    "activate": "hardened-blikvm-update-activate@.service",
    "confirm": "hardened-blikvm-update-confirm@.service",
    "socket": "hardened-blikvm-update-confirm.socket",
}


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def command(argv: list[str], *, timeout: int = 25) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout, check=False)


def checked(argv: list[str]) -> bytes:
    result = command(argv)
    require(result.returncode == 0, "command failed: " + " ".join(argv[:4]) + ": " + result.stderr.decode(errors="replace")[:512])
    return result.stdout


def names(token: str) -> dict[str, str]:
    base = PREFIX + token + "-"
    return {"current": base + "current.service", "candidate": base + "candidate.service",
            "recover": base + "recover.service",
            "apply-confirmed": base + "apply-confirmed.service", "apply-candidate": base + "apply-candidate.service",
            "watchdog": base + "watchdog.service", "timer": base + "watchdog.timer",
            "activate": base + "activate@.service", "confirm": base + "confirm@.service", "socket": base + "confirm.socket",
            "cleanup": base + "cleanup.service", "deadline": base + "deadline.timer"}


def render(template: str, key: str, root: Path, runtime: Path, units: dict[str, str]) -> str:
    text = "\n".join(line for line in template.splitlines() if not line.startswith(("StateDirectory", "RuntimeDirectory"))) + "\n"
    text = text.replace("Conflicts=hardened-blikvm-server.service ", "Conflicts=")
    replacements = {source: units[name] for name, source in TEMPLATES.items()}
    replacements.update({
        "/var/lib/hardened-blikvm-slots": str(root / "slots"),
        "/var/lib/hardened-blikvm-update": str(root / "state"),
        "/run/hardened-blikvm-update-confirm": str(runtime),
        "/run/hardened-blikvm-update": str(root / "runtime"),
        "/etc/hardened-blikvm/update-supervisor.json": str(root / "config.json"),
        "/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor": str(root / "supervisor"),
        "/usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter": str(root / "reporter"),
        "BLI_KVM_STATE_DIR=/var/lib/hardened-blikvm": "BLI_KVM_STATE_DIR=" + str(root / "server-state"),
        "127.0.0.1:18080": f"127.0.0.1:{PORT}",
        "SocketBindAllow=tcp:18080": f"SocketBindAllow=tcp:{PORT}",
    })
    for original in sorted(replacements, key=len, reverse=True):
        text = text.replace(original, replacements[original])
    if key in ("current", "candidate"):
        text += f"[Service]\nReadWritePaths={root}/server-state\nRuntimeMaxSec=120\n"
    require("hardened-blikvm-server.service" not in text and "/var/lib/hardened-blikvm-" not in text
            and "/run/hardened-blikvm-update" not in text, "production mutation path remained")
    return text


def cleanup(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid fixture namespace")
    require(not root.is_symlink() and root.stat().st_uid == 0, "unsafe fixture root")
    units = names(root.name.removeprefix(PREFIX))
    for key in ("timer", "deadline", "socket", "apply-candidate", "apply-confirmed", "candidate", "current", "watchdog", "recover"):
        command(["systemctl", "stop", units[key]])
    instances = checked(["systemctl", "list-units", "--all", "--plain", "--no-legend", root.name + "-*@*.service"])
    for line in instances.decode().splitlines():
        name = line.split()[0]
        require(name.startswith(root.name + "-") and name.endswith(".service"), "unexpected fixture instance")
        command(["systemctl", "stop", name])
    for unit in units.values():
        (UNIT_ROOT / unit).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    runtime = Path("/run") / root.name
    if runtime.exists():
        require(not runtime.is_symlink() and {p.name for p in runtime.iterdir()} <= {"confirm.sock"}, "unexpected socket runtime")
        (runtime / "confirm.sock").unlink(missing_ok=True)
        runtime.rmdir()
    shutil.rmtree(root)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--cleanup-root", type=Path)
    args = parser.parse_args()
    require(os.geteuid() == 0, "root orchestration required")
    if args.cleanup_root:
        cleanup(args.cleanup_root)
        return
    assets = cast(Path, args.assets)
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", PORT))
    group = grp.getgrnam("hardened-blikvm").gr_gid
    updater_uid = pwd.getpwnam("hardened-blikvm-update").pw_uid
    server_uid = pwd.getpwnam("hardened-blikvm-server").pw_uid
    token = secrets.token_hex(12)
    root = Path("/var/lib") / (PREFIX + token)
    runtime = Path("/run") / root.name
    units = names(token)
    root.mkdir(mode=0o750)
    root.chmod(0o750)
    os.chown(root, 0, group)
    phase = "prepare"
    facts: dict[str, object] = {}
    receipt: dict[str, object]
    try:
        runtime.mkdir(mode=0o750)
        runtime.chmod(0o750)
        os.chown(runtime, updater_uid, group)
        for name in ("slots", "state", "runtime", "server-state"):
            path = root / name
            path.mkdir(mode=0o750 if name == "slots" else 0o700)
            path.chmod(0o750 if name == "slots" else 0o700)
            os.chown(path, server_uid if name == "server-state" else updater_uid, group)
        for name in ("supervisor", "reporter"):
            shutil.copyfile(assets / name, root / name)
            (root / name).chmod(0o755)
        shutil.copyfile(Path(__file__), root / "cleanup.py")
        fixture = cast(dict[str, object], json.loads((assets / "fixture.json").read_bytes()))
        config = {"schemaVersion": 1, "product": "Hardened BliKVM", "target": "bli-h616-pikvm",
                  "keyId": fixture["keyId"], "publicKeyHex": fixture["publicKeyHex"],
                  "slotRoot": str(root / "slots"), "stateRoot": str(root / "state"),
                  "runtimeRoot": str(root / "runtime"), "systemSequence": 1, "healthTimeoutMs": 10000}
        (root / "config.json").write_text(json.dumps(config))
        (root / "config.json").chmod(0o600)

        def run(action: str, *values: str) -> dict[str, object]:
            name = "blikvm-flow-command-" + secrets.token_hex(10) + ".service"
            argv = ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + name,
                    "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm", "--property=UMask=0077",
                    "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict",
                    "--property=ReadWritePaths=" + " ".join(str(root / name) for name in ("slots", "state", "runtime")),
                    "--property=LoadCredential=update-supervisor-config:" + str(root / "config.json"),
                    "--property=RuntimeMaxSec=20", str(root / "supervisor"), "--config",
                    "/run/credentials/" + name + "/update-supervisor-config", action, *values]
            return cast(dict[str, object], json.loads(checked(argv)))

        slots: list[str] = []
        for sequence in (1, 2, 3):
            path = root / f"application-{sequence}.tar"
            shutil.copyfile(assets / path.name, path)
            path.chmod(0o640)
            os.chown(path, 0, group)
            slots.append(cast(str, run("stage", str(path))["slotId"]))
        run("initialize", slots[0])
        for key, filename in TEMPLATES.items():
            (UNIT_ROOT / units[key]).write_text(render((assets / filename).read_text(), key, root, runtime, units))
        (UNIT_ROOT / units["cleanup"]).write_text(f"[Service]\nType=oneshot\nExecStart=/usr/bin/python3 -B {root}/cleanup.py --cleanup-root {root}\n")
        (UNIT_ROOT / units["deadline"]).write_text(f"[Timer]\nOnActiveSec=100\nUnit={units['cleanup']}\n")
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", units["deadline"], units["socket"], units["timer"], units["current"]])

        def prop(key: str, field: str) -> str:
            return checked(["systemctl", "show", "-p", field, "--value", units[key]]).decode().strip()

        def healthy() -> bool:
            response = command(["curl", "--noproxy", "*", "--silent", "--fail", "--max-time", "2", f"http://127.0.0.1:{PORT}/api/health"])
            return response.returncode == 0 and b'"status":"ok"' in response.stdout

        def await_current(slot: str, floor: int) -> dict[str, object]:
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                status = run("status")
                pid = prop("current", "MainPID")
                expected = root / "slots" / slot / "bin/bli-kvm-server"
                if (status.get("pending") is None and status.get("confirmedSlotId") == slot
                        and status.get("floorSequence") == floor and prop("current", "ActiveState") == "active"
                        and pid.isdecimal() and int(pid) > 0 and (Path("/proc") / pid / "exe").resolve(strict=False) == expected
                        and not (root / "runtime/confirmed-apply-request.json").exists() and healthy()):
                    return status
                time.sleep(0.25)
            raise ValueError("confirmed server state did not converge")

        phase = "initial"
        await_current(slots[0], 1)
        # Test the mandatory dependency with the timer stopped so it cannot
        # repair state or hide which service prevented application execution.
        checked(["systemctl", "stop", units["timer"], units["watchdog"], units["recover"]])
        phase = "missing-trust-blocks-server"
        config_path = root / "config.json"
        saved_config = root / "config.saved"
        config_path.rename(saved_config)
        try:
            attempted = command(["systemctl", "start", units["current"]])
            require(attempted.returncode != 0 and prop("current", "MainPID") == "0"
                    and prop("recover", "Result") != "success", "missing trust did not block serving")
        finally:
            saved_config.rename(config_path)
        facts["missingTrustBlockedServing"] = True
        # A dependency-failed current service may have been garbage-collected;
        # only the failed recovery service needs its start limit reset.
        checked(["systemctl", "reset-failed", units["recover"]])
        phase = "tampered-web-blocks-server"
        web = root / "slots" / slots[0] / "share/web/index.html"
        original_web = web.read_bytes()
        # Alter only fixture web bytes, not the executable: without mandatory
        # verification the same Rust server would still be able to start.
        web.write_bytes(b"X" + original_web[1:])
        try:
            attempted = command(["systemctl", "start", units["current"]])
            require(attempted.returncode != 0 and prop("current", "MainPID") == "0"
                    and prop("recover", "Result") != "success", "tampered signed tree did not block serving")
        finally:
            web.write_bytes(original_web)
        facts["tamperedTreeBlockedServing"] = True
        checked(["systemctl", "reset-failed", units["recover"]])
        checked(["systemctl", "start", units["current"], units["timer"]])
        await_current(slots[0], 1)
        require(prop("recover", "ActiveState") == "active", "verified recovery gate did not remain active")
        phase = "candidate-confirmation"
        checked(["systemctl", "start", units["activate"].replace("@.", "@" + slots[1] + ".")])
        await_current(slots[1], 2)
        facts["realCandidateConfirmed"] = True
        facts["oneUseTokenConsumed"] = not (root / "runtime/health-token").exists()
        require(facts["oneUseTokenConsumed"] is True, "confirmed token remained")
        pid = prop("current", "MainPID")
        time.sleep(4.3)
        require(prop("current", "MainPID") == pid, "clean ticks restarted confirmed server")
        phase = "failed-candidate-timeout"
        checked(["systemctl", "start", units["activate"].replace("@.", "@" + slots[2] + ".")])
        final = await_current(slots[1], 2)
        require(final.get("lastRollback") == "health_timeout", "wrong recovery reason")
        restored_pid = prop("current", "MainPID")
        time.sleep(4.3)
        require(prop("current", "MainPID") == restored_pid, "clean ticks restarted restored server")
        require(prop("candidate", "ActiveState") in ("inactive", "failed"), "candidate remained active")
        facts.update({"timeoutRollbackPassed": True, "floorStayedAtTwo": True, "restoredServerHealthy": True,
                      "cleanTicksStable": True, "productionServicesTouched": False})
        receipt = {"result": "passed", "checks": facts}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        journals: dict[str, str] = {}
        for key in ("candidate", "current", "apply-candidate", "apply-confirmed", "watchdog", "recover"):
            log = command(["journalctl", "--no-pager", "-n", "6", "-o", "cat", "-u", units[key]])
            journals[key] = log.stdout.decode(errors="replace")[-1800:]
        receipt = {"result": "failed", "phase": phase, "errorType": type(error).__name__, "checks": facts, "journals": journals}
    finally:
        cleanup(root)
    receipt["fixtureRemoved"] = True
    print(json.dumps(receipt, sort_keys=True))
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
