#!/usr/bin/env python3
"""Offline graph verification for the proposed persistent-write transaction."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import persistent_write_contract as contract


def verify(*, inject_cycle: bool = False) -> subprocess.CompletedProcess[bytes]:
    with tempfile.TemporaryDirectory(prefix="blikvm-write-ordering-", dir="/tmp") as temporary:
        root = Path(temporary)
        unit_root = root / "usr/lib/systemd/system"
        unit_root.mkdir(parents=True)
        binary = root / "usr/bin/python3"
        binary.parent.mkdir(parents=True)
        shutil.copyfile("/bin/true", binary)
        binary.chmod(0o755)
        guard = Path("/var/lib") / (contract.PREFIX + "a" * 24)
        (root / guard.relative_to("/")).mkdir(parents=True)
        rendered = contract.guard_units(guard)
        boot_name = guard.name + "-boot.service"
        if inject_cycle:
            rendered[boot_name] = rendered[boot_name].replace(b"After=local-fs.target", b"After=local-fs.target basic.target")
        for name, content in rendered.items():
            (unit_root / name).write_bytes(content)
        targets = {
            "local-fs.target": "Before=sysinit.target",
            "sysinit.target": f"Requires=local-fs.target\nWants={boot_name}\nAfter=local-fs.target",
            "basic.target": "Requires=sysinit.target sockets.target\nAfter=sysinit.target sockets.target",
            "shutdown.target": "",
            "sockets.target": "Requires=sysinit.target\nAfter=sysinit.target\nWants=" + " ".join(contract.SOCKETS),
            "timers.target": "Requires=sysinit.target\nAfter=sysinit.target",
        }
        for name, directives in targets.items():
            (unit_root / name).write_text("[Unit]\nDescription=Offline write-ordering fixture\n" + directives + "\n")
        status_socket = "hardened-blikvm-update-status.socket"
        for server in contract.SERVERS:
            (unit_root / server).write_text(f"[Unit]\nRequires=basic.target\nAfter=basic.target {status_socket}\n"
                + f"Wants={status_socket}\n[Service]\nExecStart=/usr/bin/python3\n")
        for index, socket in enumerate((*contract.SOCKETS, status_socket)):
            (unit_root / socket).write_text(f"[Socket]\nListenStream=/run/write-fixture-{index}.sock\nAccept=yes\n")
            (unit_root / socket.replace(".socket", "@.service")).write_text(
                "[Service]\nType=oneshot\nExecStart=/usr/bin/python3\nStandardInput=socket\nTimeoutStartSec=5\n")
        for path, content in contract.transaction_dropins(guard).items():
            destination = root / path.relative_to("/")
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(content)
        for path in contract.dropins():
            (root / path.relative_to("/")).write_bytes(contract.DROPIN_BYTES)
        return subprocess.run(["systemd-analyze", "verify", "--man=no", "--generators=no", "--root=" + str(root),
            *contract.SERVERS, *contract.SOCKETS, boot_name], capture_output=True, timeout=15, check=False)


def main() -> None:
    positive = verify()
    if positive.returncode:
        raise RuntimeError(positive.stderr.decode())
    negative = verify(inject_cycle=True)
    if negative.returncode == 0 or b"cycle" not in negative.stderr.lower():
        raise RuntimeError("offline write checker did not reject the injected boot cycle")
    print("offline write ordering: transaction graph accepted; injected cycle rejected; no boot/device actions")


if __name__ == "__main__":
    main()
