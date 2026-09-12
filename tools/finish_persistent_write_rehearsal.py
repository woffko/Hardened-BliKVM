#!/usr/bin/env python3
"""Finish owned cleanup only after a receipt proves runtime restoration."""

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_persistent_write_rehearsal as runner
from tools.capture_pikvm_baseline import write_json_0600_atomic


def restored_root(value: object) -> str:
    receipt = runner.native.object_value(value)
    runner.require(receipt.get("probe") == "persistent-write-wiring-rehearsal"
        and receipt.get("guardRetained") is True and receipt.get("runtimeWritesClosed") is True
        and receipt.get("cleanupSubmitted") is False and receipt.get("retentionApproved") is False,
        "unambiguous restored guard receipt required")
    root = runner.packages.validate_guard(receipt.get("guardRoot"), prefix=runner.native.contract.PREFIX)
    restored = receipt.get("recovery") or receipt.get("restored")
    runner.validate_result(restored, action="restore", root=root)
    return root


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "receipt", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    source = runner.transport.validate_private_file(args.receipt)
    runner.require(source.stat().st_size <= 1024 * 1024, "oversize source receipt")
    raw = source.read_bytes()
    root = restored_root(json.loads(raw, object_pairs_hook=runner.native.temporary.upload.probe.unique_object))
    output = runner.transport.validate_output_path(args.output)
    runner.require(not output.exists(), "new cleanup receipt required")
    host, port = runner.transport.parse_endpoint(args.endpoint)
    target = runner.transport.RemoteTarget(host, port, "root", runner.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    receipt: dict[str, object] = {"probe": "finish-persistent-write-rehearsal", "result": "failed", "guardRoot": root,
        "sourceReceiptSha256": hashlib.sha256(raw).hexdigest(), "cleanupSubmitted": False, "runtimeWritesClosed": True}
    write_json_0600_atomic(output, receipt)
    secret = runner.transport.read_secret_stdin()
    try:
        receipt["cleanupSubmitted"] = True
        write_json_0600_atomic(output, receipt)
        result = runner.transport.run_ssh(target, secret, shlex.join(["python3", "-B", root + "/controller.py",
            "cleanup", "--guard-root", root]), timeout=180)
        runner.require(result.returncode == 0 and len(result.stdout) <= 65536, "owned cleanup failed or is ambiguous")
        receipt["cleanup"] = runner.validate_result(json.loads(result.stdout,
            object_pairs_hook=runner.native.temporary.upload.probe.unique_object), action="cleanup", root=root)
        receipt["result"] = "passed"
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
        receipt["independentInspectionRequired"] = True
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
