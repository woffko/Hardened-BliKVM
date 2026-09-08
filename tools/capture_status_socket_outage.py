#!/usr/bin/env python3
"""Coordinate the bounded authenticated browser status outage acceptance."""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import re
import secrets
import shlex
import socket
import subprocess
import sys
from typing import cast
from urllib.parse import urlsplit

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import capture_status_timer_firing as capture
from tools import status_socket_outage as outage
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin
from tools.status_outage_channel import PhaseChannel

timer = capture.timer
require = timer.ipc.require
PHASES = {"arm", "stop", "restore", "finish", "emergency"}


def browser_sources() -> dict[str, str]:
    names = ("device_existing_account_smoke", "status_api_acceptance", "status_soak", "status_outage_audit",
             "status_outage_scenario", "status_outage_control", "device_status_outage", "mjpeg_progress")
    return {name: hashlib.sha256(Path("tools/playwright", name + ".mjs").read_text().encode()).hexdigest() for name in names}


def program(operation: str, phase: str, state: object = None) -> tuple[str, dict[str, str]]:
    timer.policy.guard_name(operation)
    require(phase in PHASES, "invalid outage phase")
    sub = capture.subscription
    modules = (timer.resources, timer.ipc, timer.load, timer.policy, timer.properties,
               timer.metadata, timer.probe, sub.event, sub.monitor, sub.worker, capture.firing, outage)
    sources = {module.__name__: Path(cast(str, module.__file__)).read_text() for module in modules}
    status_source = sources[timer.ipc.__name__] + "\nprint(json.dumps(read_status(),sort_keys=True))\n"
    source = sub.bootstrap(sources)
    if phase == "arm":
        invocation = f"sys.modules['tools.status_timer_firing'].arm({operation!r},{status_source!r})"
    elif phase == "emergency":
        source += "sys.modules['tools.status_socket_outage'].emergency_restore()\n"
        invocation = repr({"result": "restored-unverified", "operation": operation, "guardMayRemain": True})
    else:
        state = capture.firing.validate_handoff(state, operation)
        invocation = f"sys.modules['tools.status_socket_outage'].{phase}({operation!r},{state!r},{status_source!r})"
    source += f"result={invocation}\nimport json\nprint(json.dumps(result,sort_keys=True))\n"
    source += "raise SystemExit(1 if result['result']=='failed' else 0)\n"
    require(len(source.encode()) < 110000, "outage source exceeds bound")
    return source, {name: hashlib.sha256(value.encode()).hexdigest() for name, value in sources.items()}


