#!/usr/bin/env python3
"""Native isolated systemd test of confirmed-apply requests and acknowledgements."""

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
import subprocess
import sys
import time
from typing import cast

sys.dont_write_bytecode = True


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def command(argv: list[str], payload: bytes | bytearray | None = None, timeout: int = 25) -> bytes:
    result = subprocess.run(argv, input=payload, stdin=subprocess.DEVNULL if payload is None else None,
                            capture_output=True, timeout=timeout, check=False)
    require(result.returncode == 0, "command failed: " + argv[0] + ": " + result.stderr.decode(errors="replace")[:1024])
    return result.stdout


def names(token: str) -> dict[str, str]:
    return {key: "blikvm-apply-events-" + token + "-" + key + suffix for key, suffix in (
        ("current", ".service"), ("apply", ".service"), ("watchdog", ".service"),
        ("timer", ".timer"), ("deadline", ".timer"), ("cleanup", ".service"),
    )}


def cleanup(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(r"blikvm-apply-events-[a-f0-9]{24}", root.name) is not None,
            "invalid fixture root")
    require(not root.is_symlink() and root.stat().st_uid == 0, "unsafe fixture root")
    units = names(root.name.removeprefix("blikvm-apply-events-"))
    for key in ("timer", "deadline", "watchdog", "apply", "current"):
        subprocess.run(["systemctl", "stop", units[key]], capture_output=True, timeout=35, check=False)
    for unit in units.values():
        (Path("/run/systemd/system") / unit).unlink(missing_ok=True)
    command(["systemctl", "daemon-reload"])
    # Contents belong only to this root-created isolated fixture; no production
    # path or user-selected recursion target is accepted by the cleanup CLI.
    shutil.rmtree(root)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--cleanup-root", type=Path)
    args = parser.parse_args()
    require(os.geteuid() == 0, "root test orchestration required")
    if args.cleanup_root:
        cleanup(cast(Path, args.cleanup_root))
        return
    assets = cast(Path, args.assets)
    output = cast(Path, args.output)
    require(output.parent == Path("/tmp") and not output.exists(), "new temporary receipt required")
    fixture = cast(dict[str, object], json.loads((assets / "fixture.json").read_bytes()))
    account = pwd.getpwnam("hardened-blikvm-update")
    group = grp.getgrnam("hardened-blikvm").gr_gid
    token = secrets.token_hex(12)
    root = Path("/var/lib") / ("blikvm-apply-events-" + token)
    root.mkdir(mode=0o750)
    root.chmod(0o750)
    os.chown(root, 0, group)
    units = names(token)
    phase = "prepare"
    facts: dict[str, object] = {"runMountNoexec": bool(os.statvfs("/run").f_flag & os.ST_NOEXEC)}
    receipt: dict[str, object]
    try:
        for name in ("slots", "state", "runtime"):
            (root / name).mkdir(mode=0o700)
            os.chown(root / name, account.pw_uid, group)
        binary = root / "supervisor"
        shutil.copyfile(assets / "supervisor", binary)
        binary.chmod(0o755)
        shutil.copyfile(Path(__file__), root / "cleanup.py")
        # Metadata only: never read either credential or print its contents.
        credential_probe = root / "credential-metadata.py"
        credential_probe.write_text(
            "import json,os,pathlib,sys\n"
            "directory=pathlib.Path('/run/hardened-blikvm-apply-credentials')/sys.argv[1]\n"
            "facts={'euid':os.geteuid(),'paths':{}}\n"
            "for path in [pathlib.Path('/'),pathlib.Path('/run'),directory.parent,directory,directory/'update-supervisor-config',directory/'confirmed-apply-request']:\n"
            " try:\n"
            "  s=path.lstat(); v=os.statvfs(path)\n"
            "  facts['paths'][str(path)]={'uid':s.st_uid,'mode':oct(s.st_mode),'links':s.st_nlink,'dev':s.st_dev,'ro':bool(v.f_flag & os.ST_RDONLY),'canonical':path.resolve()==path}\n"
            " except OSError as e: facts['paths'][str(path)]={'errno':e.errno}\n"
            "print('credential metadata '+json.dumps(facts,sort_keys=True))\n"
        )
        credential_probe.chmod(0o644)
        for sequence in (1, 2, 3):
            path = root / f"application-{sequence}.tar"
            shutil.copyfile(assets / path.name, path)
            path.chmod(0o640)
            os.chown(path, 0, group)
        config = root / "config.json"
        config.write_text(json.dumps({
            "schemaVersion": 1, "product": "Hardened BliKVM", "target": "bli-h616-pikvm",
            "keyId": fixture["keyId"], "publicKeyHex": fixture["publicKeyHex"],
            "slotRoot": str(root / "slots"), "stateRoot": str(root / "state"),
            "runtimeRoot": str(root / "runtime"), "systemSequence": 1, "healthTimeoutMs": 60000,
        }))
        config.chmod(0o600)

        def run(action: str, *values: str, payload: bytes | bytearray | None = None) -> dict[str, object]:
            unit = "blikvm-apply-command-" + secrets.token_hex(10) + ".service"
            argv = ["systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + unit,
                    "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm",
                    "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict",
                    "--property=ReadWritePaths=" + " ".join(str(root / name) for name in ("slots", "state", "runtime")),
                    "--property=LoadCredential=update-supervisor-config:" + str(config),
                    "--property=RuntimeMaxSec=20", str(binary), "--config",
                    "/run/credentials/" + unit + "/update-supervisor-config", action, *values]
            return cast(dict[str, object], json.loads(command(argv, payload)))

        slots = [cast(str, run("stage", str(root / f"application-{sequence}.tar"))["slotId"]) for sequence in (1, 2, 3)]
        run("initialize", slots[0])
        replacements = {
            "/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor": str(binary),
            "/etc/hardened-blikvm/update-supervisor.json": str(config),
            "/var/lib/hardened-blikvm-slots": str(root / "slots"),
            "/var/lib/hardened-blikvm-update": str(root / "state"),
            "/run/hardened-blikvm-update": str(root / "runtime"),
            "hardened-blikvm-update-apply-confirmed.service": units["apply"],
            "hardened-blikvm-update-watchdog.service": units["watchdog"],
            "hardened-blikvm-server-current.service": units["current"],
        }
        for source_name, key in (("apply.service", "apply"), ("watchdog.service", "watchdog"), ("watchdog.timer", "timer")):
            text = (assets / source_name).read_text()
            text = "\n".join(line for line in text.splitlines() if not line.startswith(("StateDirectory", "RuntimeDirectory"))) + "\n"
            for original, replacement in replacements.items():
                text = text.replace(original, replacement)
            if key == "apply":
                text = text.replace("ExecStartPost=", f"ExecStartPost=/usr/bin/python3 -B {credential_probe} %n\nExecStartPost=", 1)
            require("/var/lib/hardened-blikvm" not in text and "/run/hardened-blikvm-update" not in text,
                    "production path remained in fixture unit")
            (Path("/run/systemd/system") / units[key]).write_text(text)
        stub = "[Unit]\nDescription=Isolated apply counter process\n[Service]\nUser=hardened-blikvm-server\nGroup=hardened-blikvm\nType=simple\nExecStart=/usr/bin/sleep 180\nRuntimeMaxSec=120\nNoNewPrivileges=yes\n"
        stub_path = Path("/run/systemd/system") / units["current"]
        stub_path.write_text(stub)
        (Path("/run/systemd/system") / units["cleanup"]).write_text(
            f"[Service]\nType=oneshot\nExecStart=/usr/bin/python3 -B {root}/cleanup.py --cleanup-root {root}\n")
        (Path("/run/systemd/system") / units["deadline"]).write_text(
            f"[Timer]\nOnActiveSec=100\nUnit={units['cleanup']}\n")
        command(["systemctl", "daemon-reload"])
        command(["systemctl", "start", units["deadline"], units["current"], units["timer"]])

        def prop(key: str, name: str) -> str:
            return command(["systemctl", "show", "-p", name, "--value", units[key]]).decode().strip()

        def stable_ticks(pid: str) -> int:
            invocations: set[str] = set()
            for _ in range(5):
                time.sleep(2.1)
                require(prop("current", "MainPID") == pid, "clean watchdog tick restarted test process")
                timestamp = prop("watchdog", "ExecMainStartTimestampMonotonic")
                if timestamp.isdecimal() and int(timestamp) > 0:
                    invocations.add(timestamp)
            require(len(invocations) >= 3, "watchdog did not execute repeatedly")
            return len(invocations)

        phase = "clean-ticks"
        first_pid = prop("current", "MainPID")
        require(first_pid.isdecimal() and int(first_pid) > 0, "test process failed to start")
        facts["initialCleanTicks"] = stable_ticks(first_pid)
        phase = "confirm"
        run("activate", slots[1])
        token_bytes = bytearray((root / "runtime/health-token").read_bytes())
        try:
            run("confirm-stdin", payload=token_bytes)
        finally:
            token_bytes[:] = b"\0" * len(token_bytes)
        request = root / "runtime/confirmed-apply-request.json"
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline and (request.exists() or prop("current", "MainPID") == first_pid):
            time.sleep(0.2)
        second_pid = prop("current", "MainPID")
        require(second_pid not in ("0", first_pid) and not request.exists(), "confirmed application was not acknowledged")
        facts["afterConfirmationCleanTicks"] = stable_ticks(second_pid)
        phase = "failed-apply-preserves-request"
        stub_path.write_text(stub.replace("Type=simple\n", "Type=simple\nExecStartPre=/usr/bin/false\n"))
        command(["systemctl", "daemon-reload"])
        run("activate", slots[2])
        (root / "runtime/health-token").unlink()
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline and (not request.exists() or prop("apply", "Result") == "success"):
            time.sleep(0.2)
        require(request.exists() and prop("apply", "Result") != "success", "failed apply discarded its request")
        phase = "retry"
        stub_path.write_text(stub)
        command(["systemctl", "daemon-reload"])
        command(["systemctl", "reset-failed", units["current"], units["apply"]])
        deadline = time.monotonic() + 12
        while time.monotonic() < deadline and (request.exists() or prop("current", "MainPID") == "0"):
            time.sleep(0.2)
        third_pid = prop("current", "MainPID")
        require(third_pid not in ("0", second_pid) and not request.exists(), "pending apply did not recover")
        facts["afterRecoveryCleanTicks"] = stable_ticks(third_pid)
        status = run("status")
        require(status.get("confirmedSlotId") == slots[1] and status.get("pending") is None, "test recovery state mismatch")
        facts.update({"confirmationApplied": True, "failurePreservedRequest": True, "retryAcknowledged": True,
                      "productionServicesTouched": False})
        receipt = {"probe": "isolated-apply-events", "result": "passed", "checks": facts}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        diagnostics: dict[str, object] = {}
        for key in ("watchdog", "apply", "current", "timer", "deadline"):
            properties = subprocess.run(["systemctl", "show", "-p", "LoadState,ActiveState,SubState,Result,ExecMainStatus,LoadError",
                                         units[key]], capture_output=True, timeout=10, check=False)
            journal = subprocess.run(["journalctl", "--no-pager", "-n", "8", "-o", "cat", "-u", units[key]],
                                     capture_output=True, timeout=10, check=False)
            diagnostics[key] = {"properties": properties.stdout.decode(errors="replace")[:2048],
                                "journal": journal.stdout.decode(errors="replace")[-2048:]}
        receipt = {"probe": "isolated-apply-events", "result": "failed", "phase": phase,
                   "error": str(error)[:2048], "checks": facts, "unitDiagnostics": diagnostics}
    finally:
        cleanup(root)
    receipt["fixtureRemoved"] = True
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as stream:
        json.dump(receipt, stream, sort_keys=True)
    print(json.dumps(receipt, sort_keys=True))
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
