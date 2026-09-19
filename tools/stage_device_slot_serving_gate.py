#!/usr/bin/env python3
"""Run the signed-slot serving cutover gate on a test board.

The retained update path stages signed slots under the live trust configuration's
slot root and records the confirmed slot in the application state. That state is
useless unless the running server executes the confirmed slot artifact instead
of the package-installed copy. This gate proves the cutover on the device:

- it reads the live supervisor configuration, the confirmed slot, the slot
  binary and web hashes and the package-installed hashes, and requires the slot
  artifact to be byte-identical to the installed one before any change, so the
  switch cannot silently change behaviour;
- with ``--confirm`` it writes a reviewed drop-in that resets the board server's
  ``ExecStart`` to ``<slotRoot>/current/bin/bli-kvm-server`` and points its web
  root at the slot, reloads systemd and restarts the server, then requires the
  running executable to be the slot artifact and HTTPS health to answer;
- with ``--rollback`` it removes the drop-in and requires the server to execute
  the installed artifact again.

The SSH password arrives on the first stdin line. Without ``--confirm`` or
``--rollback`` the gate only observes.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import re
import sys
from pathlib import Path
from typing import TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools.capture_pikvm_baseline import write_json_0600_atomic


FIRST_LINE_MAX = 512
REQUIRED_BEFORE = {
    "slotRoot",
    "slotExists",
    "slotBinarySha",
    "installedBinarySha",
    "slotWebSha",
    "installedWebSha",
    "slotExeReal",
    "installedExeReal",
    "serverActive",
    "serverEnabled",
    "serverPid",
    "serverExe",
    "dropInPresent",
    "confirmedSlot",
    "currentTarget",
    "healthOk",
}
REQUIRED_AFTER = REQUIRED_BEFORE


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    mode: str
    slotRoot: str
    slotBinarySha256: str
    installedBinarySha256: str
    slotWebSha256: str
    installedWebSha256: str
    slotExeReal: str
    installedExeReal: str
    confirmedSlot: str
    currentTarget: str
    serverExeBefore: str
    serverExeAfter: str
    serverPidAfter: str
    healthOkBefore: bool
    healthOkAfter: bool
    dropInPresentAfter: bool
    serverRestarted: bool
    failureStage: str
    failureType: str
    mutationsPerformed: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str
    confirm: bool
    rollback: bool
    timeout_seconds: int


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--confirm", action="store_true")
    _ = parser.add_argument("--rollback", action="store_true")
    _ = parser.add_argument("--timeout-seconds", type=int, default=90)
    namespace = parser.parse_args()
    if not 1 <= cast(int, namespace.timeout_seconds) <= 300:
        raise SystemExit("--timeout-seconds is out of range")
    if bool(namespace.confirm) and bool(namespace.rollback):
        raise SystemExit("--confirm and --rollback are mutually exclusive")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        confirm=bool(namespace.confirm),
        rollback=bool(namespace.rollback),
        timeout_seconds=cast(int, namespace.timeout_seconds),
    )


def read_secret() -> bytearray:
    stream = getattr(sys.stdin, "buffer", sys.stdin)
    raw = stream.readline()
    if isinstance(raw, str):
        raw = raw.encode("utf-8", "replace")
    if not raw or len(raw) > FIRST_LINE_MAX:
        raise SystemExit("the SSH secret is missing on stdin")
    return bytearray(raw.rstrip(b"\r\n"))


REMOTE_TEMPLATE = r"""
set -e
python3 -B - <<'PY'
import hashlib, json, os, subprocess, time

CONFIG = "/etc/hardened-blikvm/update-supervisor.json"
UNIT = "hardened-blikvm-board-server"
DROPIN = "/etc/systemd/system/hardened-blikvm-board-server.service.d/90-slot-serving.conf"
INSTALLED_BIN = "/usr/libexec/hardened-blikvm/bli-kvm-server"
INSTALLED_WEB = "/usr/share/hardened-blikvm/web/index.html"
ACTION = "__ACTION__"


def sha(path):
    try:
        digest = hashlib.sha256()
        with open(path, "rb") as handle:
            for chunk in iter(lambda: handle.read(65536), b""):
                digest.update(chunk)
        return digest.hexdigest()
    except OSError:
        return ""


