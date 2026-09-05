#!/usr/bin/env python3
"""Isolated real-server candidate health credential delivery on native systemd."""

from __future__ import annotations

import argparse
import grp
import hashlib
import json
import os
from pathlib import Path
import pwd
import re
import secrets
import shutil
import socket
import struct
import subprocess
import sys
from typing import cast

sys.dont_write_bytecode = True
PREFIX = "blikvm-candidate-health-"
PORT = 18481
UNIT_ROOT = Path("/run/systemd/system")
SERVER_HASH = "5f2da39a6563dff23503ec4ebe7c461775b579443dbae3e10c58821d22618fbc"
REPORTER_HASH = "1c1db722b6250160349915202f6c081838ec656f663fa61fe07831ccdb06fc5c"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def command(argv: list[str], timeout: int = 25) -> subprocess.CompletedProcess[bytes]:
    return subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=timeout, check=False)


def checked(argv: list[str]) -> bytes:
    result = command(argv)
    require(result.returncode == 0, "command failed: " + argv[0])
    return result.stdout


def names(token: str) -> dict[str, str]:
    return {key: PREFIX + token + "-" + key + suffix for key, suffix in (
        ("candidate", ".service"), ("cleanup", ".service"), ("deadline", ".timer"))}


def render_candidate(template: str, root: Path, runtime: Path, *, alias: bool) -> str:
    text = "\n".join(line for line in template.splitlines()
                     if not line.startswith(("Conflicts=", "StateDirectory"))) + "\n"
    text = text.replace(" hardened-blikvm-update-confirm.socket", "")
    # This component gate tests reporter delivery only. The separate full-flow
    # gate retains the recovery dependency in its isolated unit graph.
    text = text.replace(" hardened-blikvm-update-recover.service", "")
    replacements = {
        "/var/lib/hardened-blikvm-slots/current/bin/bli-kvm-server": str(root / "server"),
        "/var/lib/hardened-blikvm-slots/current/share/web": "/usr/share/hardened-blikvm/web",
        "BLI_KVM_STATE_DIR=/var/lib/hardened-blikvm": "BLI_KVM_STATE_DIR=" + str(root / "server-state"),
        "/usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter": str(root / "reporter"),
        "/run/hardened-blikvm-update/health-token": str(root / "health-token"),
        "/run/hardened-blikvm-update-confirm/confirm.sock": str(runtime / "confirm.sock"),
        "127.0.0.1:18080": f"127.0.0.1:{PORT}",
        "SocketBindAllow=tcp:18080": f"SocketBindAllow=tcp:{PORT}",
    }
    for original, replacement in replacements.items():
        require(original in text, "candidate template binding changed")
        text = text.replace(original, replacement)
    if not alias:
        text = "\n".join(line for line in text.splitlines() if not line.startswith("BindReadOnlyPaths=")) + "\n"
    text += f"[Service]\nReadWritePaths={root}/server-state\nRuntimeMaxSec=60\n"
    require("Conflicts=" not in text and "/run/hardened-blikvm-update/" not in text
            and "BLI_KVM_STATE_DIR=/var/lib/hardened-blikvm\n" not in text,
            "production mutation path remained")
    return text


