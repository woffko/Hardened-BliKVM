#!/usr/bin/env python3
"""Stage one bounded native upload rehearsal with a private result receipt."""

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
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import installed_upload_gate as native
from tools import stage_initialized_updater_upgrade as packages
from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require


def build_uploads(old: str, new: str, bundle: str) -> dict[str, bytes]:
    packages.select_profile("dev7-dev9")
    uploads = packages.upload_payloads(old, new)
    archive = Path(bundle).read_bytes()
    require(len(archive) == native.probe.BUNDLE_BYTES and hashlib.sha256(archive).hexdigest() == native.probe.BUNDLE_SHA,
            "upload rehearsal archive binding mismatch")
    uploads["application.tar"] = archive
    for name in ("installed_upload_gate.py", *native.HELPERS):
        uploads[name] = Path(__file__).with_name(name).read_bytes()
    return uploads


def validate_result(decoded: object) -> dict[str, object]:
    require(isinstance(decoded, dict), "invalid native gate object")
    value = cast(dict[str, object], decoded)
    packages.validate_guard(value.get("guardRoot"), prefix=native.PREFIX)
    require(value.get("result") == "passed" and value.get("guardMayRemain") is False
            and value.get("stateAndServingPreserved") is True, "native upload rehearsal did not pass")
    require(isinstance(value.get("vectors"), list), "native vectors missing")
    vectors = cast(list[object], value["vectors"])
    require(len(vectors) == len(native.MODES), "native vectors incomplete")
    for decoded_item, mode in zip(vectors, native.MODES, strict=True):
        require(isinstance(decoded_item, dict), "invalid vector object")
        item = cast(dict[str, object], decoded_item)
        require(item.get("mode") == mode and item.get("result") == ("reply-unobserved" if mode == "lost-reply" else "passed"),
                "native vector order or result mismatch")
        require(isinstance(item.get("worker"), dict), "worker terminal evidence missing")
        worker = cast(dict[str, object], item["worker"])
        require(worker.get("naturalExit") is True, "worker termination was not natural")
        if mode not in ("malformed", "unauthorized"):
            require(isinstance(item.get("live"), dict), "live worker evidence missing")
            live = cast(dict[str, object], item["live"])
            require(all(live.get(key) is True for key in ("dedicatedIdentity", "emptyCapabilities", "anonymousSpool", "sandboxVerified")),
                    "native worker identity or spool not verified")
    return value


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "bundle", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new native upload receipt required")
    uploads = build_uploads(args.old_package, args.new_package, args.bundle)
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = "/tmp/blikvm-upload-controller-" + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    receipt: dict[str, object] = {"probe": "installed-upload-rehearsal", "result": "failed",
        "bundleSha256": native.probe.BUNDLE_SHA, "targetPowerControl": False, "httpWritesEnabled": False,
        "newSlotPublicationTested": False, "sourceHashes": {name: hashlib.sha256(data).hexdigest() for name, data in uploads.items()}}
    try:
        require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                "native upload staging failed")
        staged = True
        for name, data in uploads.items():
            script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
            with tempfile.TemporaryFile() as stream:
                stream.write(data)
                stream.seek(0)
                require(transport.run_ssh(target, secret, script, stdin=stream, timeout=60).returncode == 0, "native probe upload failed")
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", stage + "/installed_upload_gate.py", "run", "--assets", stage]), timeout=540)
        require(len(result.stdout) <= 65536, "oversize native gate receipt")
        decoded: object = json.loads(result.stdout)
        receipt["device"] = decoded
        value = validate_result(decoded)
        receipt["guardRoot"] = value["guardRoot"]
        require(result.returncode == 0, "native upload command failed")
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        receipt["independentInspectionRequired"] = True
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            paths = " ".join(q(stage + "/" + name) for name in uploads)
            script = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {paths}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, script).returncode != 0
            except (OSError, ValueError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        if staged:
            receipt["result"] = "failed"
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
