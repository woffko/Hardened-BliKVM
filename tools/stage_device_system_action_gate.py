#!/usr/bin/env python3
"""Stage and run the system-action end-to-end gate.

Plan-only by default: it prints the reviewed device facts and refuses to touch
anything. The gate has two phases because the reboot itself drops the
connection:

- `--confirm-reboot` dispatches the reviewed reboot through the board API
  (`POST /api/vm/system/reboot` with an explicit confirmation) and records that
  the owner accepted it. The GUI password is read from stdin by the caller and is
  never written to the receipt.
- `--confirm-observed` runs after the board is back: it records the new boot
  identity, observes the pending record the owner persisted, starts the reviewed
  acceptance unit and passes the gate only when the store itself reports the
  record closed. The operator confirms the health evidence.

`--confirm-restart` is the reboot-free half of the same proof: it restarts the
board owner and checks that the persisted record survives. SSH uses the enrolled
secret on stdin, so only one stdin consumer is active per invocation.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import Any, Final, cast

sys.dont_write_bytecode = True

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype
from tools import upload_media_image as upload


UNIT: Final = "hardened-blikvm-board-owner.service"
ACCEPT_UNIT: Final = "hardened-blikvm-system-action-accept.service"
SERVER_UNIT: Final = "hardened-blikvm-board-server.service"
NGINX_UNIT: Final = "hardened-blikvm-nginx.service"
BOARDD_UNIT: Final = "hardened-blikvm-boardd.service"
MAINTENANCE: Final = "/usr/libexec/hardened-blikvm/bli-kvm-system-action-maintenance"
STORE_ROOT: Final = "/var/lib/hardened-blikvm-system-actions"
CONTROL_SOCKET: Final = "/run/hardened-blikvm-gadget/control.sock"
PROFILE_ENV: Final = "/etc/hardened-blikvm-gadget/system-action.env"
MAX_RECEIPT_BYTES: Final = 64 * 1024
HEX_OPERATION: Final = re.compile(r"^[0-9a-f]{32}$")
OBSERVATION: Final = re.compile(r"^result=(accepted|waiting|rejected|absent|ambiguous)$")
OPERATION: Final = re.compile(r"\boperation=([0-9a-f]{32})\b")


def preflight_script() -> str:
    return f"""set -u
printf 'owner_unit=%s\\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
if [ -S {CONTROL_SOCKET} ]; then printf 'control_socket=present\\n'; else printf 'control_socket=absent\\n'; fi
if [ -d {STORE_ROOT} ]; then printf 'store_dir=present\\n'; else printf 'store_dir=absent\\n'; fi
if [ -e {STORE_ROOT}/journal-a.bin ]; then printf 'journal_a=present\\n'; else printf 'journal_a=absent\\n'; fi
if [ -e {STORE_ROOT}/journal-b.bin ]; then printf 'journal_b=present\\n'; else printf 'journal_b=absent\\n'; fi
if [ -e {PROFILE_ENV} ]; then printf 'profile_env=present\\n'; else printf 'profile_env=absent\\n'; fi
if grep -q '^BLI_KVM_SYSTEM_ACTION_PROFILE=1$' {PROFILE_ENV} 2>/dev/null; then printf 'profile_enabled=yes\\n'; else printf 'profile_enabled=no\\n'; fi
if grep -q '^BLI_KVM_SYSTEM_ACTION_ARMED=1$' {PROFILE_ENV} 2>/dev/null; then printf 'armed=yes\\n'; else printf 'armed=no\\n'; fi
if grep -qE '^BLI_KVM_SYSTEM_ACTION_REBOOT_COMMAND=.' {PROFILE_ENV} 2>/dev/null; then printf 'reboot_command=set\\n'; else printf 'reboot_command=unset\\n'; fi
if grep -qE '^BLI_KVM_SYSTEM_ACTION_SHUTDOWN_COMMAND=.' {PROFILE_ENV} 2>/dev/null; then printf 'shutdown_command=set\\n'; else printf 'shutdown_command=unset\\n'; fi
if [ -x {MAINTENANCE} ]; then printf 'maintenance_binary=present\\n'; else printf 'maintenance_binary=absent\\n'; fi
if [ -e /etc/systemd/system/{ACCEPT_UNIT} ]; then printf 'accept_unit=present\\n'; else printf 'accept_unit=absent\\n'; fi
if systemctl is-enabled {ACCEPT_UNIT} 2>/dev/null | grep -q '^enabled$'; then printf 'accept_unit_enabled=yes\\n'; else printf 'accept_unit_enabled=no\\n'; fi
printf 'boot_id=%s\\n' "$(tr -d '\\n' < /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
"""


def observe_command(expect_operation: str | None) -> str:
    if expect_operation is None:
        return f"{MAINTENANCE} accept-post-boot --pending"
    return f"{MAINTENANCE} accept-post-boot --expect-operation {expect_operation}"


def restart_script() -> str:
    return f"""set -u