def cleanup(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid fixture namespace")
    require(not root.is_symlink() and root.stat().st_uid == 0, "unsafe fixture root")
    token = root.name.removeprefix(PREFIX)
    units = names(token)
    for key in ("deadline", "candidate"):
        command(["systemctl", "stop", units[key]])
    for unit in units.values():
        (UNIT_ROOT / unit).unlink(missing_ok=True)
    checked(["systemctl", "daemon-reload"])
    runtime = Path("/run") / root.name
    if runtime.exists():
        require(not runtime.is_symlink() and runtime.stat().st_uid == 0, "unsafe fixture runtime")
        require({path.name for path in runtime.iterdir()} <= {"confirm.sock"}, "unexpected fixture runtime")
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
    for name, expected in (("server", SERVER_HASH), ("reporter", REPORTER_HASH)):
        require(hashlib.sha256((assets / name).read_bytes()).hexdigest() == expected, "binary hash mismatch")
    # Refuse collision with an existing listener before creating any service.
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", PORT))
    token = secrets.token_hex(12)
    root = Path("/var/lib") / (PREFIX + token)
    runtime = Path("/run") / root.name
    group = grp.getgrnam("hardened-blikvm").gr_gid
    server_uid = pwd.getpwnam("hardened-blikvm-server").pw_uid
    units = names(token)
    root.mkdir(mode=0o750)
    os.chown(root, 0, group)
    root.chmod(0o750)
    phase = "prepare"
    facts: dict[str, object] = {}
    credential = bytearray(secrets.token_bytes(32))
    receipt: dict[str, object]
    try:
        runtime.mkdir(mode=0o750)
        runtime.chmod(0o750)
        os.chown(runtime, 0, group)
        state = root / "server-state"
        state.mkdir(mode=0o700)
        os.chown(state, server_uid, group)
        for name in ("server", "reporter"):
            shutil.copyfile(assets / name, root / name)
            (root / name).chmod(0o755)
        shutil.copyfile(Path(__file__), root / "cleanup.py")
        fd = os.open(root / "health-token", os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "wb") as stream:
            stream.write(credential)
        template = (assets / "candidate.service").read_text()
        (UNIT_ROOT / units["cleanup"]).write_text(f"[Service]\nType=oneshot\nExecStart=/usr/bin/python3 -B {root}/cleanup.py --cleanup-root {root}\n")
        (UNIT_ROOT / units["deadline"]).write_text(f"[Timer]\nOnActiveSec=80\nUnit={units['cleanup']}\n")
        checked(["systemctl", "daemon-reload"])
        checked(["systemctl", "start", units["deadline"]])
        with socket.socket(socket.AF_UNIX) as listener:
            path = runtime / "confirm.sock"
            listener.bind(str(path))
            os.chown(path, 0, group)
            path.chmod(0o620)
            listener.listen(2)
            for alias in (False, True):
                phase = "alias-enabled" if alias else "alias-missing-negative"
                (UNIT_ROOT / units["candidate"]).write_text(render_candidate(template, root, runtime, alias=alias))
                checked(["systemctl", "daemon-reload"])
                command(["systemctl", "reset-failed", units["candidate"]])
                started = command(["systemctl", "start", units["candidate"]])
                if not alias:
                    require(started.returncode != 0, "missing alias unexpectedly succeeded")
                    listener.settimeout(0.2)
                    try:
                        unexpected, _ = listener.accept()
                    except TimeoutError:
                        facts["missingAliasRejectedWithoutSubmission"] = True
                    else:
                        unexpected.close()
                        raise ValueError("negative test submitted a credential")
                    checked(["systemctl", "stop", units["candidate"]])
                    continue
                require(started.returncode == 0, "candidate health reporter failed")
                listener.settimeout(3)
                connection, _ = listener.accept()
                with connection:
                    connection.settimeout(3)
                    peer = struct.unpack("3i", connection.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                    received = bytearray()
                    try:
                        while len(received) <= 32:
                            chunk = connection.recv(33 - len(received))
                            if not chunk:
                                break
                            received.extend(chunk)
                        require(peer[1] == server_uid and peer[2] == group and received == credential,
                                "credential or peer mismatch")
                    finally:
                        received[:] = b"\0" * len(received)
                require(checked(["systemctl", "show", "-p", "ActiveState", "--value", units["candidate"]]).strip() == b"active",
                        "candidate is not active")
                facts.update({"exactCredentialSubmitted": True, "serverPeerIdentityVerified": True, "realServerHealthPassed": True})
        receipt = {"result": "passed", "checks": facts, "productionServicesTouched": False}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        journal = command(["journalctl", "--no-pager", "-n", "10", "-o", "cat", "-u", units["candidate"]])
        receipt = {"result": "failed", "phase": phase, "errorType": type(error).__name__, "checks": facts,
                   "journal": journal.stdout.decode(errors="replace")[-3000:]}
    finally:
        credential[:] = b"\0" * len(credential)
        cleanup(root)
    receipt["fixtureRemoved"] = True
    print(json.dumps(receipt, sort_keys=True))
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
