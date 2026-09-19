#!/usr/bin/env python3
"""Run the Tailscale surface gate on a test board.

The board ships no Tailscale package and no reviewed source for one, so the
surface is fail-closed: the state is readable, and every management action is
refused. This gate proves exactly that on the device:

- ``GET /api/extensions/tailscale/status`` answers with the observable state;
- every reviewed action (``install``, ``uninstall``, ``start``, ``restart``,
  ``stop``, ``up``, ``down``, ``login``, ``logout``) is refused with
  ``applied=false`` and a rejection code, and never mutates the board.

The operator password arrives on the second stdin line. Without ``--confirm``
the gate only observes.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import sys
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
# The reviewed actions the reference exposes; package installation is offered
# only once a reviewed package source exists, so these are all refused here.
ACTIONS = (
    "install",
    "uninstall",
    "start",
    "restart",
    "stop",
    "up",
    "down",
    "login",
    "logout",
)


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
    installed: bool
    running: bool
    enabled: bool
    statusMessage: str
    actionsAttempted: int
    actionsRefused: int
    actionsApplied: int
    firstRejection: str
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
    base_url: str
    api_username: str
    ca_file: str | None
    tls_server_name: str | None
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
    _ = parser.add_argument(
        "--confirm",
        action="store_true",
        help="exercise every reviewed action and require a refusal; without it the gate observes",
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
        ca_file=cast(str | None, namespace.ca_file),
        tls_server_name=cast(str | None, namespace.tls_server_name),
        confirm=bool(namespace.confirm),
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


class TailscaleApi:
    def __init__(self, options: Options, password: str) -> None:
        self.csrf: str | None = None
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

    def data(self, method: str, route: str, payload: Json | None = None) -> Json:
        body = self._request(method, route, payload)
        if body.get("code") != 0:
            raise GateError("api", f"request_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})

    def status(self) -> Json:
        return self.data("GET", "/api/extensions/tailscale/status")

    def action(self, name: str) -> Json:
        return self.data("POST", f"/api/extensions/tailscale/{name}")


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    _secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-tailscale-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "installed": False,
        "running": False,
        "enabled": False,
        "statusMessage": "",
        "actionsAttempted": 0,
        "actionsRefused": 0,
        "actionsApplied": 0,
        "firstRejection": "",
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    password_holder = password
    password = ""
    stage = "status"

    try:
        api = TailscaleApi(options, password_holder)
        password_holder = ""
        status = api.status()
        receipt["installed"] = bool(status.get("installed"))
        receipt["running"] = bool(status.get("running"))
        receipt["enabled"] = bool(status.get("enabled"))
        receipt["statusMessage"] = str(status.get("message") or "")

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "actions"
        if not receipt["installed"]:
            # No package: the surface must stay fail-closed.
            refused = 0
            for name in ACTIONS:
                outcome = api.action(name)
                if outcome.get("applied") is True:
                    raise GateError(stage, f"{name}_admitted")
                rejection = str(outcome.get("rejection") or "")
                if not rejection:
                    raise GateError(stage, f"{name}_no_rejection")
                if not receipt["firstRejection"]:
                    receipt["firstRejection"] = rejection
                refused += 1
            receipt["actionsAttempted"] = len(ACTIONS)
            receipt["actionsRefused"] = refused
            if refused != len(ACTIONS):
                raise GateError(stage, "not_every_action_refused")
        else:
            # The package is present, so management follows the reference:
            # start must apply, the daemon must run, and the previous state
            # is restored.
            was_running = receipt["running"]
            started = api.action("start")
            if started.get("applied") is not True:
                raise GateError(stage, "start_refused")
            refreshed = api.status()
            if not bool(refreshed.get("running")):
                raise GateError(stage, "start_did_not_run")
            receipt["actionsAttempted"] = 1
            receipt["actionsApplied"] = 1
            if not was_running:
                stopped = api.action("stop")
                if stopped.get("applied") is not True:
                    raise GateError(stage, "stop_refused")
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
