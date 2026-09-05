#!/usr/bin/env python3
"""Rehearse installed updater initialization in disposable device directories."""

from __future__ import annotations

import argparse
import grp
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import secrets
import stat
import subprocess
import tarfile
import tempfile
from typing import cast


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("supervisor", "bundle", "bundle-sha256", "config-sha256", "output"):
        parser.add_argument("--" + name, required=True)
    args = cast(dict[str, str], vars(parser.parse_args()))
    require(os.geteuid() == 0, "root orchestration required")
    require(args["supervisor"] == "/usr/libexec/hardened-blikvm/bli-kvm-update-supervisor", "installed supervisor required")
    output = Path(args["output"])
    require(output.parent == Path("/tmp") and not output.exists(), "new temporary receipt required")
    config = Path("/etc/hardened-blikvm/update-supervisor.json")
    config_bytes = config.read_bytes()
    require(hashlib.sha256(config_bytes).hexdigest() == args["config_sha256"], "trust config changed")
    data = Path(args["bundle"]).read_bytes()
    require(hashlib.sha256(data).hexdigest() == args["bundle_sha256"], "bundle hash mismatch")
    identity = pwd.getpwnam("hardened-blikvm-update")
    shared_gid = grp.getgrnam("hardened-blikvm").gr_gid
    require(identity.pw_uid != 0, "nonroot updater required")
    checks: dict[str, object] = {}
    with tempfile.TemporaryDirectory(prefix="blikvm-config-credential-bundle-", dir="/tmp") as temporary:
        root = Path(temporary)
        root.chmod(0o750)
        os.chown(root, 0, shared_gid)
        for name in ("slots", "state", "runtime"):
            (root / name).mkdir(mode=0o700)
            os.chown(root / name, identity.pw_uid, shared_gid)
        source = root / "application.tar"
        source.write_bytes(data)
        source.chmod(0o640)
        os.chown(source, 0, shared_gid)
        document = cast(dict[str, object], json.loads(config_bytes))
        for field, name in (("slotRoot", "slots"), ("stateRoot", "state"), ("runtimeRoot", "runtime")):
            document[field] = str(root / name)
        local_config = root / "public-config.json"
        local_config.write_text(json.dumps(document))
        local_config.chmod(0o600)

        def run(*arguments: str, rejected: bool = False) -> dict[str, object]:
            unit = "blikvm-bundle-check-" + secrets.token_hex(10) + ".service"
            command = [
                "systemd-run", "--quiet", "--wait", "--pipe", "--collect", "--unit=" + unit,
                "--property=User=hardened-blikvm-update", "--property=Group=hardened-blikvm",
                "--property=NoNewPrivileges=yes", "--property=ProtectSystem=strict",
                "--property=ProtectHome=yes", "--property=PrivateDevices=yes",
                "--property=CapabilityBoundingSet=", "--property=RestrictNamespaces=yes",
                "--property=RestrictAddressFamilies=AF_UNIX", "--property=SystemCallFilter=@system-service",
                "--property=ReadWritePaths=" + " ".join(str(root / name) for name in ("slots", "state", "runtime")),
                "--property=LoadCredential=update-supervisor-config:" + str(local_config),
                "--property=RuntimeMaxSec=20", args["supervisor"], "--config",
                "/run/credentials/" + unit + "/update-supervisor-config", *arguments,
            ]
            result = subprocess.run(command, capture_output=True, timeout=30, check=False)
            require(not Path("/run/credentials", unit).exists(), "credential mount leaked")
            if rejected:
                require(result.returncode == 40, "tampered bundle was not rejected by stager")
                return {"exitCode": result.returncode}
            require(result.returncode == 0, "supervisor " + arguments[0] + " exit " + str(result.returncode)
                    + ": " + result.stderr.decode(errors="replace")[:1024])
            return cast(dict[str, object], json.loads(result.stdout))

        require(run("validate-config").get("result") == "valid", "configuration invalid")
        staged = run("stage", str(source))
        require(staged.get("result") == "published" and staged.get("sequence") == 1, "initial slot mismatch")
        slot_id = cast(str, staged["slotId"])
        require(run("stage", str(source)).get("result") == "already_present", "repeat stage not idempotent")
        run("initialize", slot_id)
        status = run("status")
        for key, expected in {"generation": 1, "floorSequence": 1, "activeSlotId": slot_id,
                              "confirmedSlotId": slot_id, "pending": None, "previousSlotId": None}.items():
            require(status.get(key) == expected, "unexpected initial status: " + key)
        require(os.readlink(root / "slots/current") == slot_id, "initial pointer mismatch")
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            stream = archive.extractfile("manifest.json")
            assert stream is not None
            manifest = cast(dict[str, object], json.loads(stream.read()))
            offset = archive.getmember("payload/share/web/index.html").offset_data
        files = cast(list[dict[str, object]], manifest["files"])
        for item in files:
            name = cast(str, item["path"])
            if name.startswith("share/web/"):
                installed = Path("/usr/share/hardened-blikvm/web") / name.removeprefix("share/web/")
            else:
                require(name in {"bin/bli-kvm-server", "libexec/bli-kvm-platform-broker",
                                 "libexec/bli-kvm-video-proxy", "libexec/bli-kvm-control-broker"}, "unexpected binary")
                installed = Path("/usr/libexec/hardened-blikvm") / Path(name).name
            staged_file = root / "slots" / slot_id / name
            metadata = installed.lstat()
            require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == 0 and metadata.st_nlink == 1
                    and stat.S_IMODE(metadata.st_mode) == item["mode"], "installed payload metadata mismatch")
            require(hashlib.sha256(installed.read_bytes()).hexdigest() == item["sha256"], "installed payload mismatch")
            require(hashlib.sha256(staged_file.read_bytes()).hexdigest() == item["sha256"], "staged payload mismatch")
            require(stat.S_IMODE(staged_file.stat().st_mode) == (cast(int, item["mode"]) & ~0o222), "writable slot payload")
        expected_web = {cast(str, item["path"]).removeprefix("share/web/") for item in files
                        if cast(str, item["path"]).startswith("share/web/")}
        web_root = Path("/usr/share/hardened-blikvm/web")
        observed_web: set[str] = set()
        for path in web_root.rglob("*"):
            require(not path.is_symlink(), "installed web tree contains a symlink")
            if path.is_file():
                observed_web.add(str(path.relative_to(web_root)))
        require(observed_web == expected_web, "installed web tree differs from signed bundle")
        altered = bytearray(data)
        altered[offset] ^= 1
        source.write_bytes(altered)
        run("stage", str(source), rejected=True)
        require(run("status") == status, "negative check changed state")
        require(not list((root / "runtime").iterdir()), "unexpected health challenge")
        checks.update({"slotId": slot_id, "payloadFiles": len(files), "installedPayloadEquivalent": True,
                       "initialized": True, "idempotent": True, "tamperRejected": True})
    require(config.read_bytes() == config_bytes, "enrolled config changed")
    for path in ("/var/lib/hardened-blikvm-slots", "/var/lib/hardened-blikvm-update", "/run/hardened-blikvm-update"):
        require(not Path(path).exists() and not Path(path).is_symlink(), "production state unexpectedly created")
    receipt = {"probe": "installed-update-bundle-rehearsal", "result": "passed", "checks": checks,
               "supervisorSha256": hashlib.sha256(Path(args["supervisor"]).read_bytes()).hexdigest(),
               "bundleSha256": args["bundle_sha256"], "serviceUid": identity.pw_uid, "serviceGid": shared_gid,
               "temporaryRootsRemoved": True, "productionStateInitialized": False, "payloadExecuted": False}
    descriptor = os.open(output, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        json.dump(receipt, handle, sort_keys=True)
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(json.dumps({"result": "failed", "errorType": type(error).__name__, "error": str(error)[:2048]}))
        raise SystemExit(1) from None
