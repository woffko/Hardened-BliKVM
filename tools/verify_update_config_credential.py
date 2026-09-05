#!/usr/bin/env python3
"""Local root-only systemd regression for delivery of public update config.

Uses transient services, a synthetic public key and fresh /tmp files only.
No SSH, signing key, device, installed application or updater unit is used.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import pwd
import shutil
import stat
import subprocess
import tempfile
from typing import cast


def check(condition: bool, message: str) -> None:
    if not condition:
        raise RuntimeError(message)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--supervisor", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--service-user", choices=("nobody", "hardened-blikvm-update"), default="nobody")
    args = parser.parse_args()
    binary = cast(Path, args.supervisor).resolve(strict=True)
    output = cast(Path, args.output)
    check(os.geteuid() == 0, "this local systemd regression requires root")
    check(output.is_absolute() and output.parent == Path("/tmp") and not output.exists(), "new /tmp receipt required")
    service_user = cast(str, args.service_user)
    uid = pwd.getpwnam(service_user).pw_uid
    check(uid != 0, "service user must be unprivileged")
    receipts: dict[str, object] = {}
    with tempfile.TemporaryDirectory(prefix="blikvm-config-credential-", dir="/tmp") as temporary:
        root = Path(temporary)
        root.chmod(0o755)
        executable = root / "supervisor"
        shutil.copyfile(binary, executable)
        executable.chmod(0o755)
        protected = root / "root-only"
        protected.mkdir(mode=0o700)
        config = protected / "config.json"
        document = {
            "schemaVersion": 1, "product": "Hardened BliKVM", "target": "bli-h616-pikvm",
            "keyId": "app-credential-test",
            "publicKeyHex": "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
            "slotRoot": str(root / "absent-slots"), "stateRoot": str(root / "absent-state"),
            "runtimeRoot": str(root / "absent-runtime"), "systemSequence": 1, "healthTimeoutMs": 60000,
        }
        raw = json.dumps(document).encode()
        with config.open("xb") as handle:
            handle.write(raw)
        config.chmod(0o600)
        for scenario in ("direct", "credential"):
            unit = root.name + "-" + scenario + ".service"
            command = [
                "systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + unit,
                "--property=User=" + service_user, "--property=NoNewPrivileges=yes",
                "--property=ProtectSystem=strict", "--property=ProtectHome=yes",
                "--property=PrivateDevices=yes", "--property=CapabilityBoundingSet=",
                "--property=RestrictNamespaces=yes", "--property=RuntimeMaxSec=10",
                "--property=SystemCallFilter=@system-service", "--property=RestrictAddressFamilies=AF_UNIX",
            ]
            path = str(config)
            if scenario == "credential":
                command.append("--property=LoadCredential=update-supervisor-config:" + str(config))
                path = "/run/credentials/" + unit + "/update-supervisor-config"
            command.extend([str(executable), "--config", path, "validate-config"])
            result = subprocess.run(command, capture_output=True, timeout=20, check=False)
            receipts[scenario] = {"exitCode": result.returncode}
            if scenario == "direct":
                check(result.returncode == 11, "direct root-only config did not fail with permission boundary")
            else:
                check(result.returncode == 0, "credential flow failed: " + result.stderr.decode(errors="replace"))
                check(json.loads(result.stdout) == {"command": "validate_config", "result": "valid"}, "invalid config validation receipt")
            check(not Path("/run/credentials", unit).exists(), "credential mount remained after service exit")
        check(config.read_bytes() == raw and stat.S_IMODE(config.stat().st_mode) == 0o600, "source config changed")
        for name in ("absent-slots", "absent-state", "absent-runtime"):
            check(not (root / name).exists(), "validation created runtime state")
    receipt = {
        "schemaVersion": 1, "result": "passed", "probe": "host-update-config-credential",
        "supervisorSha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "serviceUid": uid, "sourceRootOnlyPreserved": True, "stateRootsAbsent": True,
        "credentialMountsRemoved": True, "deviceContacted": False, "scenarios": receipts,
    }
    descriptor = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        json.dump(receipt, handle, sort_keys=True)
        handle.write("\n")
    if "SUDO_UID" in os.environ:
        os.chown(output, int(os.environ["SUDO_UID"]), int(os.environ["SUDO_GID"]))
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
