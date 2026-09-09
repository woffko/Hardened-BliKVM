#!/usr/bin/env python3
"""Run the fixed application preflight in memory; no remote staging files."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
from pathlib import Path
import shlex
import subprocess
import sys
import zlib
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import application_update_preflight as probe
from tools import capture_status_restore_timer as timer
from tools import capture_subscription_preflight as subscription
from tools import stage_installed_application_update as driver
from tools.capture_pikvm_baseline import write_json_0600_atomic


def program(bindings: dict[str, object]) -> tuple[str, dict[str, str]]:
    update = probe.update
    modules = ["application_bootstrap_roots", "initialize_application_state", "switch_signed_slot_server",
               "upgrade_initialized_updater", "enable_update_services", "status_api_wiring", "installed_application_update"]
    sources = {"tools." + name: Path(__file__).with_name(name + ".py").read_text() for name in modules}
    for module in (timer.resources, timer.ipc, timer.load, timer.policy, timer.properties, timer.metadata, timer.probe, probe):
        sources[module.__name__] = Path(cast(str, module.__file__)).read_text()
    update.require(len(json.dumps(bindings).encode()) <= 20000, "oversize updater bindings")
    status_source = sources[timer.ipc.__name__] + "\nprint(json.dumps(read_status(),sort_keys=True))\n"
    source = subscription.bootstrap(sources)
    source += f"value=sys.modules['tools.application_update_preflight'].verify({bindings!r},{status_source!r})\n"
    source += "import json\nprint(json.dumps(value,sort_keys=True))\n"
    update.require(len(source.encode()) <= 250000, "oversize preflight program")
    encoded = base64.b64encode(zlib.compress(source.encode(), 9)).decode("ascii")
    command = f"import base64,zlib;exec(compile(zlib.decompress(base64.b64decode({encoded!r})),'<application-preflight>','exec'))"
    update.require(len(command) <= 100000, "oversize compressed preflight program")
    return command, {name: hashlib.sha256(value.encode()).hexdigest() for name, value in sources.items()}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    output = driver.transport.validate_output_path(args.output)
    probe.require(not output.exists(), "new preflight receipt required")
    uploads = driver.packages.upload_payloads(args.old_package, args.new_package)
    bindings = timer.ipc.object_value(json.loads(uploads["bindings.json"]))
    source, hashes = program(bindings)
    host, port = driver.transport.parse_endpoint(args.endpoint)
    target = driver.transport.RemoteTarget(host, port, "root", driver.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = driver.transport.read_secret_stdin()
    result: dict[str, object] = {"probe": "application-update-preflight", "result": "failed", "sourceSha256": hashes}
    try:
        response = driver.transport.run_ssh(target, secret, shlex.join(["python3", "-B", "-c", source]), timeout=120)
        probe.require(response.returncode == 0 and len(response.stdout) <= 8192, "application preflight SSH failed")
        value = timer.ipc.object_value(json.loads(response.stdout))
        checks = {"installedUpdaterVerified", "persistentStatusVerified", "sourceSlotVerified", "transactionAbsent", "stateAndPeersUnchanged"}
        probe.require(set(value) == checks | {"result", "profile", "status", "deviceWrites"}
                      and value["result"] == "passed" and value["profile"] == "seq5-to-seq6-mjpeg"
                      and all(value[key] is True for key in checks) and value["deviceWrites"] is False, "incomplete application preflight")
        timer.load.retained_status(value["status"])
        result.update({"result": "passed", "device": value})
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        result["errorType"] = type(error).__name__
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, result)
    if result["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
