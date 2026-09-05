#!/usr/bin/env python3
"""One software reboot, bounded SSH recovery and signed-slot persistence checks."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
import time
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_initialized_updater_upgrade as packages
from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = packages.require
REBOOT_COMMAND = "systemctl reboot --no-block"


def compare(before: dict[str, object], after: dict[str, object]) -> None:
    for value in (before.get("bootIdSha256"), after.get("bootIdSha256")):
        require(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None, "missing or invalid boot identifier")
    require(after.get("result") == "ready" and after.get("bootIdSha256") != before.get("bootIdSha256"), "boot identifier did not change")
    require(after.get("status") == before.get("status"), "persistent update journal changed on clean reboot")
    initial = cast(dict[str, object], before["snapshot"])
    final = cast(dict[str, object], after["snapshot"])
    for field in ("state", "nginxUnit"):
        require(final[field] == initial[field], "persistent application content changed")
    old_runtime = cast(dict[str, str], initial["runtime"])
    new_runtime = cast(dict[str, str], final["runtime"])
    for field in ("account", "nginxConfig", "certificate", "tlsKey", "trust"):
        require(new_runtime[field] == old_runtime[field], "account/TLS/trust configuration changed")
    for field in ("brokerRestarts", "videoRestarts"):
        require(new_runtime[field] == "0", "peer service restarted after boot")
    require(after.get("recoveryOrderVerified") is True and after.get("rootfsMode") == "rw", "post-boot recovery or rootfs policy failed")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--reboot", action="store_true", help="Request exactly one software reboot after preflight")
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new reboot receipt required")
    uploads = packages.upload_payloads(args.old_package, args.new_package)
    uploads["capture_operational_boot.py"] = Path(__file__).with_name("capture_operational_boot.py").read_bytes()
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/var/lib/hardened-blikvm-reboot-probe-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    phase = "stage"
    receipt: dict[str, object] = {"probe": "operational-signed-slot-reboot", "targetPowerControl": False, "softwareRebootRequested": False}

    def capture(post: bool) -> tuple[int, dict[str, object]]:
        argv = ["python3", "-B", stage + "/capture_operational_boot.py", "--assets", stage]
        if post:
            argv.append("--post")
        result = transport.run_ssh(target, secret, shlex.join(argv), timeout=40)
        return result.returncode, cast(dict[str, object], json.loads(result.stdout)) if result.stdout else {}

    try:
        require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0, "reboot observation staging failed")
        staged = True
        for name, data in uploads.items():
            destination = q(stage + "/" + name)
            script = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream).returncode == 0, "reboot probe upload failed")
        phase = "preflight"
        code, before = capture(False)
        receipt["before"] = before
        require(code == 0 and before.get("result") == "ready" and before.get("rootfsMode") == "rw", "reboot preflight failed")
        if not args.reboot:
            receipt["result"] = "passed"
            return 0
        phase = "reboot-request"
        # Set the receipt field before the single request: a lost SSH response
        # is never a reason to launch a second reboot command.
        receipt["softwareRebootRequested"] = True
        try:
            request = transport.run_ssh(target, secret, REBOOT_COMMAND, timeout=20)
            require(request.returncode in (0, 255), "software reboot request rejected")
        except subprocess.TimeoutExpired:
            receipt["rebootRequestReplyUnavailable"] = True
        phase = "await-new-boot"
        deadline = time.monotonic() + 240
        after: dict[str, object] = {}
        attempts = 0
        while time.monotonic() < deadline:
            attempts += 1
            try:
                code, after = capture(True)
            except (OSError, ValueError, subprocess.SubprocessError):
                time.sleep(2)
                continue
            if after.get("rootfsMode") == "ro":
                # The user explicitly requires this test device left rw.
                require(not receipt.get("rootfsRwRestored"), "rootfs remained read-only after remount")
                changed = transport.run_ssh(target, secret, "mount -o remount,rw /", timeout=15)
                require(changed.returncode == 0, "could not restore requested rw mode")
                receipt["rootfsRwRestored"] = True
                continue
            if code == 0 and after.get("bootIdSha256") != before.get("bootIdSha256"):
                break
            time.sleep(2)
        receipt.update({"after": after, "recoveryProbes": attempts})
        compare(before, after)
        phase = "post-boot-stability"
        server_pid = cast(dict[str, object], cast(dict[str, object], after["snapshot"])["serving"])["serverPid"]
        time.sleep(6)
        code, final = capture(True)
        require(code == 0, "post-boot stability probe failed")
        compare(before, final)
        require(cast(dict[str, object], cast(dict[str, object], final["snapshot"])["serving"])["serverPid"] == server_pid, "server restarted after recovery")
        receipt.update({"result": "passed", "final": final, "persistentStatePreserved": True, "recoveryOrderVerified": True})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "phase": phase, "errorType": type(error).__name__})
        return 1
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            exact = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {exact}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, script).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["probeStagingRemoved"] = not staged
        if staged:
            receipt["probeStagingRoot"] = stage
        write_json_0600_atomic(output, receipt)
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    raise SystemExit(main())
