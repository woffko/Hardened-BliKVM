#!/usr/bin/env python3
"""Run the gadget-lifecycle gate on a test H616 board.

The gate proves the two P0.1 properties that a reboot used to break:

- the board owner binds the pinned HID functions itself, so the gadget exists
  and is attached before any media or HID request (``--confirm-restart``);
- a media attach and eject change only the mass-storage function and leave the
  keyboard and mouse bound (``--image`` with ``--confirm-media-cycle``).

It is observation-only by default: without a ``--confirm-*`` flag it records
the current facts and refuses to change anything. The media cycle logs in
through the board API, so the operator password is read from the second stdin
line and is never written to the receipt. ``--confirm-hid-observed`` records
that the operator saw keyboard or mouse input arrive on the target; the tool
cannot prove that by itself.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import re
import secrets
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, TypedDict, cast


Json = dict[str, Any]

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import upload_media_image as upload
from tools.capture_pikvm_baseline import write_json_0600_atomic


UNIT = "hardened-blikvm-board-owner.service"
BROKER_BINARY = "/usr/libexec/hardened-blikvm/bli-kvm-control-broker"
GADGET_ROOT = "/sys/kernel/config/usb_gadget"
GADGET = "hardened"
HID_FUNCTIONS = ("hid.usb0", "hid.usb1", "hid.usb2")
OPERATION_ID = re.compile(r"^[0-9a-f]{32}$")
IMAGE_NAME = re.compile(r"^[A-Za-z0-9._-]{1,64}$")
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
FIRST_LINE_MAX = 512
DEFAULT_TIMEOUT_SECONDS = 120
MAX_TIMEOUT_SECONDS = 300
JSON_HEADERS = {"Content-Type": "application/json"}

GADGET_FACTS = (
    "gadget_present",
    "gadget_name",
    "udc_bound",
    "functions",
    "msd_file",
    "hidg_devices",
    "hid_write",
)
PREFLIGHT_KEYS = {
    "owner_active",
    "server_active",
    "broker_sha256",
    "mass_storage_present",
    *GADGET_FACTS,
}


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
    imageName: str
    startupBindPassed: bool
    hidFunctionsBound: bool
    massStorageAbsentAtStartup: bool
    attachApplied: bool
    attachKeptHid: bool
    ejectApplied: bool
    ejectKeptHid: bool
    hidObservedOnTarget: bool
    brokerSha256: str
    functions: str
    hidgDevices: str
    failureStage: str
    failureType: str
    physicalActionsInvoked: bool
    mutationsPerformed: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str
    base_url: str
    api_username: str
    ca_file: str | None
    tls_server_name: str | None
    image: str
    confirm_restart: bool
    confirm_media_cycle: bool
    confirm_hid_observed: bool
    timeout_seconds: int


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint")
    _ = parser.add_argument("--username")
    _ = parser.add_argument("--known-hosts")
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--base-url")
    _ = parser.add_argument("--api-username")
    _ = parser.add_argument("--ca-file")
    _ = parser.add_argument("--tls-server-name")
    _ = parser.add_argument(
        "--image",
        default="",
        help="catalogued image name used by the attach/eject half",
    )
    _ = parser.add_argument(
        "--confirm-restart",
        action="store_true",
        help="restart the board owner and prove the startup bind",
    )
    _ = parser.add_argument(
        "--confirm-media-cycle",
        action="store_true",
        help="attach and eject the image through the board API",
    )
    _ = parser.add_argument(
        "--confirm-hid-observed",
        action="store_true",
        help="the operator saw keyboard or mouse input on the target",
    )
    _ = parser.add_argument("--timeout-seconds", type=int, default=DEFAULT_TIMEOUT_SECONDS)
    namespace = parser.parse_args()
    timeout_seconds = cast(int, namespace.timeout_seconds)
    if not 1 <= timeout_seconds <= MAX_TIMEOUT_SECONDS:
        raise SystemExit("--timeout-seconds is out of range")
    image = cast(str, namespace.image)
    if namespace.confirm_media_cycle and not IMAGE_NAME.fullmatch(image):
        raise SystemExit("--confirm-media-cycle needs a valid --image name")
    if namespace.confirm_media_cycle and (
        not namespace.base_url or not namespace.api_username
    ):
        raise SystemExit("--confirm-media-cycle needs --base-url and --api-username")
    return Options(
        endpoint=cast(str, namespace.endpoint or ""),
        username=cast(str, namespace.username or ""),
        known_hosts=cast(str, namespace.known_hosts or ""),
        output=cast(str, namespace.output),
        base_url=cast(str, namespace.base_url or ""),
        api_username=cast(str, namespace.api_username or ""),
        ca_file=cast("str | None", namespace.ca_file),
        tls_server_name=cast("str | None", namespace.tls_server_name),
        image=image,
        confirm_restart=cast(bool, namespace.confirm_restart),
        confirm_media_cycle=cast(bool, namespace.confirm_media_cycle),
        confirm_hid_observed=cast(bool, namespace.confirm_hid_observed),
        timeout_seconds=timeout_seconds,
    )


def read_credentials(*, dispatch: bool) -> tuple[bytearray, str]:
    """Read the reviewed stdin payload: the SSH secret and, for a media cycle,
    the operator password on the second line."""
    stream = getattr(sys.stdin, "buffer", sys.stdin)

    def read_line(limit: int) -> bytes:
        raw = stream.readline()
        if isinstance(raw, str):
            raw = raw.encode("utf-8", "replace")
        if not raw or len(raw) > limit:
            return b""
        return raw.rstrip(b"\r\n")

    secret = bytearray(read_line(FIRST_LINE_MAX))
    if not secret:
        raise SystemExit("the SSH secret is missing on stdin")
    password = ""
    if dispatch:
        password = read_line(FIRST_LINE_MAX).decode("utf-8", "replace")
        if len(password) < 8:
            raise SystemExit("the operator password is missing on stdin")
    return secret, password


def _gadget_fact_script() -> str:
    return f"""gdir={GADGET_ROOT}
