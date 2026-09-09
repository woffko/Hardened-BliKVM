#!/usr/bin/env python3
"""Stage/read or rehearse revocable update wiring; never upload/activate an app."""

import argparse
from collections.abc import Callable
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import installed_persistent_write as native
from tools import stage_initialized_updater_upgrade as packages
from tools.capture_pikvm_baseline import write_json_0600_atomic

transport = packages.transport
require = native.require


def build_uploads(old: str, new: str) -> dict[str, bytes]:
    packages.select_profile("dev7-dev9")
    payloads = packages.upload_payloads(old, new)
    for name in native.HELPERS:
        payloads[name] = Path(__file__).with_name(name).read_bytes()
    selected = {name: payloads[name] for name in native.ASSETS}
    selected["installed_persistent_write.py"] = Path(native.__file__).read_bytes()
    return selected


def validate_result(value: object, *, action: str, root: str) -> dict[str, object]:
    result = native.object_value(value)
    expected = {"prepare": "prepared", "restore": "restored", "cleanup": "cleaned"}[action]
    require(result.get("result") == expected and result.get("guardRoot") == root, "controller receipt differs")
    if action == "prepare":
        require(result.get("phase") == "wired" and result.get("deadlineArmed") is True
                and result.get("retentionApproved") is False, "revocable preparation unproven")
        bound = native.binding({"binding": result.get("binding")})
        require(bound.transaction_id == Path(root).name[len(native.contract.PREFIX):]
                and bound.baseline_slot == native.temporary.upload.boot.SEQ8_SLOT
                and bound.baseline_floor == 8 and bound.baseline_generation == 40, "preparation baseline differs")
    elif action == "restore":
        require(result.get("runtimeVerified") is True and result.get("boot") is False
                and result.get("retentionApproved") is False, "closed runtime restoration unproven")
    else:
        archive = "/var/lib/" + native.ARCHIVE_PREFIX + Path(root).name[len(native.contract.PREFIX):]
        require(result.get("runtimeWritesClosed") is True and result.get("archiveRoot") == archive
                and type(result.get("floor")) is int and cast(int, result["floor"]) >= 8
                and type(result.get("generation")) is int and cast(int, result["generation"]) >= 40, "cleanup or retained counters unproven")
    return result


def run_phases(action: str, receipt: dict[str, object], *, invoke: Callable[[str], dict[str, object]],
               observe: Callable[[], dict[str, object]], save: Callable[[], None]) -> None:
    """Each state-changing phase is submitted once; reserve its receipt first."""
    require(action in ("preflight", "rehearse"), "unreviewed rehearsal action")
    receipt["before"] = observe()
    receipt["runtimeWritesClosed"] = True
    save()
    if action == "preflight":
        receipt["result"] = "passed"
        return
    try:
        receipt["prepareSubmitted"] = True
        receipt["runtimeWritesClosed"] = False
        save()
        receipt["prepared"] = invoke("prepare")
        save()
        receipt["restored"] = invoke("restore")
        receipt["runtimeWritesClosed"] = True
        save()
        receipt["cleanupSubmitted"] = True
        save()
        receipt["cleaned"] = invoke("cleanup")
        receipt["guardRetained"] = False
        save()
        receipt["after"] = observe()
        require(native.object_value(receipt["before"])["bootIdSha256"]
                == native.object_value(receipt["after"])["bootIdSha256"], "unexpected reboot during same-boot rehearsal")
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        if receipt.get("prepareSubmitted") is True and receipt.get("runtimeWritesClosed") is not True:
            try:
                receipt["recovery"] = invoke("restore")
                receipt["runtimeWritesClosed"] = True
            except (OSError, ValueError, subprocess.SubprocessError) as recovery_error:
                receipt["recoveryErrorType"] = type(recovery_error).__name__
        # Do not repeat prepare or cleanup after an ambiguous transport result.
        # The retained root/archive and reserved ID are the recovery evidence.
        receipt["independentInspectionRequired"] = True
        save()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "rehearse"))
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new rehearsal receipt required")
    uploads = build_uploads(args.old_package, args.new_package)
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    run_id = secrets.token_hex(12)
    stage = "/tmp/blikvm-persistent-write-" + run_id
    root = "/var/lib/" + native.contract.PREFIX + run_id
    receipt: dict[str, object] = {"probe": "persistent-write-wiring-rehearsal", "action": args.action, "result": "failed",
        "guardRoot": root, "stageRoot": stage, "prepareSubmitted": False, "cleanupSubmitted": False,
        "runtimeWritesClosed": False, "guardRetained": False, "applicationUploads": 0, "applicationActivations": 0,
        "targetPowerControl": False, "rebootRequested": False, "retentionApproved": False,
        "sourceHashes": {name: hashlib.sha256(data).hexdigest() for name, data in uploads.items()}}
    def save() -> None:
        write_json_0600_atomic(output, receipt)
    save()  # Durable namespace receipt precedes staging or a remote invocation.
    secret = transport.read_secret_stdin()
    staged = False
    q = shlex.quote
    def ssh(command: str, timeout: int = 30) -> subprocess.CompletedProcess[bytes]:
        return transport.run_ssh(target, secret, command, timeout=timeout)
    def invoke(action: str) -> dict[str, object]:
        argv = ["python3", "-B", stage + "/installed_persistent_write.py" if action == "prepare" else root + "/controller.py",
            action, "--guard-root", root]
        if action == "prepare":
            receipt["guardRetained"] = True
            save()
            argv += ["--assets", stage]
        response = ssh(shlex.join(argv), {"prepare": 260, "restore": 300, "cleanup": 180}[action])
        require(response.returncode == 0 and len(response.stdout) <= 65536, "native controller phase failed")
        return validate_result(json.loads(response.stdout, object_pairs_hook=native.temporary.upload.probe.unique_object), action=action, root=root)
    def observe() -> dict[str, object]:
        response = ssh(shlex.join(["python3", "-B", stage + "/capture_operational_boot.py", "--assets", stage,
            "--profile", "dev7-dev9", "--application-profile", "seq8"]), 100)
        require(response.returncode == 0 and len(response.stdout) <= 1024 * 1024, "closed seq8 baseline unproven")
        value = native.object_value(json.loads(response.stdout, object_pairs_hook=native.temporary.upload.probe.unique_object))
        status = native.object_value(value.get("status"))
        require(value.get("result") == "ready" and value.get("applicationProfile") == "seq8"
                and status.get("floorSequence") == 8 and status.get("generation") == 40 and status.get("pending") is None,
                "seq8 baseline receipt differs")
        return {"status": status, "bootIdSha256": value.get("bootIdSha256"), "runtimeWritesClosed": True}
    try:
        require(ssh(f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                "rehearsal staging failed")
        staged = True
        for name, data in uploads.items():
            script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream, timeout=60).returncode == 0, "rehearsal staging upload failed")
        run_phases(args.action, receipt, invoke=invoke, observe=observe, save=save)
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        receipt["independentInspectionRequired"] = True
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            paths = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {paths}\nrmdir {q(stage)}"
            try:
                staged = ssh(script).returncode != 0
            except (OSError, ValueError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        if staged:
            receipt["result"] = "failed"
        secret[:] = b"\0" * len(secret)
        save()
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
