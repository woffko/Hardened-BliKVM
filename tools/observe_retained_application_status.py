#!/usr/bin/env python3
"""Read the authoritative application status under a retained write policy."""

import argparse
import json
from pathlib import Path
import shlex
import sys

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

STATUS_SCRIPT = (
    "import importlib.util, json, sys\n"
    "root = sys.argv[1]\n"
    "sys.path.insert(0, root)\n"
    "spec = importlib.util.spec_from_file_location('installed_persistent_write', root + '/controller.py')\n"
    "native = importlib.util.module_from_spec(spec)\n"
    "sys.modules['installed_persistent_write'] = native\n"
    "spec.loader.exec_module(native)\n"
    "import installed_application_write_gate as temporary\n"
    "print(json.dumps({'status': temporary.upload.application.infrastructure.current_status(), "
    "'bootIdSha256': native.boot_identity()}, sort_keys=True))\n"
)


def validate_status(value: object) -> dict[str, object]:
    result = common.native.object_value(value)
    status = common.native.object_value(result.get("status"))
    floor = status.get("floorSequence")
    generation = status.get("generation")
    common.require(status.get("activeSlotId") == status.get("confirmedSlotId")
                   and type(floor) is int and type(generation) is int, "invalid retained status")
    assert isinstance(floor, int) and isinstance(generation, int)
    common.require(floor > 0 and generation > 0 and "pending" in status and "lastRollback" in status,
                   "invalid retained status")
    boot = result.get("bootIdSha256")
    common.require(isinstance(boot, str) and common.native.re.fullmatch(r"[a-f0-9]{64}", boot) is not None,
                   "invalid retained boot identity")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "guard-root", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    root = Path(args.guard_root)
    common.native.contract.guard_units(root)
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new retained status receipt required")
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root",
        common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    receipt: dict[str, object] = {"probe": "persistent-write-retained-status", "result": "failed",
        "guardRoot": str(root)}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        response = common.transport.run_ssh(target, secret,
            shlex.join(["python3", "-B", "-c", STATUS_SCRIPT, str(root)]), timeout=30)
        common.require(response.returncode == 0 and len(response.stdout) <= 16384,
                       "retained status observation failed")
        receipt.update(result="observed", observed=validate_status(json.loads(response.stdout)))
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "observed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
