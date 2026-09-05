#!/usr/bin/env python3
"""Drive one bounded reversible installed status API acceptance transaction."""

from __future__ import annotations

import argparse
from collections.abc import Callable
import hashlib
import json
from pathlib import Path
import re
import secrets
import shlex
import subprocess
import sys
import tempfile
import time
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as transport
from tools import status_api_wiring as native
from tools import persistent_status_wiring as persistent_native
from tools.stage_installed_application_update import browser_gate
from tools.capture_pikvm_baseline import write_json_0600_atomic

require = native.require


def validate_guard(value: object, *, persistent: bool = False) -> str:
    prefix = persistent_native.PREFIX if persistent else native.PREFIX
    require(isinstance(value, str) and re.fullmatch(r"/var/lib/" + prefix + "[a-f0-9]{24}", value) is not None,
            "invalid returned status guard")
    return cast(str, value)


def checkpoint(path: str, *, persistent: bool = False) -> str:
    value = cast(dict[str, object], json.loads(transport.validate_private_file(path).read_bytes()))
    probe = "persistent-status-api-wiring" if persistent else "installed-status-api-wiring"
    require(value.get("probe") == probe and value.get("slotId") == native.SLOT,
            "status checkpoint mismatch")
    return validate_guard(value.get("guardRoot"), persistent=persistent)


def persistent_browser_gate(path: str | None, root: str) -> None:
    browser_gate(path, root, "enabled")
    value = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, path)).read_bytes()))
    require(value.get("statusAcceptance") == {"enabled": True, "desktop": True, "mobile": True,
                                              "repeatedReads": 3, "refreshChecks": 2}, "persistent status browser proof incomplete")


def boot_acceptance(boot_path: str | None, browser_path: str | None, cleanup_path: str | None) -> str:
    require(boot_path is not None and cleanup_path is not None, "boot and cleanup acceptance receipts required")
    root = checkpoint(cast(str, boot_path), persistent=True)
    boot = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, boot_path)).read_bytes()))
    proof = cast(dict[str, object], boot.get("device", {}))
    require(boot.get("result") == "passed" and boot.get("action") == "reboot-test"
            and boot.get("softwareRebootAttempted") is True and proof.get("result") == "verify-boot"
            and proof.get("guardRoot") == root and proof.get("bootChanged") is True
            and proof.get("restoredBeforeServer") is True and proof.get("runtimeVerified") is True
            and proof.get("controllerSha256") == hashlib.sha256(Path(persistent_native.__file__).read_bytes()).hexdigest(),
            "boot acceptance does not match this controller")
    status = cast(dict[str, object], proof.get("status", {}))
    require(status.get("floorSequence") == 5 and status.get("generation") == 22 and status.get("pending") is None
            and status.get("activeSlotId") == status.get("confirmedSlotId") == native.SLOT, "boot application state mismatch")
    browser_gate(browser_path, root, "restored")
    browser = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, browser_path)).read_bytes()))
    require(browser.get("statusAcceptance") == {"enabled": False, "desktop": True, "mobile": True,
                                                "repeatedReads": 0, "refreshChecks": 0}, "post-boot browser proof incomplete")
    require(checkpoint(cast(str, cleanup_path), persistent=True) == root, "boot cleanup belongs to another guard")
    cleanup = cast(dict[str, object], json.loads(transport.validate_private_file(cast(str, cleanup_path)).read_bytes()))
    cleaned = cast(dict[str, object], cleanup.get("device", {}))
    require(cleanup.get("result") == "passed" and cleanup.get("action") == "cleanup"
            and cleanup.get("controllerStagingRemoved") is True and cleaned.get("result") == "cleanup"
            and cleaned.get("phase") == "restored" and cleaned.get("runtimeVerified") is True, "boot cleanup incomplete")
    return root


def request_one_reboot(reboot: Callable[[], int], receipt: dict[str, object]) -> None:
    receipt["softwareRebootAttempted"] = True
    try:
        status = reboot()
        require(status in (0, 255), "reboot request rejected")
        receipt["rebootRequestOutcome"] = "accepted" if status == 0 else "connection-closed"
    except subprocess.TimeoutExpired:
        receipt["rebootRequestOutcome"] = "observation-timeout"


