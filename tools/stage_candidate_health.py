#!/usr/bin/env python3
"""Stage only bound candidate-test binaries; preserve the actual serving runtime."""

from __future__ import annotations

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

from tools import stage_device_prototype as transport
from tools import verify_candidate_health as native
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require
INSTALLED_SUPERVISOR_SHA = "2e0fe93a2acf14a34d60f2495b64fc245ef77f9eac0ebd0716ca21ae0b02cdc1"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("build-dir", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new receipt required")
    build = Path(args.build_dir)
    uploads: dict[str, bytes] = {}
    for name, filename, expected in (("server", "bli-kvm-server", native.SERVER_HASH),
                                     ("reporter", "bli-kvm-update-health-reporter", native.REPORTER_HASH)):
        data = (build / filename).read_bytes()
        require(len(data) < 8 * 1024 * 1024 and hashlib.sha256(data).hexdigest() == expected, "binary hash mismatch")
        uploads[name] = data
    for name in ("verify_candidate_health.py", "switch_signed_slot_server.py", "initialize_application_state.py", "application_bootstrap_roots.py"):
        uploads[name] = Path(__file__).with_name(name).read_bytes()
    uploads["candidate.service"] = (Path(__file__).resolve().parent.parent / "packaging/systemd/hardened-blikvm-server-candidate.service").read_bytes()
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-candidate-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    phase = "stage"
    receipt: dict[str, object] = {"probe": "isolated-candidate-health", "reporterSha256": native.REPORTER_HASH}
    observer_code = ("import json,sys;sys.path.insert(0," + repr(stage) + ");import switch_signed_slot_server as s;"
                     "v=s.snapshot();r=s.verify(v,True,expected_supervisor_sha=" + repr(INSTALLED_SUPERVISOR_SHA)
                     + ");print(json.dumps({'snapshot':v,'verification':r},sort_keys=True))")
    observer = shlex.join(["python3", "-B", "-c", observer_code])
    try:
        require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                "controller staging failed")
        staged = True
        for name, data in uploads.items():
            destination = q(stage + "/" + name)
            script = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream).returncode == 0, "upload failed")
        phase = "before"
        before = transport.run_ssh(target, secret, observer, timeout=60)
        require(before.returncode == 0, "serving preflight failed")
        receipt["before"] = json.loads(before.stdout)
        phase = "native-test"
        tested = transport.run_ssh(target, secret, shlex.join(["python3", "-B", stage + "/verify_candidate_health.py", "--assets", stage]), timeout=150)
        if tested.stdout:
            receipt["test"] = json.loads(tested.stdout)
        require(tested.returncode == 0, "candidate test failed")
        phase = "independent-after"
        after = transport.run_ssh(target, secret, observer, timeout=60)
        require(after.returncode == 0 and json.loads(after.stdout) == receipt["before"], "serving runtime changed")
        receipt.update({"result": "passed", "servingRuntimeUnchanged": True, "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "phase": phase, "errorType": type(error).__name__})
        return 1
    finally:
        if receipt.get("result") != "passed" and "before" in receipt:
            try:
                after = transport.run_ssh(target, secret, observer, timeout=60)
                receipt["failureRuntimeUnchanged"] = after.returncode == 0 and json.loads(after.stdout) == receipt["before"]
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["failureRuntimeUnchanged"] = False
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            exact = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {exact}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, script).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    raise SystemExit(main())