def sh(*args):
    return subprocess.run(list(args), capture_output=True, text=True).stdout.strip()


def systemctl(*args):
    return subprocess.run(["systemctl", *args], capture_output=True, text=True)


with open(CONFIG, encoding="utf-8") as handle:
    config = json.load(handle)
slot_root = config["slotRoot"]
state_root = config["stateRoot"]
slot_current = os.path.join(slot_root, "current")


def emit(phase):
    slot_bin = os.path.join(slot_current, "bin", "bli-kvm-server")
    slot_web = os.path.join(slot_current, "share", "web", "index.html")
    pid = sh("systemctl", "show", "-p", "MainPID", "--value", UNIT)
    exe = os.path.realpath("/proc/%s/exe" % pid) if pid.isdigit() else ""
    try:
        with open(os.path.join(state_root, "application-state.json"), encoding="utf-8") as handle:
            state = json.load(handle)
    except OSError:
        state = {}
    confirmed = ((state.get("confirmed") or {}).get("slotId") or "")
    target = os.path.basename(os.readlink(slot_current)) if os.path.islink(slot_current) else ""
    health = subprocess.run(
        ["curl", "--noproxy", "*", "--insecure", "--silent", "--fail", "--max-time", "5",
         "https://127.0.0.1/api/health"],
        capture_output=True, text=True,
    )
    health_ok = health.returncode == 0 and bool((health.stdout or "").strip())
    for key, value in (
        ("slotRoot", slot_root),
        ("slotExists", str(os.path.isdir(slot_current))),
        ("slotBinarySha", sha(slot_bin)),
        ("installedBinarySha", sha(INSTALLED_BIN)),
        ("slotWebSha", sha(slot_web)),
        ("installedWebSha", sha(INSTALLED_WEB)),
        ("slotExeReal", os.path.realpath(slot_bin)),
        ("installedExeReal", os.path.realpath(INSTALLED_BIN)),
        ("serverActive", sh("systemctl", "is-active", UNIT)),
        ("serverEnabled", sh("systemctl", "is-enabled", UNIT)),
        ("serverPid", pid),
        ("serverExe", exe),
        ("dropInPresent", str(os.path.exists(DROPIN))),
        ("confirmedSlot", confirmed),
        ("currentTarget", target),
        ("healthOk", str(health_ok)),
    ):
        print("%s_%s=%s" % (phase, key, value))


emit("before")
if ACTION == "apply":
    os.makedirs(os.path.dirname(DROPIN), exist_ok=True)
    body = (
        "[Service]\n"
        "ExecStart=\n"
        "ExecStart=%s/current/bin/bli-kvm-server\n"
        "Environment=BLI_KVM_WEB_ROOT=%s/current/share/web\n"
    ) % (slot_root, slot_root)
    with open(DROPIN, "w", encoding="utf-8") as handle:
        handle.write(body)
elif ACTION == "rollback":
    try:
        os.unlink(DROPIN)
    except OSError:
        pass
if ACTION in ("apply", "rollback"):
    systemctl("daemon-reload")
    systemctl("restart", UNIT)
    time.sleep(4)
    emit("after")
