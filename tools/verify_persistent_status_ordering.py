#!/usr/bin/env python3
"""Check the persistent status boot-guard graph offline, without booting a device."""

from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import persistent_status_contract as contract


def verify(*, inject_cycle: bool = False) -> subprocess.CompletedProcess[bytes]:
    with tempfile.TemporaryDirectory(prefix="blikvm-status-ordering-", dir="/tmp") as temporary:
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
            rendered[boot_name] = rendered[boot_name].replace("After=local-fs.target", "After=local-fs.target basic.target")
        for name, content in rendered.items():
            (unit_root / name).write_text(content)
        # Explicit minimal target graph. This validates ordering decisions,
        # not the behavior of the real early-boot controller or target OS.
        targets = {
            "local-fs.target": "Before=sysinit.target",
            "sysinit.target": f"Requires=local-fs.target\nWants={boot_name}\nAfter=local-fs.target",
            "basic.target": "Requires=sysinit.target\nAfter=sysinit.target",
            "shutdown.target": "",
            "sockets.target": "Requires=sysinit.target\nAfter=sysinit.target",
            "timers.target": "Requires=sysinit.target\nAfter=sysinit.target",
        }
        for name, directives in targets.items():
            (unit_root / name).write_text("[Unit]\nDescription=Offline status-ordering fixture\n" + directives + "\n")
        for server in (contract.CURRENT, contract.CANDIDATE):
            (unit_root / server).write_text(
                f"[Unit]\nRequires=basic.target\nAfter=basic.target {contract.SOCKET}\nWants={contract.SOCKET}\n"
                "[Service]\nExecStart=/usr/bin/python3\n")
        (unit_root / contract.SOCKET).write_text(
            "[Socket]\nListenStream=/run/status-fixture.sock\nAccept=yes\n")
        (unit_root / "hardened-blikvm-update-status@.service").write_text(
            "[Service]\nType=oneshot\nExecStart=/usr/bin/python3\nStandardInput=socket\nTimeoutStartSec=5\n")
        return subprocess.run(["systemd-analyze", "verify", "--man=no", "--generators=no", "--root=" + str(root),
                               contract.CURRENT, contract.CANDIDATE, boot_name],
                              capture_output=True, timeout=15, check=False)


def main() -> None:
    positive = verify()
    if positive.returncode != 0:
        raise RuntimeError(positive.stderr.decode())
    negative = verify(inject_cycle=True)
    if negative.returncode == 0 or b"cycle" not in negative.stderr.lower():
        raise RuntimeError("offline checker did not reject the deliberately cyclic boot guard")
    print("offline status ordering: valid graph accepted; injected basic/sysinit cycle rejected; no boot performed")


if __name__ == "__main__":
    main()
