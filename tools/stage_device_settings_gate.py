#!/usr/bin/env python3
"""Run the device-settings gate on a test H616 board.

The gate exercises the reviewed device and network toggles end to end and
leaves the board as it found it:

- mDNS is switched on, verified through the resolver's own link state, then
  switched back to the state it had;
- HID-only is switched on, a media attach must then be refused, and the switch
  goes back;
- the system-log test line is sent and must appear in the journal;
- the read-only views (IPv6 mode, WiFi state, TLS state) are recorded.

The operator password arrives on the second stdin line. Without `--confirm`
the gate only observes. The mouse jiggler's movement is only visible on the
target, so the gate records its state and leaves the observation to the
operator.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import upload_media_image as upload
from tools.capture_pikvm_baseline import write_json_0600_atomic


Json = dict[str, Any]

FIRST_LINE_MAX = 512
JSON_HEADERS = {"Content-Type": "application/json"}
RESOLVER_STATE = re.compile(r"Multicast DNS:\s*(\S+)")
FACTS = {"mdns_link", "journal_test_lines"}


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
    initialMdns: bool
    mdnsEnabled: bool
    mdnsRestored: bool
    hidOnlyEnabled: bool
    mediaRefusedWhileHidOnly: bool
    hidOnlyRestored: bool
    systemLogTestApplied: bool
    systemLogLineObserved: bool
    ipv6Mode: str
    wifiAvailable: bool
    wifiSsid: str
    tlsEnabled: bool
    tlsSubject: str
    jigglerState: str
    failureStage: str
    failureType: str
    mutationsPerformed: bool
    physicalActionsInvoked: bool
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
    confirm: bool
    timeout_seconds: int


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--base-url", required=True)
    _ = parser.add_argument("--api-username", required=True)
    _ = parser.add_argument("--ca-file")
    _ = parser.add_argument("--tls-server-name")
    _ = parser.add_argument("--image", default="kubuntu.img")
    _ = parser.add_argument(
        "--confirm",
        action="store_true",
        help="switch the toggles and restore them; without it the gate observes",
    )
    _ = parser.add_argument("--timeout-seconds", type=int, default=60)
    namespace = parser.parse_args()
    if not 1 <= cast(int, namespace.timeout_seconds) <= 300:
        raise SystemExit("--timeout-seconds is out of range")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        base_url=cast(str, namespace.base_url),
        api_username=cast(str, namespace.api_username),
        ca_file=cast("str | None", namespace.ca_file),
        tls_server_name=cast("str | None", namespace.tls_server_name),
        image=cast(str, namespace.image),
        confirm=cast(bool, namespace.confirm),
        timeout_seconds=cast(int, namespace.timeout_seconds),
    )


def read_credentials(*, dispatch: bool) -> tuple[bytearray, str]:
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


def facts_script() -> str:
    """Read the resolver's own mDNS state and the journal test line count."""
    return (
        "set +e\n"
        "printf 'mdns_link=%s\\n' \"$(resolvectl mdns 2>/dev/null | "
        "grep -m1 '^Global:' | sed 's/^Global:[[:space:]]*//' | "
        "tr -c 'A-Za-z0-9_.:-' '_')\"\n"
        "printf 'journal_test_lines=%s\\n' \"$(journalctl -t hardened-blikvm "
        "--no-pager 2>/dev/null | wc -l | tr -d ' ')\"\n"
    )


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" not in line or line.startswith(" "):
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]+", key):
            facts[key] = value
    return facts


