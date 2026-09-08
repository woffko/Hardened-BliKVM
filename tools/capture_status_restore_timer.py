#!/usr/bin/env python3
"""Run the fixed status-timer arm/disarm probe through secret-stdin SSH."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import observe_service_resources as resources
from tools import retained_load_ipc as ipc
from tools import observe_retained_backend_load as load
from tools import status_outage_contract as policy
from tools import systemd_property as properties
from tools import status_restore_properties as metadata
from tools import probe_status_restore_timer as probe
from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic


def program(operation: str) -> tuple[str, dict[str, str]]:
    policy.guard_name(operation)
    modules = (resources, ipc, load, policy, properties, metadata, probe)
    sources = {module.__name__: Path(cast(str, module.__file__)).read_text() for module in modules}
    child = sources[ipc.__name__] + "\nprint(json.dumps(read_status(), sort_keys=True))\n"
    bootstrap = (
        "import sys,types,json\n"
        "package=types.ModuleType('tools');package.__path__=[];sys.modules['tools']=package\n"
        f"sources={sources!r}\n"
        "for name,source in sources.items():\n"
        " module=types.ModuleType(name);module.__package__='tools';sys.modules[name]=module\n"
        " setattr(package,name.split('.')[-1],module)\n"
        " exec(compile(source,'<'+name+'>','exec'),module.__dict__)\n"
        f"result=sys.modules[{probe.__name__!r}].probe({operation!r},{child!r})\n"
        "print(json.dumps(result,sort_keys=True))\n"
        "raise SystemExit(0 if result['result']=='passed' else 1)\n"
    )
    ipc.require(len(bootstrap.encode()) < 100000, "probe source too large")
    return bootstrap, {name: hashlib.sha256(source.encode()).hexdigest() for name, source in sources.items()}


def validate(raw: bytes, operation: str) -> dict[str, object]:
    ipc.require(len(raw) <= 4096, "oversize timer probe receipt")
    result = ipc.object_value(json.loads(raw))
    ipc.require(result.get("operation") == operation and type(result.get("guardMayRemain")) is bool,
                "timer probe receipt identity mismatch")
    if result.get("result") == "passed":
        checks = {"guardVerified", "guardRemoved", "statusUnchanged", "servingIdentityUnchanged", "statusSocketNeverStopped"}
        ipc.require(set(result) == checks | {"result", "operation", "guardMayRemain", "initialGuardMarginSeconds"}
                    and all(result[key] is True for key in checks) and result["guardMayRemain"] is False,
                    "incomplete timer probe")
        margin = result["initialGuardMarginSeconds"]
        ipc.require(type(margin) is int and policy.MIN_REMAINING_SECONDS <= margin <= policy.GUARD_SECONDS,
                    "invalid timer margin")
    else:
        ipc.require(set(result) == {"result", "operation", "guardMayRemain", "phase", "errorType"}
                    and result["result"] == "failed" and isinstance(result["phase"], str)
                    and isinstance(result["errorType"], str)
                    and result["phase"] in {"baseline", "namespace", "arm", "verify", "restore-noop", "disarm", "cleanup"}
                    and result["errorType"] in {"OSError", "ValueError", "KeyError", "CalledProcessError", "TimeoutExpired",
                                                 "FileNotFoundError", "PermissionError", "JSONDecodeError"},
                    "invalid timer probe failure")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    ipc.require(not output.exists(), "new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    operation = secrets.token_hex(12)
    source, hashes = program(operation)
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "status-restore-timer-arm-disarm", "result": "failed",
                                "operation": operation, "sourceSha256": hashes, "guardMayRemain": True}
    try:
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=210)
        receipt["sshExitCode"] = result.returncode
        observation = validate(result.stdout, operation)
        receipt["observation"] = observation
        receipt["guardMayRemain"] = observation["guardMayRemain"]
        if result.returncode == 0 and observation["result"] == "passed":
            receipt["result"] = "passed"
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