if [ -d "$gdir/{GADGET}" ]; then printf 'gadget_present=yes\\n'; printf 'gadget_name={GADGET}\\n'; else printf 'gadget_present=no\\n'; printf 'gadget_name=none\\n'; fi
if [ -n "$(cat "$gdir/{GADGET}/UDC" 2>/dev/null | tr -d '\\n')" ]; then printf 'udc_bound=yes\\n'; else printf 'udc_bound=no\\n'; fi
functions=""
for path in "$gdir"/{GADGET}/functions/*; do
  [ -d "$path" ] || continue
  if [ -z "$functions" ]; then functions=$(basename "$path"); else functions="$functions,$(basename "$path")"; fi
done
if [ -n "$functions" ]; then printf 'functions=%s\\n' "$functions"; else printf 'functions=none\\n'; fi
file=$(cat "$gdir/{GADGET}/functions/mass_storage.usb0/lun.0/file" 2>/dev/null | tr -d '\\n' || true)
if [ -n "$file" ]; then printf 'msd_file=%s\\n' "$(basename "$file")"; else printf 'msd_file=none\\n'; fi
if [ -d "$gdir/{GADGET}/functions/mass_storage.usb0" ]; then printf 'mass_storage_present=yes\\n'; else printf 'mass_storage_present=no\\n'; fi
hidg=""
for device in /dev/hidg*; do
  [ -c "$device" ] || continue
  if [ -z "$hidg" ]; then hidg="$device"; else hidg="$hidg,$device"; fi
done
printf 'hidg_devices=%s\\n' "${{hidg:-none}}"
if [ -c /dev/hidg0 ]; then
  if printf '' > /dev/hidg0 2>/dev/null; then printf 'hid_write=ok\\n'; else printf 'hid_write=failed\\n'; fi
else
  printf 'hid_write=skipped\\n'
fi
"""


def preflight_script() -> str:
    return f"""set +e
printf 'owner_active=%s\\n' "$(systemctl is-active {UNIT} 2>/dev/null || true)"
printf 'server_active=%s\\n' "$(systemctl is-active hardened-blikvm-board-server.service 2>/dev/null || true)"
if [ -x {BROKER_BINARY} ]; then printf 'broker_sha256=%s\\n' "$(sha256sum {BROKER_BINARY} | cut -d ' ' -f 1)"; else printf 'broker_sha256=absent\\n'; fi
""" + _gadget_fact_script()


def restart_script(timeout_seconds: int) -> str:
    return f"""set -eu
systemctl restart {UNIT}
waited=0
while [ "$waited" -lt {timeout_seconds} ]; do
  if [ -S /run/hardened-blikvm-gadget/control.sock ] && [ -d {GADGET_ROOT}/{GADGET} ]; then break; fi
  waited=$((waited + 2)); sleep 2
done
printf 'owner_active=%s\\n' "$(systemctl is-active {UNIT})"
printf 'socket_present=%s\\n' "$([ -S /run/hardened-blikvm-gadget/control.sock ] && echo yes || echo no)"
printf 'server_active=%s\\n' "$(systemctl is-active hardened-blikvm-board-server.service 2>/dev/null || true)"
""" + _gadget_fact_script()


def facts_script() -> str:
    return "set +e\n" + _gadget_fact_script()


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" not in line or line.startswith(" "):
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]+", key):
            facts[key] = value
    return facts


def validate_readback(facts: dict[str, str], *, stage: str, expect_storage: bool) -> None:
    if facts.get("gadget_present") != "yes" or facts.get("gadget_name") != GADGET:
        raise GateError(stage, "gadget_absent")
    if facts.get("udc_bound") != "yes":
        raise GateError(stage, "gadget_unbound")
    functions = facts.get("functions", "none")
    for function in HID_FUNCTIONS:
        if function not in functions.split(","):
            raise GateError(stage, "hid_function_missing")
    if facts.get("hidg_devices", "none") == "none":
        raise GateError(stage, "hidg_device_missing")
    if facts.get("hid_write") not in ("ok", "skipped"):
        raise GateError(stage, "hid_write_failed")
    if expect_storage:
        if facts.get("mass_storage_present") != "yes":
            raise GateError(stage, "mass_storage_missing")
        if facts.get("msd_file", "none") == "none":
            raise GateError(stage, "msd_file_missing")
    else:
        if facts.get("mass_storage_present") != "no":
            raise GateError(stage, "mass_storage_unexpected")
        if facts.get("msd_file", "none") != "none":
            raise GateError(stage, "msd_file_unexpected")


def require_ssh(options: Options, secret: bytearray, script: str, timeout: int) -> dict[str, str]:
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    if not options.username or not options.known_hosts:
        raise SystemExit("--endpoint, --username and --known-hosts are required")
    target = prototype.RemoteTarget(
        host, ssh_port, options.username, Path(options.known_hosts)
    )
    result = prototype.run_ssh(target, secret, script, timeout=timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    return parse_facts(stdout)


class BoardApi:
    csrf: str | None

    def __init__(self, options: Options, password: str) -> None:
        self.csrf = None
        self.base = options.base_url.rstrip("/")
        parsed = urllib.parse.urlparse(self.base)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise GateError("api", "base_url_invalid")
        if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
            raise GateError("api", "insecure_base_url")
        self.opener = upload.build_opener(options.ca_file, options.tls_server_name)
        login = self._request(
            "POST",
            "/api/auth/login",
            {"username": options.api_username, "password": password},
            csrf=None,
        )
        if login.get("code") != 0:
            raise GateError("api", "login_failed")
        data = cast(Json, login.get("data") or {})
        csrf = data.get("csrfToken")
        if not isinstance(csrf, str) or not csrf:
            raise GateError("api", "csrf_missing")
        self.csrf = csrf

    def _request(
        self, method: str, route: str, payload: Json | None, csrf: str | None = None
    ) -> Json:
        token = self.csrf if csrf is None else csrf
        headers = JSON_HEADERS | {"Origin": self.base}
        if token:
            headers["x-csrf-token"] = token
        request = urllib.request.Request(
            urllib.parse.urljoin(self.base, route),
            data=None if payload is None else json.dumps(payload).encode(),
            headers=headers,
            method=method,
        )
        try:
            with self.opener.open(request, timeout=30) as response:
                return cast(Json, json.loads(response.read().decode() or "{}"))
        except urllib.error.HTTPError as error:
            return cast(Json, json.loads(error.read().decode() or "{}"))

    def catalog(self) -> list[Json]:
        body = self._request("GET", "/api/virtual-media/catalog", None)
        if body.get("code") != 0:
            raise GateError("api", "catalog_unavailable")
        data = cast(Json, body.get("data") or {})
        images = data.get("images")
        if not isinstance(images, list):
            return []
        entries: list[Json] = []
        for entry in cast(list[object], images):
            if isinstance(entry, dict):
                entries.append(cast(Json, entry))
        return entries

    def command(self, command: Json) -> tuple[bool, str]:
        """Run one reviewed media command and report applied plus rejection."""
        body = self._request(
            "POST",
            "/api/virtual-media",
            {"operationId": secrets.token_hex(16), "command": command},
        )
        if body.get("code") != 0:
            raise GateError("api", f"command_rejected_{body.get('code')}")
        data = cast(Json, body.get("data") or {})
        rejection = data.get("rejection")
        return bool(data.get("applied")), rejection if isinstance(rejection, str) else ""


def command_with_retry(
    api: BoardApi, command: Json, timeout_seconds: int, stage: str
) -> None:
    """Apply a media command, waiting out the reviewed mutation cooldown.

    The media policy keeps a short global cooldown after every applied
    mutation, so an attach that follows a select can be answered with
    ``cooldown_active``; the gate waits for it instead of reporting a false
    failure, and fails closed on anything else.
    """
    deadline = time.monotonic() + min(timeout_seconds, 60)
    while True:
        applied, rejection = api.command(command)
        if applied:
            return
        if rejection != "cooldown_active" or time.monotonic() >= deadline:
            raise GateError(stage, f"{command.get('action', 'command')}_{rejection or 'not_applied'}")
        time.sleep(1.0)


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    dispatch = options.confirm_media_cycle
    secret, password = read_credentials(dispatch=dispatch)
    mutations = options.confirm_restart or options.confirm_media_cycle

    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-gadget-lifecycle-gate",
        "result": "observed",
        "mode": "plan",
        "imageName": options.image,
        "startupBindPassed": False,
        "hidFunctionsBound": False,
        "massStorageAbsentAtStartup": False,
        "attachApplied": False,
        "attachKeptHid": False,
        "ejectApplied": False,
        "ejectKeptHid": False,
        "hidObservedOnTarget": options.confirm_hid_observed,
        "brokerSha256": "",
        "functions": "",
        "hidgDevices": "",
        "failureStage": "",
        "failureType": "",
        "physicalActionsInvoked": False,
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    stage = "preflight"
    try:
        preflight = require_ssh(
            options, secret, preflight_script(), min(options.timeout_seconds, 60)
        )
        broker = preflight.get("broker_sha256", "")
        if not HEX_SHA256.fullmatch(broker):
            raise GateError("preflight", "broker_binary_missing")
        receipt["brokerSha256"] = broker

        if options.confirm_restart:
            stage = "startup-bind"
            after = require_ssh(
                options, secret, restart_script(options.timeout_seconds), options.timeout_seconds
            )
            if after.get("owner_active") != "active":
                raise GateError(stage, "owner_inactive")
            if after.get("socket_present") != "yes":
                raise GateError(stage, "control_socket_missing")
            validate_readback(after, stage=stage, expect_storage=False)
            receipt["startupBindPassed"] = True
            receipt["hidFunctionsBound"] = True
            receipt["massStorageAbsentAtStartup"] = True
            receipt["functions"] = after.get("functions", "")
            receipt["hidgDevices"] = after.get("hidg_devices", "")
            receipt["mode"] = "restart"

        if options.confirm_media_cycle:
            stage = "attach"
            api = BoardApi(options, password)
            password = ""
            images = api.catalog()
            match = [entry for entry in images if entry.get("imageName") == options.image]
            if not match:
                raise GateError(stage, "image_not_catalogued")
            mode = "removable_disk" if match[0].get("removableAllowed") else "cdrom"
            command_with_retry(
                api,
                {
                    "action": "select",
                    "imageId": match[0].get("imageId"),
                    "mode": mode,
                    "writable": False,
                },
                options.timeout_seconds,
                stage,
            )
            command_with_retry(
                api,
                {"action": "set_connected", "connected": True},
                options.timeout_seconds,
                stage,
            )
            receipt["attachApplied"] = True
            attached = require_ssh(
                options, secret, facts_script(), min(options.timeout_seconds, 60)
            )
            validate_readback(attached, stage="attach", expect_storage=True)
            if attached.get("msd_file") != options.image:
                raise GateError("attach", "image_mismatch")
            receipt["attachKeptHid"] = True
            receipt["functions"] = attached.get("functions", "")
            receipt["hidgDevices"] = attached.get("hidg_devices", "")

            stage = "eject"
            command_with_retry(
                api,
                {"action": "set_connected", "connected": False},
                options.timeout_seconds,
                stage,
            )
            receipt["ejectApplied"] = True
            ejected = require_ssh(
                options, secret, facts_script(), min(options.timeout_seconds, 60)
            )
            validate_readback(ejected, stage="eject", expect_storage=False)
            receipt["ejectKeptHid"] = True
            receipt["functions"] = ejected.get("functions", "")
            receipt["hidgDevices"] = ejected.get("hidg_devices", "")
            receipt["mode"] = "restart+media" if options.confirm_restart else "media"
    except GateError as error:
        receipt["result"] = "failed"
        receipt["failureStage"] = error.stage
        receipt["failureType"] = error.failure_type
        receipt["mutationsPerformed"] = mutations
        receipt["persistentChangeScope"] = (
            "owner restart and one attach/eject cycle" if mutations else "none"
        )
        write_json_0600_atomic(output, receipt)
        return 1

    if options.confirm_media_cycle or options.confirm_restart:
        receipt["result"] = "passed"
    receipt["mutationsPerformed"] = mutations
    receipt["persistentChangeScope"] = (
        "owner restart and one attach/eject cycle" if mutations else "none"
    )
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