def require_ssh(
    options: Options, secret: bytearray, script: str, timeout: int
) -> dict[str, str]:
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, options.username, Path(options.known_hosts)
    )
    result = prototype.run_ssh(target, secret, script, timeout=timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    facts = parse_facts(stdout)
    if not FACTS.issubset(facts):
        raise GateError("facts", "device_facts_incomplete")
    return facts


class BoardApi:
    csrf: str | None

    def __init__(self, options: Options, password: str) -> None:
        self.csrf = None
        self.base = options.base_url.rstrip("/")
        parsed = urllib.parse.urlparse(self.base)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise GateError("api", "base_url_invalid")
        self.opener = upload.build_opener(options.ca_file, options.tls_server_name)
        login = self._request(
            "POST",
            "/api/auth/login",
            {"username": options.api_username, "password": password},
            csrf=None,
        )
        if login.get("code") != 0:
            raise GateError("api", "login_failed")
        token = (cast(Json, login.get("data") or {})).get("csrfToken")
        if not isinstance(token, str) or not token:
            raise GateError("api", "csrf_missing")
        self.csrf = token

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

    def get(self, route: str) -> Json:
        body = self._request("GET", route, None)
        if body.get("code") != 0:
            raise GateError("api", f"read_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})

    def post(self, route: str, payload: Json | None = None) -> Json:
        body = self._request("POST", route, payload or {})
        if body.get("code") != 0:
            raise GateError("api", f"write_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})


def mdns_enabled(facts: dict[str, str]) -> bool:
    return facts.get("mdns_link", "").lower().startswith("yes")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-device-settings-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "initialMdns": False,
        "mdnsEnabled": False,
        "mdnsRestored": False,
        "hidOnlyEnabled": False,
        "mediaRefusedWhileHidOnly": False,
        "hidOnlyRestored": False,
        "systemLogTestApplied": False,
        "systemLogLineObserved": False,
        "ipv6Mode": "",
        "wifiAvailable": False,
        "wifiSsid": "",
        "tlsEnabled": False,
        "tlsSubject": "",
        "jigglerState": "",
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "physicalActionsInvoked": False,
        "persistentChangeScope": "none",
    }
    password_holder = password
    password = ""
    try:
        api = BoardApi(options, password_holder)
        password_holder = ""
        before = require_ssh(
            options, secret, facts_script(), min(options.timeout_seconds, 30)
        )
        initial_mdns = mdns_enabled(before)
        receipt["initialMdns"] = initial_mdns
        ipv6 = api.get("/api/network/ipv6")
        receipt["ipv6Mode"] = str(ipv6.get("mode", ""))
        wifi = api.get("/api/network/wifi")
        receipt["wifiAvailable"] = bool(wifi.get("available"))
        receipt["wifiSsid"] = str(wifi.get("ssid", ""))
        tls = api.get("/api/vm/tls")
        receipt["tlsEnabled"] = bool(tls.get("enabled"))
        receipt["tlsSubject"] = str(tls.get("subject", ""))
        hid_only = api.get("/api/vm/hid-only")
        initial_hid_only = bool(hid_only.get("enabled"))
        jiggler = api.get("/api/vm/mouse-jiggler")
        receipt["jigglerState"] = (
            "on" if jiggler.get("enabled") else "off"
        )

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        after = before
        target = not initial_mdns
        toggled = api.post("/api/vm/mdns/enable" if target else "/api/vm/mdns/disable")
        if toggled.get("applied") is not True:
            raise GateError("mdns", "mdns_toggle_not_applied")
        receipt["mutationsPerformed"] = True
        settle = 0
        while settle < 10:
            after = require_ssh(
                options, secret, facts_script(), min(options.timeout_seconds, 30)
            )
            if mdns_enabled(after) is target:
                break
            time.sleep(1.0)
            settle += 1
        if mdns_enabled(after) is not target:
            raise GateError("mdns", "mdns_state_not_applied")
        receipt["mdnsEnabled"] = True

        hid_toggle = api.post("/api/vm/hid-only", {"enabled": True})
        if hid_toggle.get("applied") is not True:
            raise GateError("hid-only", "hid_only_not_applied")
        catalog = api.get("/api/virtual-media/catalog")
        images = cast(list[Json], catalog.get("images") or [])
        if not images:
            raise GateError("hid-only", "no_catalogued_image")
        refusal = api.post(
            "/api/virtual-media",
            {
                "operationId": "0" * 32,
                "command": {
                    "action": "select",
                    "imageId": images[0].get("imageId"),
                    "mode": "removable_disk",
                    "writable": False,
                },
            },
        )
        if refusal.get("applied") is not False:
            raise GateError("hid-only", "media_attach_not_refused")
        receipt["hidOnlyEnabled"] = True
        receipt["mediaRefusedWhileHidOnly"] = True

        log_toggle = api.post("/api/system-log/test")
        if log_toggle.get("applied") is not True:
            raise GateError("system-log", "test_not_applied")
        receipt["systemLogTestApplied"] = True
        settle = 0
        while settle < 5:
            line_check = require_ssh(
                options, secret, facts_script(), min(options.timeout_seconds, 30)
            )
            if int(line_check.get("journal_test_lines", "0")) > int(
                before.get("journal_test_lines", "0")
            ):
                receipt["systemLogLineObserved"] = True
                break
            time.sleep(1.0)
            settle += 1
        if not receipt["systemLogLineObserved"]:
            raise GateError("system-log", "test_line_not_observed")

        restored = after
        api.post("/api/vm/hid-only", {"enabled": initial_hid_only})
        api.post("/api/vm/mdns/enable" if initial_mdns else "/api/vm/mdns/disable")
        settle = 0
        while settle < 10:
            restored = require_ssh(
                options, secret, facts_script(), min(options.timeout_seconds, 30)
            )
            if mdns_enabled(restored) is initial_mdns:
                break
            time.sleep(1.0)
            settle += 1
        if mdns_enabled(restored) is not initial_mdns:
            raise GateError("restore", "mdns_not_restored")
        receipt["mdnsRestored"] = True
        receipt["hidOnlyRestored"] = True
        receipt["persistentChangeScope"] = (
            "resolved configuration and unit restart, one system-log line, restored after the run"
        )
    except GateError as error:
        receipt["result"] = "failed"
        receipt["failureStage"] = error.stage
        receipt["failureType"] = error.failure_type
        write_json_0600_atomic(output, receipt)
        return 1

    receipt["result"] = "passed"
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