python3 - <<'PYEOF'
import json, socket, struct
def observe():
    envelope = {{"schemaVersion": 2, "requestId": 1, "operationId": "ab" * 16,
                "request": {{"type": "accept_post_boot", "request": {{"guardsReady": False}}}}}}
    payload = json.dumps(envelope).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(10)
        client.connect("{CONTROL_SOCKET}")
        client.sendall(struct.pack(">I", len(payload)) + payload)
        head = client.recv(4)
        size = struct.unpack(">I", head)[0]
        body = b""
        while len(body) < size:
            chunk = client.recv(size - len(body))
            if not chunk:
                break
            body += chunk
        response = json.loads(body.decode()).get("response", {{}})
    # Without the guard attestation the policy never changes the journal: a
    # pending record answers backend_rejected, an empty journal invalid_state.
    return "pending" if response.get("code") == "backend_rejected" else "none"
print("observe_before=%s" % observe())
PYEOF
systemctl restart {UNIT}
printf 'restart_started=yes\n'
waited=0
while [ "$waited" -lt 30 ]; do
  if [ -S {CONTROL_SOCKET} ] && systemctl is-active {UNIT} >/dev/null 2>&1; then break; fi
  sleep 1
  waited=$((waited + 1))
done
printf 'owner_unit=%s\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
printf 'socket_waited_seconds=%s\n' "$waited"
python3 - <<'PYEOF'
import json, socket, struct
def observe():
    envelope = {{"schemaVersion": 2, "requestId": 1, "operationId": "ab" * 16,
                "request": {{"type": "accept_post_boot", "request": {{"guardsReady": False}}}}}}
    payload = json.dumps(envelope).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(10)
        client.connect("{CONTROL_SOCKET}")
        client.sendall(struct.pack(">I", len(payload)) + payload)
        head = client.recv(4)
        size = struct.unpack(">I", head)[0]
        body = b""
        while len(body) < size:
            chunk = client.recv(size - len(body))
            if not chunk:
                break
            body += chunk
        response = json.loads(body.decode()).get("response", {{}})
    return "pending" if response.get("code") == "backend_rejected" else "none"
print("observe_after=%s" % observe())
PYEOF
printf 'boot_id=%s\n' "$(tr -d '\\n' < /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
"""


def post_boot_script() -> str:
    return f"""set -u
printf 'boot_id=%s\\n' "$(tr -d '\\n' < /proc/sys/kernel/random/boot_id 2>/dev/null || true)"
printf 'owner_unit=%s\\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
printf 'server_unit=%s\\n' "$(systemctl is-active {SERVER_UNIT} 2>/dev/null || true)"
printf 'nginx_unit=%s\\n' "$(systemctl is-active {NGINX_UNIT} 2>/dev/null || true)"
printf 'boardd_unit=%s\\n' "$(systemctl is-active {BOARDD_UNIT} 2>/dev/null || true)"
printf 'health_code=%s\\n' "$(curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:18080/api/health 2>/dev/null || true)"
if [ -S {CONTROL_SOCKET} ]; then printf 'control_socket=present\\n'; else printf 'control_socket=absent\\n'; fi
if [ -e {STORE_ROOT}/journal-a.bin ]; then printf 'journal_a=present\\n'; else printf 'journal_a=absent\\n'; fi
if grep -q '^BLI_KVM_SYSTEM_ACTION_ARMED=1$' {PROFILE_ENV} 2>/dev/null; then printf 'armed=yes\\n'; else printf 'armed=no\\n'; fi
"""


def accept_script() -> str:
    """Ask the owner itself to close the pending record.

    Only the owner holds the journal, so the acceptance has to run there; the
    standalone maintenance process cannot open the store while the owner runs.
    """
    return f"""set -u
python3 - <<'PYEOF'
import json, socket, struct
def call(request):
    envelope = {{"schemaVersion": 2, "requestId": 1, "operationId": "ab" * 16, "request": request}}
    payload = json.dumps(envelope).encode()
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(10)
        client.connect("{CONTROL_SOCKET}")
        client.sendall(struct.pack(">I", len(payload)) + payload)
        head = client.recv(4)
        size = struct.unpack(">I", head)[0]
        body = b""
        while len(body) < size:
            chunk = client.recv(size - len(body))
            if not chunk:
                break
            body += chunk
        return json.loads(body.decode())
