#!/usr/bin/env python3
"""Run the firewall baseline gate on a test board.

Once `nftables` is installed the owner can apply its reviewed baseline ruleset.
This gate proves the surface really works and is reversible:

- it reads the firewall state through the board API;
- with ``--confirm`` it applies the reviewed ``baseline`` mode, requires the
  confirmation to apply, and requires the state to report the baseline active;
- it restores the mode it found (``disabled`` unless the baseline was already
  active), so a run leaves the firewall as it found it.

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
    backendAvailable: bool
    nftInstalled: bool
    initialMode: str
    initialBaselineActive: bool
    appliedMode: str
    applied: bool
    confirmed: bool
    restoredMode: str
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
    _ = parser.add_argument("--confirm", action="store_true")
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
    stream = getattr(sys, "stdin", sys.stdin)
    stream = getattr(stream, "buffer", stream)

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


class FirewallApi:
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
        return self.data("GET", "/api/system/firewall")

    def set_mode(self, mode: str) -> Json:
        return self.data("POST", "/api/system/firewall", {"mode": mode})

    def confirm(self) -> Json:
        return self.data("POST", "/api/system/firewall/confirm", {})


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    _secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-firewall-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "backendAvailable": False,
        "nftInstalled": False,
        "initialMode": "",
        "initialBaselineActive": False,
        "appliedMode": "",
        "applied": False,
        "confirmed": False,
        "restoredMode": "",
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    password_holder = password
    password = ""
    stage = "read"
    api: FirewallApi | None = None

    try:
        api = FirewallApi(options, password_holder)
        password_holder = ""
        before = api.status()
        receipt["backendAvailable"] = bool(before.get("backendAvailable"))
        receipt["nftInstalled"] = bool(before.get("nftInstalled"))
        receipt["initialMode"] = str(before.get("mode") or "")
        receipt["initialBaselineActive"] = bool(before.get("baselineActive"))

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "apply"
        applied = api.set_mode("baseline")
        receipt["applied"] = applied.get("applied") is True
        if not receipt["applied"]:
            raise GateError(stage, str(applied.get("rejection") or "not_applied"))
        receipt["appliedMode"] = "baseline"
        receipt["mutationsPerformed"] = True
        receipt["persistentChangeScope"] = "reviewed firewall baseline, restored after the run"
        confirmed = api.confirm()
        receipt["confirmed"] = confirmed.get("applied") is True
        if not receipt["confirmed"]:
            raise GateError(stage, "confirm_rejected")
        after = api.status()
        if not bool(after.get("baselineActive")):
            raise GateError("verify", "baseline_not_active")

        stage = "restore"
        if receipt["initialMode"] not in ("baseline",):
            restored = api.set_mode("disabled")
            if restored.get("applied") is not True:
                raise GateError(stage, str(restored.get("rejection") or "restore_rejected"))
            receipt["restoredMode"] = "disabled"
            final = api.status()
            if bool(final.get("baselineActive")):
                raise GateError(stage, "baseline_still_active")
        else:
            receipt["restoredMode"] = "baseline"
    except GateError as error:
        if receipt["mutationsPerformed"] and api is not None:
            try:
                api.set_mode("disabled" if receipt["initialMode"] != "baseline" else "baseline")
            except Exception:  # noqa: BLE001 - best effort; the failure is primary
                pass
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
