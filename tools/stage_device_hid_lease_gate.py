#!/usr/bin/env python3
"""Run the HID lease supersede gate on a test board.

The gate proves the reviewed P3 input fix on the device:

- one session acquires the HID lease, then asks again *without releasing*
  (the "lost lease id" cycle); the owner must supersede its own lease and grant
  a fresh one instead of answering ``lease_conflict``;
- a report with the fresh lease must be applied;
- a *second*, independent session must still be refused with
  ``lease_conflict`` while the first holds the lease, so the fix never weakens
  the cross-session protection;
- the first session releases the lease and the run leaves no lease behind.

The operator password arrives on the second stdin line. Without ``--confirm``
the gate only observes.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import hashlib
import json
import secrets
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
TARGET_DOMAIN = "hardened-blikvm-target-v1"
CONFLICT_REJECTIONS = {"lease_conflict", "busy"}


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
    platformId: str
    targetId: str
    firstLeaseId: str
    secondLeaseId: str
    superseded: bool
    reportsApplied: int
    reportRejections: int
    secondSessionRefused: bool
    secondSessionRejection: str
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
    ttl_seconds: int
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
    _ = parser.add_argument("--ttl-seconds", type=int, default=30)
    _ = parser.add_argument(
        "--confirm",
        action="store_true",
        help="exercise the supersede and conflict paths; without it the gate observes",
    )
    _ = parser.add_argument("--timeout-seconds", type=int, default=60)
    namespace = parser.parse_args()
    if not 5 <= cast(int, namespace.ttl_seconds) <= 120:
        raise SystemExit("--ttl-seconds must be between 5 and 120")
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
        ttl_seconds=cast(int, namespace.ttl_seconds),
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


def target_id(platform: str) -> str:
    return hashlib.sha256(f"{TARGET_DOMAIN}:{platform}".encode()).hexdigest()[:32]


class BoardSession:
    """One authenticated session with its own cookie jar."""

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

    def platform(self) -> str:
        data = self.data("GET", "/api/capabilities")
        platform = (cast(Json, data.get("platform") or {})).get("id")
        return platform if isinstance(platform, str) and platform else "bli-h616-pikvm"

    def confirm_focus(self, target: str) -> None:
        challenge = self.data("GET", "/api/hid/focus").get("challenge")
        if not isinstance(challenge, str) or not challenge:
            raise GateError("focus", "challenge_missing")
        data = self.data(
            "POST",
            "/api/hid/focus",
            {
                "operationId": secrets.token_hex(16),
                "challenge": challenge,
                "targetId": target,
                "operatorConfirmed": True,
            },
        )
        if data.get("confirmed") is False:
            raise GateError("focus", "not_confirmed")

    def lease(self, action: str, ttl: int, lease_id: str = "") -> Json:
        payload: Json = {"action": action, "operationId": secrets.token_hex(16)}
        if action == "acquire":
            payload["ttlSeconds"] = ttl
        elif lease_id:
            payload["leaseId"] = lease_id
        body = self._request("POST", "/api/hid/lease", payload)
        if body.get("code") != 0:
            raise GateError("api", f"lease_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})

    def report(self, lease_id: str, dx: int = 1) -> Json:
        body = self._request(
            "POST",
            "/api/hid/lease",
            {
                "action": "report",
                "operationId": secrets.token_hex(16),
                "leaseId": lease_id,
                "report": {"mode": "relative_mouse", "data": [0, dx, 0, 0]},
            },
        )
        if body.get("code") != 0:
            raise GateError("api", f"report_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    _secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-hid-lease-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "platformId": "",
        "targetId": "",
        "firstLeaseId": "",
        "secondLeaseId": "",
        "superseded": False,
        "reportsApplied": 0,
        "reportRejections": 0,
        "secondSessionRefused": False,
        "secondSessionRejection": "",
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    # The operator password is needed for the second session as well, so it is
    # held until both sessions exist and only then dropped.
    password_holder = password
    password = ""
    stage = "read"

    try:
        first = BoardSession(options, password_holder)
        platform = first.platform()
        target = target_id(platform)
        receipt["platformId"] = platform
        receipt["targetId"] = target
        first.confirm_focus(target)
        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "supersede"
        lease_first = first.lease("acquire", options.ttl_seconds)
        first_id = str(lease_first.get("leaseId") or "")
        if lease_first.get("applied") is not True or not first_id:
            raise GateError(stage, "first_acquire_rejected")
        # The client loses its lease id and asks again without releasing.
        lease_second = first.lease("acquire", options.ttl_seconds)
        second_id = str(lease_second.get("leaseId") or "")
        receipt["firstLeaseId"] = first_id
        receipt["secondLeaseId"] = second_id
        if lease_second.get("applied") is not True or not second_id:
            raise GateError(
                stage,
                "supersede_"
                + str(lease_second.get("rejection") or "not_applied"),
            )
        if second_id == first_id:
            raise GateError(stage, "lease_id_not_rotated")
        receipt["superseded"] = True
        receipt["mutationsPerformed"] = True
        receipt["persistentChangeScope"] = "one HID lease in the owner, released after the run"

        stage = "report"
        applied = 0
        rejections = 0
        for index in range(5):
            try:
                first.report(second_id, dx=1 if index % 2 == 0 else 255)
                applied += 1
            except GateError:
                rejections += 1
        receipt["reportsApplied"] = applied
        receipt["reportRejections"] = rejections
        if applied != 5 or rejections != 0:
            raise GateError(stage, "report_rejected")

        stage = "conflict"
        second = BoardSession(options, password_holder)
        second.confirm_focus(target)
        other = second.lease("acquire", options.ttl_seconds)
        receipt["secondSessionRejection"] = str(other.get("rejection") or "")
        if other.get("applied") is True:
            # The second session must never take the first session's lease.
            first.lease("release", 0, second_id)
            raise GateError(stage, "second_session_admitted")
        if receipt["secondSessionRejection"] not in CONFLICT_REJECTIONS:
            raise GateError(stage, "unexpected_conflict_rejection")
        receipt["secondSessionRefused"] = True

        stage = "release"
        released = first.lease("release", 0, second_id)
        if released.get("applied") is not True:
            raise GateError(stage, "release_rejected")
        receipt["persistentChangeScope"] = "none"
        password_holder = ""
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
