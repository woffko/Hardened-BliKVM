#!/usr/bin/env python3
"""Upgrade the unused updater dev1 to dev2, preserving enrolled public trust."""

from __future__ import annotations

import argparse
import base64
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as transport
from tools import stage_device_update_runtime_package as package
from tools import stage_device_unused_update_trust_removal as trust
from tools import stage_device_update_trust_config as enrollment
from tools import stage_device_web_cutover as cutover
from tools.capture_pikvm_baseline import write_json_0600_atomic


OLD = ("a42ebcc0ee9e0f4c2e0ad3ca313e783261a41fad3317358da7a0bd0effa9da20",
       "531f5f1e0a818d6c18ec86dbd99ac59f4f8ff4f4",
       "bc9a6e8a1131b78b86c347a2f38ea6352e89953e3d5002660de8a0438bc5633d",
       "17d8562a9508b87868a1d6b9bd97cd540f579421")
NEW = ("b3c385adfbcdbe9d1932982b71a9551f7325f9b8467fc632e194009fc822c4e8",
       "e1e80628226cb3cccf02d8a99279d56c793166e8",
       "7345a9aec28ae0655dfc59ffd374c0a60e465b82f2b1bd289d7615d6f0d95bca",
       "e1e80628226cb3cccf02d8a99279d56c793166e8")


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def assignments(values: dict[str, str | int]) -> str:
    return "\n".join(f"{name}={shlex.quote(str(value))}" for name, value in values.items())


def no_state() -> str:
    return """
for path in "$update_state" "$slot_root" "$update_runtime" "$update_socket_runtime"; do
  test ! -e "$path" && test ! -L "$path"
done
update_units_inert
test -z "$(systemctl list-units --all --no-legend --plain --state=active,activating,deactivating,reloading 'hardened-blikvm-update-watchdog*' 'hardened-blikvm-update-confirm*' 'hardened-blikvm-update-activate*' 'hardened-blikvm-update-apply*' 'hardened-blikvm-server-current*' 'hardened-blikvm-server-candidate*')"
test ! -L "$config_target"
test "$(stat -c '%a:%u:%g:%h' "$config_target")" = 600:0:0:1
test "$(sha256sum "$config_target" | cut -d ' ' -f 1)" = "$config_sha256"
"""


def snapshot_checks() -> str:
    return package.runtime_unchanged_checks() + """
persistent_runtime_healthy
test "$(sha256sum "$state_root/account.json" | cut -d ' ' -f 1)" = "$account_sha256"
test "$(sha256sum /etc/hardened-blikvm/nginx.conf | cut -d ' ' -f 1)" = "$nginx_config_sha256"
test "$(sha256sum "$certificate_path" | cut -d ' ' -f 1)" = "$certificate_sha256"
test "$(sha256sum "$key_path" | cut -d ' ' -f 1)" = "$key_sha256"
"""


def cleanup(paths: package.Paths) -> str:
    q = shlex.quote
    return f"""
test ! -L "$stage_root"
test "$(stat -c '%a:%u:%g' "$stage_root")" = 700:0:0
test ! -L "$marker" && test "$(cat "$marker")" = "$marker_value"
systemctl disable --now {q(paths.timer)} >/dev/null 2>&1 || true
systemctl disable {q(paths.boot_service)} >/dev/null 2>&1 || true
rm -f -- {q(paths.timer_link)} {q(paths.boot_service_link)} {q(paths.timer_path)} {q(paths.rollback_service_path)} {q(paths.boot_service_path)}
systemctl daemon-reload
rm -f -- "$stage_root/new.deb" "$stage_root/old.deb" "$stage_root/new.deb.part" "$stage_root/old.deb.part" "$stage_root/install.log" "$stage_root/committed" "$stage_root/committed.part" "$stage_root/rollback.sh" "$stage_root/rollback.sh.part" "$marker"
rmdir "$stage_root"
"""