def reboot_test(invoke: Callable[[str], tuple[int, dict[str, object]]], reboot: Callable[[], int],
                receipt: dict[str, object]) -> dict[str, object]:
    code, before = invoke("preflight-reboot")
    require(code == 0 and before.get("result") == "preflight-reboot", "guarded reboot preflight failed")
    for field in ("bootIdSha256", "controllerSha256"):
        value = before.get(field)
        require(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None, "invalid reboot preflight binding")
    validate_guard(before.get("guardRoot"), persistent=True)
    receipt["rebootBefore"] = before
    request_one_reboot(reboot, receipt)
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        try:
            code, after = invoke("verify-boot")
            if code == 0 and after.get("result") == "verify-boot":
                require(after.get("bootChanged") is True and after.get("restoredBeforeServer") is True
                        and after.get("runtimeVerified") is True
                        and after.get("guardRoot") == before.get("guardRoot")
                        and after.get("beforeBootIdSha256") == before.get("bootIdSha256")
                        and after.get("afterBootIdSha256") != before.get("bootIdSha256")
                        and after.get("controllerSha256") == before.get("controllerSha256"), "boot restoration proof mismatch")
                return after
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
        time.sleep(3)
    raise ValueError("boot recovery not verified within bounded observation window; do not repeat reboot")


def compare_retained_boot(before: dict[str, object], after: dict[str, object]) -> None:
    for value in (before.get("bootIdSha256"), after.get("bootIdSha256")):
        require(isinstance(value, str) and re.fullmatch(r"[a-f0-9]{64}", value) is not None, "invalid retained boot identity")
    require(after.get("result") == "inspect-retained" and after.get("persistentWiringVerified") is True
            and after.get("startupOrderVerified") is True and after.get("guardRemoved") is True, "retained runtime not verified")
    require(after.get("bootIdSha256") != before.get("bootIdSha256") and after.get("guardRoot") == before.get("guardRoot"),
            "retained reboot identity mismatch")
    require(after["dropinHashes"] == before["dropinHashes"], "persistent drop-ins changed on reboot")
    old = cast(dict[str, object], before["snapshot"])
    new = cast(dict[str, object], after["snapshot"])
    for field in ("state", "managerStatus", "nginxUnit"):
        require(new[field] == old[field], "retained application state changed")
    old_runtime = cast(dict[str, object], old["runtime"])
    new_runtime = cast(dict[str, object], new["runtime"])
    for field in ("account", "nginxConfig", "certificate", "tlsKey", "trust"):
        require(new_runtime[field] == old_runtime[field], "retained configuration changed")
    require(new_runtime["brokerRestarts"] == new_runtime["videoRestarts"] == "0", "runtime peer restarted after boot")


