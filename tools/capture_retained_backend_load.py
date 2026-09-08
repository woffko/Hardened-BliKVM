#!/usr/bin/env python3
"""Capture fixed backend IPC load and resource observations through SSH."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import observe_service_resources as resources
from tools import retained_load_ipc as ipc
from tools import observe_retained_backend_load as load
from tools import stage_device_prototype as transport
from tools.capture_retained_resource_baseline import validate as validate_resources
from tools.capture_pikvm_baseline import write_json_0600_atomic


def validate(raw: bytes) -> dict[str, object]:
    ipc.require(len(raw) <= 256 * 1024, "oversize backend observation")
    value = ipc.object_value(json.loads(raw, object_pairs_hook=ipc.unique_object, parse_constant=ipc.reject_constant))
    ipc.require(set(value) == {"result", "workload", "samples", "summary", "loadSamples", "loadSummary",
                               "workloadChildExited"} and value["result"] == "observed"
                and value["workload"] == load.WORKLOAD and value["workloadChildExited"] is True,
                "incomplete backend observation")
    # Reuse the existing strict resource-number/schema/summary verifier. Only
    # its internal validation envelope is adapted; the receipt retains load scope.
    validate_resources(json.dumps({"result": "observed", "workload": "no-observer-generated-workload",
                                   "samples": value["samples"], "summary": value["summary"]}).encode())
    ipc.require(isinstance(value["loadSamples"], list), "invalid workload samples")
    samples = [ipc.object_value(item) for item in cast(list[object], value["loadSamples"])]
    ipc.require(value["loadSummary"] == load.validate_load(samples), "workload summary mismatch")
    return value


def program() -> tuple[str, dict[str, str]]:
    modules = (resources, ipc, load)
    sources = {module.__name__: Path(cast(str, module.__file__)).read_text() for module in modules}
    bootstrap = (
        "import sys, types\n"
        "package = types.ModuleType('tools')\n"
        "package.__path__ = []\n"
        "sys.modules['tools'] = package\n"
        f"sources = {sources!r}\n"
        "for name, source in sources.items():\n"
        "    module = types.ModuleType(name)\n"
        "    module.__package__ = 'tools'\n"
        "    sys.modules[name] = module\n"
        "    setattr(package, name.split('.')[-1], module)\n"
        "    exec(compile(source, '<' + name + '>', 'exec'), module.__dict__)\n"
    )
    worker = bootstrap + "sys.modules['tools.observe_retained_backend_load'].worker()\n"
    remote = bootstrap + f"sys.modules['tools.observe_retained_backend_load'].main({worker!r})\n"
    ipc.require(len(remote.encode()) < 100000, "observer source too large")
    return remote, {name: hashlib.sha256(source.encode()).hexdigest() for name, source in sources.items()}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    ipc.require(not output.exists(), "new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    source, hashes = program()
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"schemaVersion": 1, "probe": "retained-backend-load-resources",
                                "sourceSha256": hashes, "result": "failed"}
    try:
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=900)
        receipt["sshExitCode"] = result.returncode
        if result.returncode == 0:
            receipt["observation"] = validate(result.stdout)
            receipt["result"] = "observed"
        else:
            receipt["errorType"] = "remote-observation-failed"
            # Permit only bounded diagnostic categories, never stderr/raw paths.
            if len(result.stdout) <= 1024:
                failure = ipc.object_value(json.loads(result.stdout))
                phase = failure.get("phase")
                if phase in ("preflight", "workload", "validation"):
                    receipt["failurePhase"] = phase
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "observed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
