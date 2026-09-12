#!/usr/bin/env python3
"""Apply the explicitly approved upload-only proxy change with private backup."""

import argparse
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as common
from tools import upload_proxy_streaming as native
from tools.capture_pikvm_baseline import write_json_0600_atomic


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new proxy receipt required")
    uploads = common.build_uploads(args.old_package, args.new_package)
    uploads["upload_proxy_streaming.py"] = Path(native.__file__).read_bytes()
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root", common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    run_id = secrets.token_hex(12)
    root = "/var/lib/" + native.PREFIX + run_id
    stage = "/tmp/blikvm-upload-proxy-" + run_id
    receipt: dict[str, object] = {"probe": "upload-proxy-streaming", "result": "failed", "backupRoot": root,
        "stageRoot": stage, "submitted": False, "writesEnabledByThisTool": False,
        "sourceHashes": {name: hashlib.sha256(data).hexdigest() for name, data in uploads.items()}}
    def save() -> None:
        write_json_0600_atomic(output, receipt)
    save()
    secret = common.transport.read_secret_stdin()
    staged = False
    q = shlex.quote
    def ssh(command: str, timeout: int = 30) -> subprocess.CompletedProcess[bytes]:
        return common.transport.run_ssh(target, secret, command, timeout=timeout)
    try:
        common.require(ssh(f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
            "proxy controller staging failed")
        staged = True
        for name, data in uploads.items():
            command = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                common.require(common.transport.run_ssh(target, secret, command, stdin=stream, timeout=60).returncode == 0,
                    "proxy controller upload failed")
        receipt["submitted"] = True
        save()
        result = ssh(shlex.join(["python3", "-B", stage + "/upload_proxy_streaming.py", "prepare", "--root", root, "--assets", stage]), 300)
        common.require(result.returncode == 0 and len(result.stdout) <= 65536, "proxy apply failed; inspect retained backup")
        value = common.native.object_value(json.loads(result.stdout, object_pairs_hook=common.native.temporary.upload.probe.unique_object))
        common.require(value.get("result") == "passed" and value.get("backupRoot") == root
            and value.get("runtimePreserved") is True and value.get("runtimeWritesClosed") is True
            and value.get("recoveryHooksVerified") is True and value.get("freshWorkersVerified") is True, "proxy acceptance missing")
        receipt["device"] = value
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        receipt["independentInspectionRequired"] = True
        # Never submit the apply phase a second time after an uncertain reply.
        # systemd owns ExecStopPost recovery; its root-private backup persists.
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            paths = " ".join(q(stage + "/" + name) for name in uploads)
            command = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {paths}\nrmdir {q(stage)}"
            try:
                staged = ssh(command).returncode != 0
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