def retained_reboot_test(invoke: Callable[[str], tuple[int, dict[str, object]]], reboot: Callable[[], int],
                         receipt: dict[str, object]) -> dict[str, object]:
    code, before = invoke("inspect-retained")
    require(code == 0 and before.get("result") == "inspect-retained", "retained reboot preflight failed")
    receipt["rebootBefore"] = before
    request_one_reboot(reboot, receipt)
    deadline = time.monotonic() + 240
    while time.monotonic() < deadline:
        try:
            code, after = invoke("inspect-retained")
            if code == 0:
                compare_retained_boot(before, after)
                receipt["persistentRebootVerified"] = True
                return after
        except (OSError, ValueError, subprocess.SubprocessError):
            pass
        time.sleep(3)
    raise ValueError("retained reboot verification timed out; do not repeat reboot")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("preflight", "prepare", "inspect", "restore", "commit", "cleanup", "verify-clean", "verify-boot", "reboot-test", "inspect-retained", "retained-reboot", "recover-serving"))
    parser.add_argument("--persistent", action="store_true")
    parser.add_argument("--browser-receipt")
    parser.add_argument("--boot-recovery-receipt")
    parser.add_argument("--boot-browser-receipt")
    parser.add_argument("--boot-cleanup-receipt")
    for name in ("endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--guard-receipt")
    args = parser.parse_args()
    backend = persistent_native if args.persistent else native
    require(args.action not in ("preflight", "commit", "verify-boot", "reboot-test", "inspect-retained", "retained-reboot", "recover-serving") or args.persistent, "action requires persistent mode")
    require(args.action != "verify-clean" or not args.persistent, "persistent clean verification is not implemented")
    output = transport.validate_output_path(args.output)
    require(not output.exists(), "new status receipt required")
    guard = None if args.action in ("prepare", "preflight") else checkpoint(args.guard_receipt, persistent=args.persistent)
    if args.action == "commit":
        boot_acceptance(args.boot_recovery_receipt, args.boot_browser_receipt, args.boot_cleanup_receipt)
        persistent_browser_gate(args.browser_receipt, cast(str, guard))
    uploads = {name: Path(__file__).with_name(name).read_bytes() for name in backend.HELPERS} if args.action in ("prepare", "preflight", "verify-clean", "inspect-retained", "retained-reboot", "recover-serving") else {}
    if args.action in ("inspect-retained", "retained-reboot", "recover-serving"):
        uploads["capture_retained_status.py"] = Path(__file__).with_name("capture_retained_status.py").read_bytes()
    host, port = transport.parse_endpoint(args.endpoint)
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    secret = transport.read_secret_stdin()
    stage = ("/var/lib/hardened-blikvm-status-observer-" if args.action == "retained-reboot" else "/tmp/blikvm-status-wiring-controller-") + secrets.token_hex(12)
    q = shlex.quote
    staged = False
    receipt: dict[str, object] = {"probe": "persistent-status-api-wiring" if args.persistent else "installed-status-api-wiring", "action": args.action,
                                  "slotId": native.SLOT, "targetPowerControl": False}

    def invoke(action: str) -> tuple[int, dict[str, object]]:
        script_name = "persistent_status_wiring.py" if args.persistent else "status_api_wiring.py"
        argv = ["python3", "-B", stage + "/" + script_name, action, "--assets", stage] if action in ("prepare", "preflight") else [
            "python3", "-B", cast(str, guard) + "/controller.py", action, "--guard-root", cast(str, guard)]
        if action == "verify-clean":
            argv = ["python3", "-B", stage + "/status_api_wiring.py", action, "--guard-root", cast(str, guard)]
        if action in ("inspect-retained", "recover-serving"):
            argv = ["python3", "-B", stage + "/capture_retained_status.py", "--guard-root", cast(str, guard)]
            if action == "recover-serving":
                argv.append("--recover-serving")
        if action == "commit":
            argv.append("--browser-verified")
        result = transport.run_ssh(target, secret, shlex.join(argv), timeout=30 if action in ("verify-boot", "inspect-retained") else 180)
        if action == "inspect-retained" and result.returncode != 0:
            known = ("persistent guard remains", "persistent recovery unit remains", "runtime overrides remain",
                     "foreign persistent override directory", "persistent override changed", "unexpected installed updater version",
                     "installed status unit changed", "loaded status environment mismatch", "running status environment mismatch",
                     "status socket was enabled", "status socket not active", "unsafe status socket",
                     "application journal generation changed", "status or recovery did not precede server",
                     "operational application state changed", "updater service not enabled/active", "watchdog invocation failed",
                     "unsafe updater runtime ownership/mode", "unexpected active update transaction", "tmpfiles configuration changed",
                     "serving service unavailable", "serving executable changed", "ModuleNotFoundError", "FileNotFoundError")
            stderr = (result.stderr or b"").decode(errors="replace")
            receipt["observerErrorClasses"] = [label for label in known if label in stderr]
            receipt["observerExitCode"] = result.returncode
        return result.returncode, cast(dict[str, object], json.loads(result.stdout)) if result.stdout else {}

    try:
        if uploads:
            require(transport.run_ssh(target, secret, f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                    "status controller staging failed")
            staged = True
            for name, data in uploads.items():
                destination = q(stage + "/" + name)
                command = f"set -eu\numask 077\ncat > {destination}\ntest \"$(sha256sum {destination} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}"
                with tempfile.TemporaryFile() as stream:
                    stream.write(data)
                    stream.seek(0)
                    require(transport.run_ssh(target, secret, command, stdin=stream, timeout=60).returncode == 0, "status controller upload failed")
        if args.action in ("reboot-test", "retained-reboot"):
            def reboot() -> int:
                return transport.run_ssh(target, secret, "systemctl reboot --no-block", timeout=20).returncode

            result = (reboot_test if args.action == "reboot-test" else retained_reboot_test)(invoke, reboot, receipt)
            code = 0
        else:
            code, result = invoke(args.action)
        receipt["device"] = result
        if "guardRoot" in result:
            guard = validate_guard(result["guardRoot"], persistent=args.persistent)
        if guard:
            receipt["guardRoot"] = guard
        expected_result = {"prepare": "prepared", "reboot-test": "verify-boot", "retained-reboot": "inspect-retained"}.get(args.action, args.action)
        require(code == 0 and result.get("result") == expected_result, "status device step failed")
        if args.action in ("prepare", "restore", "reboot-test", "retained-reboot"):
            receipt["browserRunTag"] = Path(cast(str, guard)).name + ("-enabled" if args.action in ("prepare", "retained-reboot") else "-restored")
        receipt["result"] = "passed"
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "errorType": type(error).__name__})
        if guard is not None and args.action == "prepare":
            try:
                code, restored = invoke("restore")
                receipt.update({"recovery": restored, "recoverySucceeded": code == 0})
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["recoverySucceeded"] = False
        return 1
    finally:
        if staged:
            allowed = " ".join("! -name " + q(name) for name in uploads)
            exact = " ".join(q(stage + "/" + name) for name in uploads)
            command = f"set -eu\ntest ! -L {q(stage)}\ntest \"$(stat -c '%a:%u:%g' {q(stage)})\" = 700:0:0\ntest -z \"$(find {q(stage)} -mindepth 1 -maxdepth 1 {allowed} -print -quit)\"\nrm -f -- {exact}\nrmdir {q(stage)}"
            try:
                staged = transport.run_ssh(target, secret, command).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        secret[:] = b"\0" * len(secret)


if __name__ == "__main__":
    raise SystemExit(main())
