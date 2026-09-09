#!/usr/bin/env python3
"""Run the reviewed HTTP gate and browser phases under one secret-stdin job."""

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
from typing import cast
from urllib.parse import urlsplit

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import installed_application_write_gate as native
from tools import stage_installed_upload_gate as staging
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin

transport = staging.transport
require = native.contract.require
PLAYWRIGHT = "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs"
CHROMIUM = "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome"


def build_uploads(old: str, new: str, bundle: str) -> dict[str, bytes]:
    uploads = staging.build_uploads(old, new, bundle, native.upload.probe.PROFILES["seq8-publish"])
    for name in ("installed_application_write_gate.py", *native.HELPERS):
        uploads[name] = Path(__file__).with_name(name).read_bytes()
    return uploads


def validate_device(value: object, root: str, result: str) -> dict[str, object]:
    require(isinstance(value, dict), "invalid controller receipt")
    receipt = cast(dict[str, object], value)
    require(receipt.get("guardRoot") == root and receipt.get("result") == result, "controller receipt identity differs")
    if result == "prepared":
        require(receipt.get("phase") == "wired" and receipt.get("deadlineArmed") is True, "HTTP gate not armed")
    else:
        require(receipt.get("runtimeWritesClosed") is True and type(receipt.get("floor")) is int
                and receipt["floor"] in (7, 8), "HTTP restoration unproven")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "bundle", "endpoint", "base-url", "username", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    require(not output.exists() and re.fullmatch(r"[a-z][a-z0-9-]{0,80}", output.stem) is not None, "new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    url = urlsplit(cast(str, args.base_url))
    require(url.scheme == "https" and url.hostname == host and not url.username and not url.password
            and not url.query and not url.fragment and url.path in ("", "/"), "invalid browser endpoint")
    require(re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", args.username) is not None, "invalid browser account")
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    uploads = build_uploads(args.old_package, args.new_package, args.bundle)
    run_id = secrets.token_hex(12)
    root = "/var/lib/" + native.contract.PREFIX + run_id
    stage = "/tmp/blikvm-http-controller-" + run_id
    receipt: dict[str, object] = {"probe": "installed-application-http-gate", "result": "failed", "guardRoot": root,
        "sourceHashes": {name: hashlib.sha256(data).hexdigest() for name, data in uploads.items()},
        "prepareSubmitted": False, "runtimeWritesClosed": False, "guardRetained": True,
        "targetPowerControl": False, "browser": {}}
    # Reserve the exact device namespace durably before any remote command.
    write_json_0600_atomic(output, receipt)
    staged = False
    q = shlex.quote
    with secret_pair_stdin() as pair:
        def ssh(script: str, timeout: int = 30) -> subprocess.CompletedProcess[bytes]:
            return transport.run_ssh(target, pair["ssh"], script, timeout=timeout)

        def controller(action: str, limit: int) -> dict[str, object]:
            argv = ["python3", "-B", stage + "/installed_application_write_gate.py" if action == "prepare" else root + "/controller.py",
                    action, "--guard-root", root]
            if action == "prepare":
                argv += ["--assets", stage]
            result = ssh(shlex.join(argv), limit)
            require(result.returncode == 0 and len(result.stdout) <= 65536, "device controller phase failed")
            value: object = json.loads(result.stdout, object_pairs_hook=native.upload.probe.unique_object)
            return validate_device(value, root, "prepared" if action == "prepare" else "cleaned" if action == "cleanup" else "restored")

        def browser(mode: str) -> dict[str, object]:
            destination = output.with_name(f"{output.stem}-{run_id}-{mode}.json")
            require(not destination.exists(), "browser receipt exists")
            argv = ["node", "tools/playwright/device_application_write.mjs", mode, PLAYWRIGHT, CHROMIUM,
                    args.base_url.rstrip("/"), args.username, args.bundle, str(destination)]
            result = subprocess.run(argv, input=pair["gui"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                    timeout=150 if mode == "activate" else 100, check=False, close_fds=True)
            path = transport.validate_private_file(str(destination))
            require(path.stat().st_size <= 65536, "oversize browser receipt")
            decoded: object = json.loads(path.read_bytes(), object_pairs_hook=native.upload.probe.unique_object)
            require(isinstance(decoded, dict), "invalid browser receipt")
            value = cast(dict[str, object], decoded)
            cast(dict[str, object], receipt["browser"])[mode] = value
            write_json_0600_atomic(output, receipt)
            require(result.returncode == 0 and value.get("result") == "passed" and value.get("mode") == mode,
                    "browser acceptance failed")
            return value

        try:
            require(ssh(f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                    "controller staging failed")
            staged = True
            for name, data in uploads.items():
                script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
                with tempfile.TemporaryFile() as stream:
                    stream.write(data)
                    stream.seek(0)
                    require(transport.run_ssh(target, pair["ssh"], script, stdin=stream, timeout=60).returncode == 0,
                            "controller asset upload failed")
            receipt["prepareSubmitted"] = True
            write_json_0600_atomic(output, receipt)
            receipt["prepared"] = controller("prepare", 180)
            write_json_0600_atomic(output, receipt)
            browser("activate")
            finalized = controller("finalize", 240)
            require(finalized.get("floor") == 8 and finalized.get("generation") == 40, "confirmed seq8 not established")
            receipt["finalized"] = finalized
            receipt["runtimeWritesClosed"] = True
            write_json_0600_atomic(output, receipt)
            browser("closed")
            receipt["cleanup"] = controller("cleanup", 90)
            receipt["guardRetained"] = False
            receipt["result"] = "passed"
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            receipt["errorType"] = type(error).__name__
            if receipt["prepareSubmitted"] and not receipt["runtimeWritesClosed"]:
                try:
                    receipt["recovery"] = controller("restore", 240)
                    receipt["runtimeWritesClosed"] = True
                except (OSError, ValueError, subprocess.SubprocessError) as recovery_error:
                    receipt["recoveryErrorType"] = type(recovery_error).__name__
            receipt["independentInspectionRequired"] = receipt["guardRetained"]
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
            write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