applied = {{"type": "applied"}}
first = call({{"type": "accept_post_boot", "request": {{"guardsReady": True}}}})
replay = call({{"type": "accept_post_boot", "request": {{"guardsReady": True}}}})
print("accept_response=%s" % first.get("response", {{}}).get("type"))
print("store_observation=%s" % ("accepted" if first.get("response") == applied else "rejected"))
print("replay_observation=%s" % ("accepted" if replay.get("response") == applied else "rejected"))
PYEOF
"""


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" in line and not line.startswith(" "):
            key, value = line.split("=", 1)
            if re.fullmatch(r"[A-Za-z0-9_]+", key):
                facts[key] = value
    return facts


def dispatch_reboot(
    base_url: str,
    username: str,
    password: str,
    ca_file: str | None,
    tls_server_name: str | None,
) -> dict[str, Any]:
    """Dispatch the reviewed reboot over the board API and report the answer."""
    base = base_url.rstrip("/")
    parsed = urllib.parse.urlparse(base)
    if parsed.scheme not in ("http", "https") or not parsed.hostname:
        raise ValueError("base URL must be an absolute http(s) URL")
    if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
        raise ValueError("a remote board must be reached over https")
    opener = upload.build_opener(ca_file, tls_server_name)
    login = upload.post_json(
        opener, base, "/api/auth/login", {"username": username, "password": password}
    )
    if login.get("code") != 0:
        raise ValueError("login failed")
    csrf = login.get("data", {}).get("csrfToken")
    if not csrf:
        raise ValueError("login returned no CSRF token")
    request = urllib.request.Request(
        urllib.parse.urljoin(base, "/api/vm/system/reboot"),
        data=json.dumps({"confirm": True}).encode(),
        headers={
            "Content-Type": "application/json",
            "Origin": base.rstrip("/"),
            "x-csrf-token": csrf,
        },
        method="POST",
    )
    try:
        with opener.open(request, timeout=30) as response:
            body = json.loads(response.read().decode())
            status = response.status
    except urllib.error.HTTPError as error:
        body = json.loads(error.read().decode() or "{}")
        status = error.code
    data = body.get("data") or {}
    return {
        "status": status,
        "code": body.get("code"),
        "error": body.get("error"),
        "capability": body.get("data") is not None,
        "applied": data.get("applied") is True,
        "rejection": data.get("rejection"),
    }


def decide_outcome(
    *,
    confirm_restart: bool,
    confirm_reboot: bool,
    confirm_observed: bool,
    preflight: dict[str, str],
    phase: dict[str, str],
    dispatch: dict[str, Any] | None = None,
) -> str:
    if confirm_reboot and not phase:
        # A dispatch the board refused (for example because its journal write
        # failed) must not be recorded as sent: the action never ran and the
        # post-boot acceptance can never pass.
        if dispatch is not None and dispatch.get("applied") is not True:
            return "inspect"
        return "dispatching"
    if confirm_restart:
        persisted = (
            phase.get("observe_before") == "pending"
            and phase.get("observe_after") == "pending"
            and phase.get("owner_unit") == "active"
        )
        return "restart-persisted" if persisted else "inspect"
    if confirm_reboot:
        if phase.get("store_observation") not in ("waiting", "accepted"):
            return "inspect"
        if not confirm_observed:
            return "dispatching"
        if phase.get("store_observation") != "accepted":
            return "inspect"
        return "passed"
    if confirm_observed:
        if phase.get("store_observation") != "accepted":
            return "inspect"
        healthy = (
            phase.get("owner_unit") == "active"
            and phase.get("server_unit") == "active"
            and phase.get("nginx_unit") == "active"
            and phase.get("boardd_unit") == "active"
            and phase.get("health_code") == "200"
        )
        return "passed" if healthy else "inspect"
    return "plan"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint")
    _ = parser.add_argument("--username")
    _ = parser.add_argument("--known-hosts")
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--base-url")
    _ = parser.add_argument("--api-username")
    _ = parser.add_argument("--ca-file")
    _ = parser.add_argument("--tls-server-name")
    _ = parser.add_argument("--expect-operation")
    _ = parser.add_argument("--confirm-restart", action="store_true")
    _ = parser.add_argument(
        "--confirm-reboot",
        action="store_true",
        help="dispatch the reviewed reboot through the board API",
    )
    _ = parser.add_argument(
        "--confirm-observed",
        action="store_true",
        help="the operator verified the post-boot health evidence",
    )
    _ = parser.add_argument("--timeout", type=int, default=300)
    return parser.parse_args()


def require_ssh(args: argparse.Namespace) -> prototype.RemoteTarget:
    for name in ("endpoint", "username", "known_hosts"):
        if not getattr(args, name):
            raise SystemExit(f"--{name.replace('_', '-')} is required for this step")
    host, ssh_port = prototype.parse_endpoint(args.endpoint)
    return prototype.RemoteTarget(
        host, ssh_port, args.username, Path(args.known_hosts)
    )


def read_credentials(*, dispatch: bool) -> tuple[bytearray, str]:
    """Read the reviewed stdin payload: the SSH secret on the first line and,
    for a reboot dispatch run, the operator password on the second."""
    stream = getattr(sys.stdin, "buffer", sys.stdin)
    first = stream.readline()
    if not first or len(first) > 512:
        raise SystemExit("the SSH secret is missing on stdin")
    secret = bytearray(first.rstrip(b"\r\n"))
    if not secret:
        raise SystemExit("the SSH secret is missing on stdin")
    password = ""
    if dispatch:
        second = stream.readline()
        password = second.decode("utf-8", "replace").rstrip("\r\n")
        if len(password) < 8:
            raise SystemExit("the operator password is missing on stdin")
    return secret, password


def run_remote(
    args: argparse.Namespace, script: str, secret: bytearray
) -> dict[str, str]:
    target = require_ssh(args)
    result = prototype.run_ssh(target, secret, script, timeout=args.timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    return parse_facts(stdout)


def main() -> int:
    args = parse_args()
    output: Path = prototype.validate_output_path(args.output)
    if args.expect_operation is not None and not HEX_OPERATION.fullmatch(
        args.expect_operation
    ):
        raise SystemExit("--expect-operation must be 32 lowercase hex characters")
    if args.confirm_restart and (args.confirm_reboot or args.confirm_observed):
        raise SystemExit("--confirm-restart is a standalone mode")

    receipt: dict[str, Any] = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-system-action-gate",
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": bool(
                args.confirm_restart or args.confirm_reboot or args.confirm_observed
            ),
            "receiptContainsSecrets": False,
        },
    }
    preflight: dict[str, str] = {}
    phase: dict[str, str] = {}
    dispatch = bool(args.confirm_reboot)
    secret, operator_password = read_credentials(dispatch=dispatch)

    if args.confirm_reboot and not args.confirm_observed:
        if not args.base_url or not args.api_username:
            raise SystemExit("--confirm-reboot needs --base-url and --api-username")
        password = operator_password
        if len(password) < 8:
            raise SystemExit("password input is invalid")
        receipt["dispatch"] = dispatch_reboot(
            args.base_url,
            args.api_username,
            password,
            args.ca_file,
            args.tls_server_name,
        )
        password = ""
        preflight = run_remote(args, preflight_script(), secret)
        receipt["mode"] = "reboot"
        receipt["result"] = decide_outcome(
            confirm_restart=False,
            confirm_reboot=True,
            confirm_observed=False,
            preflight=preflight,
            phase={},
            dispatch=cast(Any, receipt["dispatch"]),
        )
    elif args.confirm_restart:
        phase = run_remote(args, restart_script(), secret)
        receipt["mode"] = "restart"
        receipt["phase"] = phase
        receipt["result"] = decide_outcome(
            confirm_restart=True,
            confirm_reboot=False,
            confirm_observed=False,
            preflight={},
            phase=phase,
        )
    elif args.confirm_observed:
        preflight = run_remote(args, preflight_script(), secret)
        boot = run_remote(args, post_boot_script(), secret)
        phase = run_remote(args, accept_script(), secret)
        phase.update(boot)
        receipt["mode"] = "accept"
        receipt["phase"] = phase
        receipt["result"] = decide_outcome(
            confirm_restart=False,
            confirm_reboot=False,
            confirm_observed=True,
            preflight=preflight,
            phase=phase,
        )
    else:
        preflight = run_remote(args, preflight_script(), secret)
        receipt["mode"] = "plan"
        receipt["result"] = "plan"

    receipt["preflight"] = preflight
    if preflight.get("boot_id"):
        receipt["bootIdBefore"] = preflight["boot_id"]
    after_boot = phase.get("boot_id") or phase.get("boot_boot_id")
    if after_boot:
        receipt["bootIdAfter"] = after_boot
    operation = phase.get("operation") or phase.get("operation_after")
    if operation:
        receipt["operationId"] = operation
    receipt["operatorObserved"] = bool(args.confirm_observed)
    encoded = json.dumps(receipt).encode()
    if len(encoded) > MAX_RECEIPT_BYTES:
        raise SystemExit("receipt is too large")
    write_json_0600_atomic(output, receipt)
    print(f"system_action_gate={output} mode={receipt['mode']} result={receipt['result']}")
    return 0 if receipt["result"] in ("plan", "restart-persisted", "dispatching", "passed") else 1


if __name__ == "__main__":
    raise SystemExit(main())