def validate_phase(value: object, operation: str, phase: str) -> dict[str, object]:
    if phase == "arm":
        return capture.firing.validate_handoff(value, operation)
    result = timer.ipc.object_value(value)
    checks = {"workersDrained", "servingIdentityUnchanged"} if phase == "stop" else {"statusUnchanged", "servingIdentityUnchanged"}
    expected = {"stop": "stopped", "restore": "restored", "finish": "finished"}.get(phase)
    require(expected is not None and set(result) == checks | {"result", "operation", "guardMayRemain"}
            and result["result"] == expected and result["operation"] == operation
            and result["guardMayRemain"] is (phase != "finish")
            and all(result[key] is True for key in checks), "outage phase not verified")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--endpoint", required=True)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    transport = timer.transport
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new outage receipt required")
    host, port = transport.parse_endpoint(args.endpoint)
    url = urlsplit(cast(str, args.base_url))
    require(url.scheme == "https" and url.hostname == host and not url.username and not url.password
            and url.path in ("", "/") and not url.query and not url.fragment, "invalid device browser URL")
    require(re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", args.username) is not None, "invalid browser username")
    base_url = args.base_url.rstrip("/")
    operation = secrets.token_hex(12)
    browser_output = output.with_name("status-outage-browser-" + operation + ".json")
    require(not browser_output.exists(), "new browser receipt required")
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    _, hashes = program(operation, "arm")
    browser_hashes = browser_sources()
    receipt: dict[str, object] = {"probe": "status-socket-browser-outage", "operation": operation,
                                "result": "failed", "sourceSha256": hashes, "browserSourceSha256": browser_hashes,
                                "guardMayRemain": False}
    handoff: dict[str, object] | None = None
    attempted = False
    browser: subprocess.Popen[bytes] | None = None
    parent, child = socket.socketpair()
    parent.settimeout(120)
    try:
        with secret_pair_stdin() as pair:
            ssh = pair["ssh"]
            require(len(ssh) <= transport.MAX_SECRET_BYTES + 1, "invalid SSH secret size")
            if ssh.endswith(b"\n"):
                del ssh[-1:]
            if ssh.endswith(b"\r"):
                del ssh[-1:]
            require(bool(ssh) and len(ssh) <= transport.MAX_SECRET_BYTES
                    and not any(mark in ssh for mark in (b"\n", b"\r", b"\0")), "invalid SSH secret")
            require(0 < len(pair["gui"]) <= 4096, "invalid browser secret size")

            def remote(phase: str) -> dict[str, object]:
                if phase == "emergency":
                    # Recovery must remain possible if probe sources changed.
                    completed = transport.run_ssh(target, ssh, shlex.join(timer.policy.restore_command()), timeout=20)
                else:
                    source, current_hashes = program(operation, phase, handoff)
                    require(current_hashes == hashes, "outage sources changed")
                    completed = transport.run_ssh(target, ssh, shlex.join(["python3", "-B", "-c", source]),
                                                  timeout={"arm": 35, "stop": 30, "restore": 35, "finish": 70}[phase])
                require(completed.returncode == 0 and len(completed.stdout) <= 32768, "outage SSH phase failed")
                if phase == "emergency":
                    return {"restorationRequested": True}
                return validate_phase(json.loads(completed.stdout), operation, phase)

            try:
                browser = subprocess.Popen([
                    "node", "tools/playwright/device_status_outage.mjs", str(child.fileno()),
                    "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
                    "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
                    base_url, args.username, str(browser_output),
                    "hardened-blikvm-status-api-" + operation + "-enabled", "enabled",
                ], stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   pass_fds=(child.fileno(),), close_fds=True)
                child.close()
                if browser.stdin is None:
                    raise ValueError("browser stdin unavailable")
                browser.stdin.write(pair["gui"])
                browser.stdin.close()
                channel = PhaseChannel(parent)
                while True:
                    request = channel.receive()
                    if request is None:
                        break
                    request_id, phase = request
                    accepted = False
                    try:
                        if phase == "stop":
                            attempted = True
                            receipt["guardMayRemain"] = True
                            write_json_0600_atomic(output, receipt)
                            handoff = remote("arm")
                            receipt["handoff"] = handoff
                            # Durable handoff precedes any listener-stop request.
                            write_json_0600_atomic(output, receipt)
                        require(handoff is not None, "missing outage handoff")
                        require(browser.poll() is None, "browser exited before outage phase")
                        receipt[phase] = remote(phase)
                        accepted = True
                    except (OSError, ValueError, KeyError, subprocess.SubprocessError):
                        receipt["phaseFailure"] = phase
                    write_json_0600_atomic(output, receipt)
                    channel.reply(request_id, phase, accepted)
                require(browser.wait(timeout=15) == 0, "browser outage process failed")
                require(browser_sources() == browser_hashes, "browser sources changed")
                require(browser_output.stat().st_size <= 32768, "oversize browser receipt")
                result = timer.ipc.object_value(json.loads(browser_output.read_bytes()))
                require(result.get("result") == "passed" and result.get("runTag") == "hardened-blikvm-status-api-" + operation + "-enabled",
                        "browser outage acceptance failed")
                facts = timer.ipc.object_value(result.get("statusOutageAcceptance"))
                require(set(facts) == {"staleVerified", "coldErrorVerified", "recoveryVerified", "videoPreserved"}
                        and all(value is True for value in facts.values()), "browser outage facts incomplete")
                require(handoff is not None and "stop" in receipt and "restore" in receipt and "phaseFailure" not in receipt,
                        "outage phases incomplete")
                receipt["browser"] = {"result": "passed", "acceptance": facts}
                receipt["finish"] = remote("finish")
                receipt["guardMayRemain"] = False
                receipt["result"] = "passed"
            finally:
                try:
                    if browser is not None and browser.poll() is None:
                        browser.terminate()
                        try:
                            browser.wait(timeout=8)
                        except subprocess.TimeoutExpired:
                            browser.kill()
                            browser.wait(timeout=5)
                finally:
                    if attempted and receipt["guardMayRemain"] is True:
                        # Missing acknowledgement never authorizes another stop
                        # or timer removal, even if browser termination failed.
                        try:
                            receipt["emergency"] = remote("emergency")
                        except (OSError, ValueError, KeyError, subprocess.SubprocessError):
                            receipt["emergency"] = {"restorationRequested": False}
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        receipt["errorType"] = type(error).__name__
    finally:
        parent.close()
        child.close()
        write_json_0600_atomic(output, receipt)
    if receipt["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
