#!/usr/bin/env python3
"""Native read-only status socket gate against the retained application state."""

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
PREFIX = "blikvm-update-status-"
SUPERVISOR_SHA = "2a1fe1ca3b255c6b93b8b89b8cb2f85476fbed7453344cb0ec60fbe64033c181"
UNIT_ROOT = Path("/run/systemd/system")


def require(value: bool, message: str) -> None:
    if not value:
        raise ValueError(message)


def command(argv: list[str]) -> bytes:
    result = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, timeout=20, check=False)
    require(result.returncode == 0, "command failed: " + argv[0])
    return result.stdout


def client(path: str, case: str) -> dict[str, object]:
    request = {"schemaVersion": 1, "requestId": 7, "request": {"type": "status"}}
    if case == "mutation":
        request["request"] = {"type": "activate", "slot": "untrusted"}
    data = json.dumps(request, separators=(",", ":")).encode()
    frame = struct.pack(">I", 0xFFFFFFFF if case == "oversize" else len(data)) + data
    wire = bytearray()
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(5)
        stream.connect(path)
        try:
            stream.sendall(frame)
            stream.shutdown(socket.SHUT_WR)
            while len(wire) <= 16388:
                chunk = stream.recv(16389 - len(wire))
                if not chunk:
                    break
                wire.extend(chunk)
        except (BrokenPipeError, ConnectionResetError):
            pass
    if not wire:
        return {"result": "rejected", "bytesReceived": 0}
    require(len(wire) >= 4 and struct.unpack(">I", wire[:4])[0] == len(wire) - 4 and len(wire) <= 16388, "invalid status response frame")
    return {"result": "response", "response": json.loads(wire[4:])}


def cleanup(root: Path) -> None:
    require(root.parent == Path("/var/lib") and re.fullmatch(PREFIX + "[a-f0-9]{24}", root.name) is not None,
            "invalid status fixture namespace")
    require(not root.is_symlink() and root.stat().st_uid == 0, "unsafe status fixture root")
    base = root.name
    for name in (base + ".socket", base + "-deadline.timer"):
        subprocess.run(["systemctl", "stop", name], capture_output=True, timeout=20, check=False)
    instances = command(["systemctl", "list-units", "--all", "--plain", "--no-legend", base + "@*.service"])
    for line in instances.decode().splitlines():
        name = line.split()[0]
        require(name.startswith(base + "@") and name.endswith(".service"), "unexpected status fixture instance")
        subprocess.run(["systemctl", "stop", name], capture_output=True, timeout=20, check=False)
    for suffix in (".socket", "@.service", "-deadline.timer", "-cleanup.service"):
        (UNIT_ROOT / (base + suffix)).unlink(missing_ok=True)
    command(["systemctl", "daemon-reload"])
    runtime = Path("/run") / base
    if runtime.exists():
        require(not runtime.is_symlink() and runtime.stat().st_uid == 0 and not list(runtime.iterdir()), "status runtime is not empty")
        runtime.rmdir()
    shutil.rmtree(root)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--cleanup-root", type=Path)
    parser.add_argument("--client")
    parser.add_argument("--case", choices=("status", "mutation", "oversize"), default="status")
    args = parser.parse_args()
    if args.client:
        print(json.dumps(client(args.client, args.case), sort_keys=True))
        return
    require(os.geteuid() == 0, "root fixture orchestration required")
    if args.cleanup_root:
        cleanup(args.cleanup_root)
        return
    assets = cast(Path, args.assets)
    binary = (assets / "supervisor").read_bytes()
    require(hashlib.sha256(binary).hexdigest() == SUPERVISOR_SHA, "status supervisor hash mismatch")
    group = grp.getgrnam("hardened-blikvm").gr_gid
    uid = pwd.getpwnam("hardened-blikvm-server").pw_uid
    root = Path("/var/lib") / (PREFIX + secrets.token_hex(12))
    root.mkdir(mode=0o750)
    root.chmod(0o750)
    os.chown(root, 0, group)
    base = root.name
    endpoint = Path("/run") / base / "status.sock"
    facts: dict[str, object] = {}
    receipt: dict[str, object]
    try:
        (root / "supervisor").write_bytes(binary)
        (root / "supervisor").chmod(0o755)
        shutil.copyfile(Path(__file__), root / "controller.py")
        (root / "controller.py").chmod(0o644)
        worker = (assets / "status.service").read_text().replace("/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor", str(root / "supervisor"))
        require(not any(value in worker for value in ("OnSuccess=", "Requires=", "ReadWritePaths=", "StateDirectory=", "RuntimeDirectory=")), "status worker acquired mutation authority")
        listener = (assets / "status.socket").read_text().replace("/run/hardened-blikvm-update-api/status.sock", str(endpoint))
        (UNIT_ROOT / (base + "@.service")).write_text(worker)
        (UNIT_ROOT / (base + ".socket")).write_text(listener)
        (UNIT_ROOT / (base + "-cleanup.service")).write_text(f"[Service]\nType=oneshot\nExecStart=/usr/bin/python3 -B {root}/controller.py --cleanup-root {root}\n")
        (UNIT_ROOT / (base + "-deadline.timer")).write_text(f"[Timer]\nOnActiveSec=100\nUnit={base}-cleanup.service\n")
        command(["systemctl", "daemon-reload"])
        command(["systemctl", "start", base + "-deadline.timer", base + ".socket"])

        def query(peer: int, case: str) -> dict[str, object]:
            result = subprocess.run(["python3", "-B", str(root / "controller.py"), "--client", str(endpoint), "--case", case],
                                    user=peer, group=group, extra_groups=[], stdin=subprocess.DEVNULL,
                                    capture_output=True, timeout=10, check=False)
            require(result.returncode == 0, "status client failed")
            return cast(dict[str, object], json.loads(result.stdout))

        require(query(0, "status") == {"result": "rejected", "bytesReceived": 0}, "root peer unexpectedly received status")
        facts["wrongUidRejected"] = True
        for case in ("mutation", "oversize"):
            require(query(uid, case) == {"result": "rejected", "bytesReceived": 0}, "invalid request received status")
        facts["mutationAndOversizeRejected"] = True
        statuses: list[dict[str, object]] = []
        for _ in range(4):
            answer = query(uid, "status")
            response = cast(dict[str, object], answer.get("response"))
            require(answer.get("result") == "response" and response.get("schemaVersion") == 1 and response.get("requestId") == 7, "status envelope mismatch")
            result = cast(dict[str, object], response["response"])
            require(result.get("type") == "status", "status worker rejected valid read")
            value = cast(dict[str, object], result["status"])
            require(value.get("floorSequence") == 2 and value.get("pending") is None
                    and cast(dict[str, object], value["selected"]).get("sequence") == 2
                    and value["selected"] == value["confirmed"], "status does not match confirmed application")
            statuses.append(value)
        require(all(value == statuses[0] for value in statuses), "status changed during read-only requests")
        facts.update({"readOnlyRequests": 4, "status": statuses[0]})
        receipt = {"result": "passed", "checks": facts}
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        log = subprocess.run(["journalctl", "--no-pager", "-n", "12", "-o", "cat", "-u", base + "@*.service"], capture_output=True, timeout=10, check=False)
        receipt = {"result": "failed", "errorType": type(error).__name__, "checks": facts, "journal": log.stdout.decode(errors="replace")[-3000:]}
    finally:
        cleanup(root)
    receipt["fixtureRemoved"] = True
    print(json.dumps(receipt, sort_keys=True))
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
