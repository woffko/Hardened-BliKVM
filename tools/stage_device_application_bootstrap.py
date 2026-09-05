#!/usr/bin/env python3
"""Guarded first persistent application slot, with an explicit rollback rehearsal."""

from __future__ import annotations

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

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import initialize_application_state as native
from tools import stage_device_prototype as transport
from tools import stage_device_unused_update_trust_removal as trust
from tools import stage_device_update_runtime_upgrade as updater
from tools import stage_device_update_trust_config as enrollment
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bundle", "update-package", "trust-receipt", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--preflight-only", action="store_true")
    parser.add_argument("--exercise-rollback", action="store_true")
    args = parser.parse_args()
    output = transport.validate_output_path(cast(str, args.output))
    require(not output.exists(), "new receipt required")
    source = Path(cast(str, args.bundle))
    require(source.is_file() and not source.is_symlink() and source.stat().st_size <= 128 * 1024 * 1024, "invalid source bundle")
    data = source.read_bytes()
    require(hashlib.sha256(data).hexdigest() == native.CURRENT_BUNDLE_SHA, "unaccepted bootstrap bundle")
    update = updater.package.validate_package(cast(str, args.update_package), *updater.NEW, expected_version="0.1.0~dev2")
    accepted = trust.load_enrollment_receipt(cast(str, args.trust_receipt), native.CURRENT_CONFIG_SHA)
    baseline = cutover.load_baseline("evidence/private/device-web-cutover-preflight.json")
    host, port = transport.parse_endpoint(cast(str, args.endpoint))
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    token = secrets.token_hex(12)
    stage = "/tmp/hardened-blikvm-bootstrap-host-" + token
    q = shlex.quote
    password = transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "persistent-application-bootstrap", "bundleSha256": native.CURRENT_BUNDLE_SHA}
    guard: str | None = None
    staged = False
    phase = "preflight"
    preflight = trust.build_preflight_script(accepted, baseline, update_version="0.1.0~dev2")
    preflight += "\nguards_absent\n" + updater.no_state() + updater.package.installed_checks(update) + """
test -z "$(find /etc/systemd/system -maxdepth 3 -name 'hardened-blikvm-state-init-*' -print -quit)"
test -z "$(find /var/lib -maxdepth 1 -name 'hardened-blikvm-state-init-*' -print -quit)"
test "$(df -B1 --output=avail /var/lib | tail -n 1 | tr -d ' ')" -gt 50000000
"""

    def call(action: str, *extra: str) -> dict[str, object]:
        command = shlex.join(["python3", "-B", stage + "/controller.py", action, *extra])
        result = transport.run_ssh(target, password, command, timeout=150)
        require(len(result.stdout) <= 16384, "oversized bootstrap response")
        observed = cast(dict[str, object], json.loads(result.stdout))
        receipt[phase] = observed
        if result.returncode != 0:
            failed_guard = observed.get("guardRoot")
            if isinstance(failed_guard, str):
                receipt["recoveryPath"] = failed_guard
            raise ValueError("native bootstrap failed")
        return observed

    try:
        before = transport.run_ssh(target, password, preflight, timeout=90)
        require(before.returncode == 0, "bootstrap preflight failed")
        before_facts = trust.parse_facts(before.stdout, trust.PRE_KEYS)
        receipt["before"] = before_facts
        if args.preflight_only:
            receipt.update({"result": "passed", "deviceChanged": False})
            return 0
        phase = "stage-controller"
        create = f"set -eu\numask 077\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}\nprintf '%s' {q(token)} > {q(stage + '/owner')}"
        require(transport.run_ssh(target, password, create).returncode == 0, "controller stage failed")
        staged = True
        uploads = (("controller.py", Path(native.__file__).read_bytes()),
                   ("application_bootstrap_roots.py", Path(__file__).with_name("application_bootstrap_roots.py").read_bytes()),
                   ("application.tar", data))
        for name, payload in uploads:
            script = f"set -eu\numask 077\ncat > {q(stage + '/' + name)}\ntest \"$(sha256sum {q(stage + '/' + name)} | cut -d ' ' -f 1)\" = {hashlib.sha256(payload).hexdigest()}"
            with tempfile.TemporaryFile() as source_file:
                source_file.write(payload)
                source_file.seek(0)
                require(transport.run_ssh(target, password, script, stdin=source_file, timeout=60).returncode == 0, "controller upload failed")
        phase = "prepare-roots"
        prepared = call("prepare", "--bundle", stage + "/application.tar")
        returned_guard = prepared.get("guardRoot")
        require(isinstance(returned_guard, str) and re.fullmatch(r"/var/lib/hardened-blikvm-state-init-[a-f0-9]{24}", returned_guard) is not None,
                "invalid bootstrap guard identity")
        guard = cast(str, returned_guard)
        phase = "independent-inspection"
        require(call("inspect", "--guard-root", guard).get("result") == "verified", "bootstrap inspection failed")
        values = trust.common_values(accepted, baseline, before_facts)
        audit = updater.assignments(values) + "\n" + enrollment.REMOTE_HELPERS + "\nset -eu\n"
        audit += updater.snapshot_checks() + "\nupdate_units_inert\n" + updater.package.installed_checks(update)
        audit += "\ntest ! -e /run/hardened-blikvm-update\ntest ! -e /run/hardened-blikvm-update-confirm\n"
        require(transport.run_ssh(target, password, audit, timeout=90).returncode == 0, "serving runtime changed")
        if args.exercise_rollback:
            phase = "rollback-rehearsal"
            restored = call("rollback", "--guard-root", guard)
            require(restored.get("result") == "rolled_back", "bootstrap rollback not confirmed")
            guard = None
            after = transport.run_ssh(target, password, preflight, timeout=90)
            require(after.returncode == 0 and trust.parse_facts(after.stdout, trust.PRE_KEYS) == before_facts,
                    "post-rollback baseline changed")
            receipt.update({"result": "passed", "rollbackExercised": True, "stateInitialized": False,
                            "quarantineRoot": restored["quarantineRoot"]})
        else:
            phase = "commit"
            require(call("commit", "--guard-root", guard).get("result") == "committed", "bootstrap commit failed")
            phase = "committed-inspection"
            require(call("inspect", "--guard-root", guard).get("result") == "verified", "committed state rejected")
            require(transport.run_ssh(target, password, audit, timeout=90).returncode == 0, "committed runtime changed")
            phase = "guard-cleanup"
            require(call("cleanup", "--guard-root", guard).get("result") == "cleaned", "guard cleanup failed")
            guard = None
            phase = "independent-final"
            require(call("status").get("result") == "verified", "final initialized state rejected")
            require(transport.run_ssh(target, password, audit, timeout=90).returncode == 0, "final runtime changed")
            receipt.update({"result": "passed", "stateInitialized": True, "slotId": native.SLOT})
        receipt.update({"runtimeUnchanged": True, "trustPreserved": True, "unitsEnabled": False, "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "failureStage": phase, "errorType": type(error).__name__})
        if guard:
            try:
                phase = "recovery"
                receipt["recovery"] = call("rollback", "--guard-root", guard)
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["recoveryIncomplete"] = True
        return 1
    finally:
        if staged:
            cleanup = f"""set -eu
test ! -L {q(stage)}
test "$(stat -c '%a:%u:%g' {q(stage)})" = 700:0:0
test "$(cat {q(stage + '/owner')})" = {q(token)}
test -z "$(find {q(stage)} -mindepth 1 -maxdepth 1 ! -name owner ! -name controller.py ! -name application_bootstrap_roots.py ! -name application.tar -print -quit)"
rm -f -- {q(stage + '/controller.py')} {q(stage + '/application_bootstrap_roots.py')} {q(stage + '/application.tar')} {q(stage + '/owner')}
rmdir {q(stage)}
"""
            try:
                staged = transport.run_ssh(target, password, cleanup, timeout=45).returncode != 0
            except (OSError, subprocess.SubprocessError):
                staged = True
        receipt["controllerStagingRemoved"] = not staged
        write_json_0600_atomic(output, receipt)
        for index in range(len(password)):
            password[index] = 0


if __name__ == "__main__":
    raise SystemExit(main())
