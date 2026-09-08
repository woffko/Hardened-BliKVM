#!/usr/bin/env python3
"""Verify the bounded Subscribe/monitor processes without creating a timer."""

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

from tools import status_outage_contract as policy
from tools import systemd_property as properties
from tools import status_restore_event as event
from tools import status_restore_monitor as monitor
from tools import systemd_subscription as native
from tools import subscription_process as worker
from tools import stage_device_prototype as transport
from tools.capture_pikvm_baseline import write_json_0600_atomic

CHECKS = {"subscriptionReady", "monitorReady", "managerIsPidOne", "noEventsObserved",
          "subscriptionChildExited", "monitorChildExited"}


def bootstrap(sources: dict[str, str]) -> str:
    return ("import sys,types\npackage=types.ModuleType('tools');package.__path__=[];sys.modules['tools']=package\n"
            f"sources={sources!r}\n"
            "for name,source in sources.items():\n"
            " module=types.ModuleType(name);module.__package__='tools';sys.modules[name]=module\n"
            " setattr(package,name.split('.')[-1],module)\n"
            " exec(compile(source,'<'+name+'>','exec'),module.__dict__)\n")


def program(operation: str) -> tuple[str, dict[str, str]]:
    policy.guard_name(operation)
    modules = (policy, properties, event, monitor, native, worker)
    sources = {module.__name__: Path(cast(str, module.__file__)).read_text() for module in modules}
    child_sources = {module.__name__: sources[module.__name__] for module in (policy, properties, event, native)}
    child = bootstrap(child_sources) + "sys.modules['tools.systemd_subscription'].subscription_worker()\n"
    source = bootstrap(sources) + f"worker_source={child!r}\noperation={operation!r}\n" + REMOTE
    if len(source.encode()) > 100000:
        raise ValueError("subscription preflight source too large")
    return source, {name: hashlib.sha256(value.encode()).hexdigest() for name, value in sources.items()}


REMOTE = r'''
import json, os, re, select, subprocess
from tools.subscription_process import SubscriptionProcess
from tools.status_restore_monitor import RestoreMonitor
from tools.status_restore_event import SYSTEM_BUS_ADDRESS
phase = "host"
try:
    if os.geteuid() != 0 or os.uname().machine != "aarch64":
        raise ValueError("unexpected host")
    phase = "subscribe"
    with SubscriptionProcess(worker_source) as subscription:
        phase = "monitor"
        with RestoreMonitor(operation) as observation:
            phase = "owner"
            base = ["/usr/bin/busctl", "--system", "--address=" + SYSTEM_BUS_ADDRESS,
                    "--json=short", "--timeout=5s", "call", "org.freedesktop.DBus",
                    "/org/freedesktop/DBus", "org.freedesktop.DBus"]
            def query(arguments):
                answer = subprocess.run(base + arguments, capture_output=True, timeout=6, check=True)
                if len(answer.stdout) > 2048:
                    raise ValueError("oversize bus owner reply")
                return json.loads(answer.stdout)
            reply = query(["GetNameOwner", "s", "org.freedesktop.systemd1"])
            if not isinstance(reply,dict) or set(reply)!={"type","data"} or reply["type"]!="s":
                raise ValueError("invalid manager owner reply")
            data=reply["data"]
            if not isinstance(data,list) or len(data)!=1 or not isinstance(data[0],str) or re.fullmatch(r":[0-9]{1,10}\.[0-9]{1,10}",data[0]) is None:
                raise ValueError("invalid manager owner")
            pid=query(["GetConnectionUnixProcessID","s",data[0]])
            if pid!={"type":"u","data":[1]} or type(pid["data"][0]) is not int:
                raise ValueError("systemd owner is not PID 1")
            phase="filtered-idle"
            subscription.ensure_alive()
            if observation.process.poll() is not None or select.select([observation.process.stdout],[],[],0.25)[0]:
                raise ValueError("unexpected monitor output")
        phase="monitor-exit"
        if observation.process.poll() is None:
            raise ValueError("monitor did not exit")
        subscription.ensure_alive()
    phase="subscription-exit"
    if subscription.process.returncode != 0:
        raise ValueError("subscription did not close cleanly")
    result={"result":"passed","operation":operation,"subscriptionReady":True,"monitorReady":True,
            "managerIsPidOne":True,"noEventsObserved":True,"subscriptionChildExited":True,"monitorChildExited":True}
except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as error:
    result={"result":"failed","operation":operation,"phase":phase,"errorType":type(error).__name__}
print(json.dumps(result,sort_keys=True))
raise SystemExit(0 if result["result"]=="passed" else 1)
'''


def validate(raw: bytes, operation: str) -> dict[str, object]:
    if len(raw) > 4096:
        raise ValueError("oversize subscription preflight receipt")
    value: object = json.loads(raw)
    if not isinstance(value, dict):
        raise ValueError("invalid subscription preflight receipt")
    result = cast(dict[str, object], value)
    if (set(result) != CHECKS | {"result", "operation"} or result["result"] != "passed"
            or result["operation"] != operation or any(result[key] is not True for key in CHECKS)):
        raise ValueError("subscription preflight not accepted")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    output = transport.validate_output_path(args.output)
    if output.exists():
        raise ValueError("new receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    operation = secrets.token_hex(12)
    source, hashes = program(operation)
    secret = transport.read_secret_stdin()
    receipt: dict[str, object] = {"result": "failed", "probe": "subscription-monitor-readonly-preflight",
                                "operation": operation, "sourceSha256": hashes}
    try:
        result = transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=60)
        receipt["sshExitCode"] = result.returncode
        if result.returncode == 0:
            receipt["observation"] = validate(result.stdout, operation)
            receipt["result"] = "passed"
        elif len(result.stdout) <= 4096:
            failure: object = json.loads(result.stdout)
            if isinstance(failure, dict):
                value = cast(dict[str, object], failure)
                phase = value.get("phase")
                if isinstance(phase, str) and phase in {"host", "subscribe", "monitor", "owner", "filtered-idle", "monitor-exit", "subscription-exit"}:
                    receipt["failurePhase"] = phase
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
