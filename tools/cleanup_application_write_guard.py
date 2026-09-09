#!/usr/bin/env python3
"""Verify and remove one already-restored HTTP acceptance guard."""

import argparse
import json
from pathlib import Path
import shlex
import subprocess
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import run_application_write_gate as runner
from tools.capture_pikvm_baseline import write_json_0600_atomic


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "guard-root", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    root = Path(args.guard_root)
    runner.native.contract.guard_units(root)
    output = runner.transport.validate_output_path(args.output)
    runner.require(not output.exists(), "new cleanup receipt required")
    host, port = runner.transport.parse_endpoint(args.endpoint)
    target = runner.transport.RemoteTarget(host, port, "root", runner.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = runner.transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "http-update-guard-cleanup", "result": "failed", "guardRoot": str(root)}
    try:
        result = runner.transport.run_ssh(target, secret,
            shlex.join(["python3", "-B", str(root / "controller.py"), "cleanup", "--guard-root", str(root)]), timeout=120)
        runner.require(result.returncode == 0 and len(result.stdout) <= 65536, "guard cleanup verification failed")
        value: object = json.loads(result.stdout, object_pairs_hook=runner.native.upload.probe.unique_object)
        receipt["device"] = runner.validate_device(value, str(root), "cleaned")
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