PY
"""


def remote_script(action: str) -> str:
    return REMOTE_TEMPLATE.replace("__ACTION__", action)


def require_ssh(options: Options, secret: bytearray, script: str) -> dict[str, str]:
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, options.username, Path(options.known_hosts)
    )
    result = prototype.run_ssh(target, secret, script, timeout=options.timeout_seconds)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    if result.returncode != 0:
        raise GateError("ssh", "command_failed")
    return facts_from(stdout)


def facts_from(stdout: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in stdout.splitlines():
        if "=" not in line or line.startswith(" "):
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]+", key):
            facts[key] = value
    return facts


def phase_facts(facts: dict[str, str], phase: str) -> dict[str, str]:
    prefix = phase + "_"
    return {key.removeprefix(prefix): value for key, value in facts.items() if key.startswith(prefix)}


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    secret = read_secret()
    mode = "apply" if options.confirm else "rollback" if options.rollback else "plan"
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-slot-serving-gate",
        "result": "observed",
        "mode": mode,
        "slotRoot": "",
        "slotBinarySha256": "",
        "installedBinarySha256": "",
        "slotWebSha256": "",
        "installedWebSha256": "",
        "slotExeReal": "",
        "installedExeReal": "",
        "confirmedSlot": "",
        "currentTarget": "",
        "serverExeBefore": "",
        "serverExeAfter": "",
        "serverPidAfter": "",
        "healthOkBefore": False,
        "healthOkAfter": False,
        "dropInPresentAfter": False,
        "serverRestarted": False,
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    stage = "read"

    try:
        before = phase_facts(require_ssh(options, secret, remote_script("none")), "before")
        if not REQUIRED_BEFORE.issubset(before):
            raise GateError(stage, "status_unavailable")
        receipt["slotRoot"] = before["slotRoot"]
        receipt["slotBinarySha256"] = before["slotBinarySha"]
        receipt["installedBinarySha256"] = before["installedBinarySha"]
        receipt["slotWebSha256"] = before["slotWebSha"]
        receipt["installedWebSha256"] = before["installedWebSha"]
        receipt["slotExeReal"] = before["slotExeReal"]
        receipt["installedExeReal"] = before["installedExeReal"]
        receipt["confirmedSlot"] = before["confirmedSlot"]
        receipt["currentTarget"] = before["currentTarget"]
        receipt["serverExeBefore"] = before["serverExe"]
        receipt["healthOkBefore"] = before["healthOk"] == "True"
        if before["serverActive"] != "active" or before["serverEnabled"] != "enabled":
            raise GateError(stage, "serving_unit_not_active")
        if before["slotExists"] != "True":
            raise GateError(stage, "slot_missing")
        if not before["slotBinarySha"] or before["slotBinarySha"] != before["installedBinarySha"]:
            raise GateError(stage, "slot_binary_differs")
        if not before["slotWebSha"] or before["slotWebSha"] != before["installedWebSha"]:
            raise GateError(stage, "slot_web_differs")
        if not before["confirmedSlot"] or before["currentTarget"] != before["confirmedSlot"]:
            raise GateError(stage, "current_is_not_confirmed")

        if not options.confirm and not options.rollback:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "mutate"
        action = "apply" if options.confirm else "rollback"
        after = phase_facts(require_ssh(options, secret, remote_script(action)), "after")
        if not REQUIRED_AFTER.issubset(after):
            raise GateError(stage, "status_unavailable_after_mutation")
        receipt["serverExeAfter"] = after["serverExe"]
        receipt["serverPidAfter"] = after["serverPid"]
        receipt["healthOkAfter"] = after["healthOk"] == "True"
        receipt["dropInPresentAfter"] = after["dropInPresent"] == "True"
        receipt["serverRestarted"] = True
        receipt["mutationsPerformed"] = True

        if after["serverActive"] != "active":
            raise GateError(stage, "serving_unit_not_active_after")
        if not receipt["healthOkAfter"]:
            raise GateError(stage, "health_unavailable_after")
        if options.confirm:
            if after["serverExe"] != before["slotExeReal"]:
                raise GateError(stage, "server_not_executing_slot")
            if not receipt["dropInPresentAfter"]:
                raise GateError(stage, "drop_in_missing")
            if after["slotBinarySha"] != after["installedBinarySha"]:
                raise GateError(stage, "slot_binary_changed")
            receipt["result"] = "passed"
            receipt["persistentChangeScope"] = "one board server drop-in and restart"
        else:
            if after["dropInPresent"] == "True":
                raise GateError(stage, "drop_in_still_present")
            if after["serverExe"] != before["installedExeReal"]:
                raise GateError(stage, "server_not_executing_installed")
            receipt["result"] = "rolled_back"
            receipt["persistentChangeScope"] = "none"
    except GateError as error:
        receipt["result"] = "failed"
        receipt["failureStage"] = error.stage
        receipt["failureType"] = error.failure_type
        write_json_0600_atomic(output, receipt)
        return 1

    write_json_0600_atomic(output, receipt)
    return 0



def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2
    except Exception as error:  # noqa: BLE001 - the operator log is the surface
        import traceback

        traceback.print_exc()
        print(f"gate failed before a receipt: {error!r}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