def scripts(paths: package.Paths, old: package.PackageInfo, new: package.PackageInfo,
            accepted: trust.AcceptedEnrollment, baseline: dict[str, str], before: dict[str, str]) -> tuple[str, str, str]:
    values = trust.common_values(accepted, baseline, before)
    values.update({"stage_root": paths.root, "marker": paths.marker,
                   "marker_value": f"{paths.run_id}:{old.sha256}:{new.sha256}:{accepted.config_sha256}",
                   "old_sha256": old.sha256, "new_sha256": new.sha256})
    prefix = assignments(values) + "\n" + enrollment.REMOTE_HELPERS + "\nset -eu\nexport LC_ALL=C\n"
    lock = """
test ! -L "$stage_root"
test "$(stat -c '%a:%u:%g' "$stage_root")" = 700:0:0
exec 9<"$stage_root"
flock -w 30 -x 9
test ! -L "$marker" && test "$(cat "$marker")" = "$marker_value"
"""
    rollback = prefix + """
test "$#" = 1
case "$1" in boot|timer|explicit) ;; *) exit 2 ;; esac
""" + lock + no_state() + """
if [ ! -e "$stage_root/committed" ]; then
  test ! -L "$stage_root/old.deb"
  test "$(stat -c '%a:%u:%g:%h' "$stage_root/old.deb")" = 600:0:0:1
  test "$(sha256sum "$stage_root/old.deb" | cut -d ' ' -f 1)" = "$old_sha256"
  installed=$(dpkg-query -W -f='${Version}' "$package_name")
  case "$installed" in '0.1.0~dev1'|'0.1.0~dev2') ;; *) exit 1 ;; esac
  timeout 120 dpkg --force-confold -i "$stage_root/old.deb" > "$stage_root/install.log" 2>&1
  systemctl daemon-reload
  test "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}' "$package_name")" = 'ii :0.1.0~dev1'
""" + package.installed_checks(old) + "\nelse\n" + """
test ! -L "$stage_root/committed"
test "$(stat -c '%a:%u:%g:%h' "$stage_root/committed")" = 600:0:0:1
test "$(cat "$stage_root/committed")" = "$marker_value"
test "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}' "$package_name")" = 'ii :0.1.0~dev2'
""" + package.installed_checks(new) + "\nfi\n" + no_state() + """
test -z "$(dpkg --audit)"
if [ "$1" != boot ]; then
""" + snapshot_checks() + "\nfi\n" + cleanup(paths)
    apply = prefix + lock + no_state() + snapshot_checks() + """
test ! -e "$stage_root/committed"
test "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}' "$package_name")" = 'ii :0.1.0~dev1'
""" + package.installed_checks(old) + """
test ! -L "$stage_root/new.deb"
test "$(stat -c '%a:%u:%g:%h' "$stage_root/new.deb")" = 600:0:0:1
test "$(sha256sum "$stage_root/new.deb" | cut -d ' ' -f 1)" = "$new_sha256"
timeout 120 dpkg --force-confold -i "$stage_root/new.deb" > "$stage_root/install.log" 2>&1
systemctl daemon-reload
test "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}' "$package_name")" = 'ii :0.1.0~dev2'
""" + package.installed_checks(new) + no_state() + snapshot_checks() + "\ntest -z \"$(dpkg --audit)\"\n"
    commit = prefix + lock + package.installed_checks(new) + no_state() + snapshot_checks() + """
test "$(dpkg-query -W -f='${db:Status-Abbrev}:${Version}' "$package_name")" = 'ii :0.1.0~dev2'
printf '%s' "$marker_value" > "$stage_root/committed.part"
chmod 600 "$stage_root/committed.part"
sync "$stage_root/committed.part"
mv "$stage_root/committed.part" "$stage_root/committed"
sync "$stage_root"
""" + cleanup(paths)
    return rollback, apply, commit


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("old-package", "new-package", "trust-receipt", "expected-config-sha256", "endpoint", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--preflight-only", action="store_true")
    parser.add_argument("--exercise-rollback", action="store_true")
    args = parser.parse_args()
    old = package.validate_package(cast(str, args.old_package), *OLD, expected_version="0.1.0~dev1")
    new = package.validate_package(cast(str, args.new_package), *NEW, expected_version="0.1.0~dev2")
    accepted = trust.load_enrollment_receipt(cast(str, args.trust_receipt), cast(str, args.expected_config_sha256))
    output = transport.validate_output_path(cast(str, args.output))
    require(not output.exists(), "new receipt required")
    baseline = cutover.load_baseline("evidence/private/device-web-cutover-preflight.json")
    host, port = transport.parse_endpoint(cast(str, args.endpoint))
    target = transport.RemoteTarget(host, port, "root", transport.validate_private_file("evidence/private/ssh-known-hosts"))
    paths = package.paths(new.sha256, secrets.token_hex(8))
    password = transport.read_secret_stdin()
    receipt: dict[str, object] = {"probe": "update-runtime-dev2-upgrade", "oldPackageSha256": old.sha256,
                                  "newPackageSha256": new.sha256, "trustConfigSha256": accepted.config_sha256}
    phase = "preflight"
    staged = False
    armed = False
    rollback = ""
    marker = f"{paths.run_id}:{old.sha256}:{new.sha256}:{accepted.config_sha256}"
    try:
        preflight = trust.build_preflight_script(accepted, baseline, update_version="0.1.0~dev1")
        preflight += "\nguards_absent\n" + no_state() + package.installed_checks(old)
        result = transport.run_ssh(target, password, preflight, timeout=90)
        require(result.returncode == 0, "preflight failed")
        before = trust.parse_facts(result.stdout, trust.PRE_KEYS)
        receipt["preflight"] = before
        if args.preflight_only:
            receipt.update({"result": "passed", "deviceChanged": False})
            return 0
        rollback, apply, commit = scripts(paths, old, new, accepted, baseline, before)
        phase = "stage"
        q = shlex.quote
        prepare = f"set -eu\numask 077\ntest ! -e {q(paths.root)} && test ! -L {q(paths.root)}\nmkdir -m 700 {q(paths.root)}\nprintf '%s' {q(marker)} > {q(paths.marker)}"
        require(transport.run_ssh(target, password, prepare).returncode == 0, "stage failed")
        staged = True
        for artifact, name in ((old, "old.deb"), (new, "new.deb")):
            script = f"set -eu\numask 077\ncat > {q(paths.root + '/' + name + '.part')}\ntest \"$(sha256sum {q(paths.root + '/' + name + '.part')} | cut -d ' ' -f 1)\" = {artifact.sha256}\nmv {q(paths.root + '/' + name + '.part')} {q(paths.root + '/' + name)}"
            with artifact.path.open("rb") as source:
                require(transport.run_ssh(target, password, script, stdin=source, timeout=60).returncode == 0, "upload failed")
        phase = "arm"
        arm = "set -eu\numask 077\n"
        for name, content in ((paths.rollback_script, rollback), (paths.boot_service_path, package.build_boot_service(paths)),
                              (paths.rollback_service_path, package.build_rollback_service(paths)), (paths.timer_path, package.build_timer(paths))):
            arm += f"test ! -e {q(name)} && test ! -L {q(name)}\nprintf '%s' {q(base64.b64encode(content.encode()).decode())} | base64 -d > {q(name)}\nchmod 600 {q(name)}\nsync {q(name)}\n"
        arm += f"systemctl daemon-reload\nsystemctl enable {q(paths.boot_service)}\nsystemctl enable --now {q(paths.timer)}\nsystemctl is-active --quiet {q(paths.timer)}\nsync\n"
        armed = True
        require(transport.run_ssh(target, password, arm, timeout=60).returncode == 0, "arming failed")
        phase = "apply"
        require(transport.run_ssh(target, password, apply, timeout=180).returncode == 0, "upgrade failed")
        if args.exercise_rollback:
            phase = "rollback-rehearsal"
            result = transport.run_ssh(target, password, f"sh {q(paths.rollback_script)} explicit", timeout=180)
            require(result.returncode == 0, "rollback rehearsal failed")
            staged = armed = False
            phase = "rollback-independent-final"
            result = transport.run_ssh(target, password, preflight, timeout=90)
            require(result.returncode == 0 and trust.parse_facts(result.stdout, trust.PRE_KEYS) == before,
                    "rollback final state mismatch")
            receipt.update({"result": "passed", "rollbackExercised": True, "retainedVersion": "0.1.0~dev1",
                            "trustPreserved": True, "runtimeUnchanged": True, "stateInitialized": False,
                            "unitsEnabled": False, "targetPowerControl": False})
            return 0
        phase = "commit"
        require(transport.run_ssh(target, password, commit, timeout=90).returncode == 0, "commit failed")
        staged = armed = False
        phase = "independent-final"
        final = trust.build_preflight_script(accepted, baseline, update_version="0.1.0~dev2")
        final += "\nguards_absent\n" + no_state() + package.installed_checks(new)
        result = transport.run_ssh(target, password, final, timeout=90)
        require(result.returncode == 0 and trust.parse_facts(result.stdout, trust.PRE_KEYS) == before, "final state mismatch")
        receipt.update({"result": "passed", "runtimeUnchanged": True, "trustPreserved": True,
                        "stateInitialized": False, "unitsEnabled": False, "targetPowerControl": False})
        return 0
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        receipt.update({"result": "failed", "stage": phase, "errorType": type(error).__name__})
        if staged:
            try:
                recovery = f"sh {shlex.quote(paths.rollback_script)} explicit" if armed else assignments({"stage_root": paths.root, "marker": paths.marker, "marker_value": marker}) + "\nset -eu\n" + cleanup(paths)
                recovery_result = transport.run_ssh(target, password, recovery, timeout=180)
                receipt["recoveryExitCode"] = recovery_result.returncode
                staged = recovery_result.returncode != 0
            except (OSError, subprocess.SubprocessError):
                pass
        return 1
    finally:
        receipt["cleanupConfirmed"] = not staged
        write_json_0600_atomic(output, receipt)
        for index in range(len(password)):
            password[index] = 0


if __name__ == "__main__":
    raise SystemExit(main())
