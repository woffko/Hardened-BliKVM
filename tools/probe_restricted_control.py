#!/usr/bin/env python3
"""Probe the restricted HID control path end to end without injecting input.

The probe logs in with the operator account and exercises the server focus,
lease acquire, a deliberately empty keyboard report and the lease release. The
all-zero keyboard report translates to no KVMD events, so no key or pointer
motion reaches the target; the probe only proves the control path and records
the fixed rejection codes.

The password is read from stdin. The username is read from a private file so it
never appears in the process arguments or in the receipt.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
from pathlib import Path
import secrets
import ssl
import stat
import sys
from typing import Any, cast
from urllib.parse import urlsplit

if not __package__:
    sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype


EMPTY_KEYBOARD = [0, 0, 0, 0, 0, 0, 0, 0]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--base-url", required=True)
    _ = parser.add_argument("--username-file", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument(
        "--press-key",
        action="store_true",
        help="also press and release KeyA (visible single keystroke) to test the backend report",
    )
    return parser.parse_args()


def read_password() -> str:
    data = sys.stdin.buffer.read(4096)
    password = data.decode("utf-8", errors="strict").rstrip("\r\n")
    if not 8 <= len(password) <= 256:
        raise ValueError("invalid password input")
    return password


def read_username(path_value: str) -> str:
    path = Path(path_value)
    metadata = path.stat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > 128:
        raise ValueError("username file is invalid")
    username = path.read_text(encoding="utf-8").strip()
    if not username or "\n" in username:
        raise ValueError("username is invalid")
    return username


def target_id_for(platform_id: str) -> str:
    digest = hashlib.sha256(f"hardened-blikvm-target-v1:{platform_id}".encode("utf-8")).digest()
    return digest[:16].hex()


class Client:
    def __init__(self, base_url: str) -> None:
        parts = urlsplit(base_url)
        if parts.scheme != "https" or not parts.hostname:
            raise ValueError("base URL must be https")
        self.origin = f"https://{parts.hostname}:{parts.port}"
        self.host = cast(str, parts.hostname)
        self.port = cast(int, parts.port)
        self.cookies: dict[str, str] = {}
        self.csrf_token: str | None = None
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_CLIENT)
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE
        self.context = context

    def request(
        self,
        method: str,
        route: str,
        body: dict[str, Any] | None = None,
        *,
        csrf: bool = False,
    ) -> tuple[int, dict[str, Any]]:
        connection = http.client.HTTPSConnection(self.host, self.port, context=self.context, timeout=10)
        headers = {"Accept": "application/json", "Origin": self.origin, "Host": f"{self.host}:{self.port}"}
        payload = None
        if body is not None:
            payload = json.dumps(body).encode("utf-8")
            headers["Content-Type"] = "application/json"
        if csrf and self.csrf_token:
            headers["x-csrf-token"] = self.csrf_token
        if self.cookies:
            headers["Cookie"] = "; ".join(f"{k}={v}" for k, v in self.cookies.items())
        try:
            connection.request(method, route, body=payload, headers=headers)
            response = connection.getresponse()
            raw = response.read()
            for header, value in response.getheaders():
                if header.lower() == "set-cookie":
                    name, _, rest = value.partition("=")
                    cookie_value = rest.split(";", 1)[0]
                    self.cookies[name.strip()] = cookie_value
            try:
                decoded = json.loads(raw.decode("utf-8")) if raw else {}
            except (UnicodeError, json.JSONDecodeError):
                decoded = {}
            return response.status, cast(dict[str, Any], decoded)
        finally:
            connection.close()

    def data(self, decoded: dict[str, Any]) -> dict[str, Any]:
        value = decoded.get("data")
        return cast(dict[str, Any], value) if isinstance(value, dict) else {}


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    password = read_password()
    username = read_username(args.username_file)
    client = Client(args.base_url)
    facts: dict[str, Any] = {}

    status, body = client.request(
        "POST", "/api/auth/login", {"username": username, "password": password}
    )
    facts["login"] = status
    if status != 200:
        facts["failure"] = "login"
        write_json_0600_atomic(output, facts)
        return 1
    csrf_token = client.data(body).get("csrfToken")
    if isinstance(csrf_token, str) and csrf_token:
        client.csrf_token = csrf_token

    status, body = client.request("GET", "/api/capabilities")
    platform_id = str(client.data(body).get("platform", {}).get("id", ""))
    capabilities = client.data(body)
    facts["hid_capability"] = capabilities.get("hid", {}).get("status", {}).get("availability")
    facts["power_capability"] = capabilities.get("power", {}).get("status", {}).get("availability")
    if not platform_id:
        facts["failure"] = "platform_id"
        write_json_0600_atomic(output, facts)
        return 1

    status, body = client.request("GET", "/api/hid/focus")
    challenge = client.data(body).get("challenge")
    facts["focus_challenge"] = status
    if status != 200 or not isinstance(challenge, str):
        facts["failure"] = "focus_challenge"
        facts["focus_challenge_body"] = body.get("code", body)
        write_json_0600_atomic(output, facts)
        return 1

    status, body = client.request(
        "POST",
        "/api/hid/focus",
        {
            "operationId": secrets.token_hex(16),
            "challenge": challenge,
            "targetId": target_id_for(platform_id),
            "operatorConfirmed": True,
        },
        csrf=True,
    )
    focus_data = client.data(body)
    facts["focus_confirm"] = status
    facts["focus_attested"] = focus_data.get("attested")
    facts["focus_rejection"] = focus_data.get("rejection")
    if status != 200 or focus_data.get("attested") is not True:
        facts["failure"] = "focus_confirm"
        write_json_0600_atomic(output, facts)
        return 1

    status, body = client.request(
        "POST",
        "/api/hid/lease",
        {"action": "acquire", "operationId": secrets.token_hex(16), "ttlSeconds": 30},
        csrf=True,
    )
    lease_data = client.data(body)
    lease_id = lease_data.get("leaseId")
    facts["lease_acquire"] = status
    facts["lease_applied"] = lease_data.get("applied")
    facts["lease_rejection"] = lease_data.get("rejection")
    if status != 200 or lease_data.get("applied") is not True or not isinstance(lease_id, str):
        facts["failure"] = "lease_acquire"
        write_json_0600_atomic(output, facts)
        return 1

    status, body = client.request(
        "POST",
        "/api/hid/lease",
        {
            "action": "report",
            "operationId": secrets.token_hex(16),
            "leaseId": lease_id,
            "report": {"mode": "keyboard", "data": EMPTY_KEYBOARD},
        },
        csrf=True,
    )
    report_data = client.data(body)
    facts["report_noop"] = status
    facts["report_applied"] = report_data.get("applied")
    facts["report_rejection"] = report_data.get("rejection")

    if args.press_key:
        for label, data in (
            ("press", [0, 0, 4, 0, 0, 0, 0, 0]),
            ("release", EMPTY_KEYBOARD),
        ):
            status, body = client.request(
                "POST",
                "/api/hid/lease",
                {
                    "action": "report",
                    "operationId": secrets.token_hex(16),
                    "leaseId": lease_id,
                    "report": {"mode": "keyboard", "data": data},
                },
                csrf=True,
            )
            event_data = client.data(body)
            facts[f"key_{label}"] = status
            facts[f"key_{label}_applied"] = event_data.get("applied")
            facts[f"key_{label}_rejection"] = event_data.get("rejection")

    status, body = client.request(
        "POST",
        "/api/hid/lease",
        {"action": "release", "operationId": secrets.token_hex(16), "leaseId": lease_id},
        csrf=True,
    )
    release_data = client.data(body)
    facts["lease_release"] = status
    facts["release_applied"] = release_data.get("applied")
    facts["release_rejection"] = release_data.get("rejection")

    wrote = client.request("POST", "/api/auth/logout", {}, csrf=True)[0]
    facts["logout"] = wrote
    ok = (
        facts.get("focus_attested") is True
        and facts.get("lease_applied") is True
        and facts.get("report_applied") is True
        and facts.get("release_applied") is True
    )
    facts["result"] = "passed" if ok else "failed"
    write_json_0600_atomic(output, facts)
    return 0 if ok else 1


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, http.client.HTTPException):
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
