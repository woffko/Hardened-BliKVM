#!/usr/bin/env python3
"""Capture a bounded, sanitized, read-only PiKVM API baseline.

The password is read once from stdin. Raw responses, credentials, endpoint
hostnames, and device identifiers are never written to the output file.
"""

from __future__ import annotations

import argparse
import base64
from dataclasses import dataclass
from http.client import HTTPResponse
import http.cookiejar
import json
import os
from pathlib import Path
import re
import ssl
import sys
import tempfile
from typing import TypedDict, cast
from urllib import error, parse, request


READ_ONLY_PATHS = (
    "/api/info",
    "/api/atx",
    "/api/hid",
    "/api/msd",
    "/api/streamer",
)
SENSITIVE_KEY = re.compile(
    r"(?:auth|cookie|credential|key|login|pass|secret|serial|ssid|token|user)",
    re.IGNORECASE,
)
VERSION_KEY = re.compile(r"(?:build|kernel|kvmd|streamer|version)", re.IGNORECASE)
SAFE_SCALAR = re.compile(r"^[A-Za-z0-9._:+ /()\[\]-]{1,160}$")
BOOLEAN_KEYS = {
    "active",
    "busy",
    "connected",
    "enabled",
    "healthy",
    "online",
    "power",
}
MAX_RESPONSE_BYTES = 1024 * 1024

SafeScalar = str | int | float | bool | None


class ResponseSummary(TypedDict):
    path: str
    status: int
    ok: bool
    topLevelKeys: list[str]
    resultKeys: list[str]
    safeFacts: dict[str, SafeScalar]


class FailureSummary(TypedDict):
    path: str
    errorType: str


class BaselineOutput(TypedDict):
    schemaVersion: int
    probe: str
    requests: list[ResponseSummary]
    failures: list[FailureSummary]


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    output: str
    insecure: bool


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--insecure", action="store_true")
    namespace = parser.parse_args()
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        output=cast(str, namespace.output),
        insecure=cast(bool, namespace.insecure),
    )


def validate_endpoint(endpoint: str) -> str:
    value = endpoint.rstrip("/")
    parsed = parse.urlparse(value)
    if parsed.scheme != "https" or not parsed.hostname:
        raise ValueError("endpoint must be an absolute HTTPS URL")
    if parsed.username or parsed.password or parsed.query or parsed.fragment:
        raise ValueError("endpoint must not contain credentials, query, or fragment")
    return value


def collect_safe_facts(value: object, prefix: str = "") -> dict[str, SafeScalar]:
    facts: dict[str, SafeScalar] = {}
    if isinstance(value, dict):
        mapping = cast(dict[object, object], value)
        for raw_name, child in mapping.items():
            name = str(raw_name)
            path = f"{prefix}.{name}" if prefix else name
            if SENSITIVE_KEY.search(name):
                continue
            if VERSION_KEY.search(name) and isinstance(child, (str, int, float)):
                rendered = str(child)
                if SAFE_SCALAR.fullmatch(rendered):
                    facts[path] = cast(SafeScalar, child)
                continue
            if name.casefold() in BOOLEAN_KEYS and isinstance(child, bool):
                facts[path] = child
                continue
            facts.update(collect_safe_facts(child, path))
    elif isinstance(value, list):
        sequence = cast(list[object], value)
        for index, child in enumerate(sequence[:32]):
            facts.update(collect_safe_facts(child, f"{prefix}[{index}]"))
    return facts


def response_summary(
    path: str, status: int, payload: dict[str, object]
) -> ResponseSummary:
    top_level_keys = sorted(payload)
    result = payload.get("result")
    result_mapping = (
        cast(dict[str, object], result) if isinstance(result, dict) else {}
    )
    result_keys = sorted(result_mapping)
    return {
        "path": path,
        "status": status,
        "ok": payload.get("ok") is True,
        "topLevelKeys": top_level_keys,
        "resultKeys": result_keys,
        "safeFacts": collect_safe_facts(payload),
    }


def login(
    endpoint: str, username: str, password: str, context: ssl.SSLContext
) -> tuple[request.OpenerDirector, str]:
    """Open a session with the JSON login and return the opener plus the
    CSRF token it issued. The password is used once and never stored."""
    jar = http.cookiejar.CookieJar()
    opener = request.build_opener(
        request.HTTPCookieProcessor(jar),
        request.HTTPSHandler(context=context),
    )
    body = json.dumps({"username": username, "password": password}).encode()
    request_ = request.Request(
        endpoint + "/api/auth/login",
        data=body,
        headers={
            "Accept": "application/json",
            "Content-Type": "application/json",
            "Origin": endpoint,
            "User-Agent": "BLi-KVM-baseline/0.1",
        },
        method="POST",
    )
    with cast(
        HTTPResponse, opener.open(request_, timeout=8)
    ) as response:
        payload = json.loads(response.read(MAX_RESPONSE_BYTES + 1).decode())
    token = str((payload.get("data") or {}).get("csrfToken") or "")
    if payload.get("code") != 0 or not token:
        raise ValueError("login was refused")
    return opener, token


def fetch_json(
    opener: request.OpenerDirector,
    endpoint: str,
    path: str,
) -> tuple[int, dict[str, object]]:
    url = endpoint + path
    headers = {
        "Accept": "application/json",
        "User-Agent": "BLi-KVM-baseline/0.1",
    }
    request_ = request.Request(url, headers=headers, method="GET")
    with cast(HTTPResponse, opener.open(request_, timeout=8)) as response:
        data = response.read(MAX_RESPONSE_BYTES + 1)
        if len(data) > MAX_RESPONSE_BYTES:
            raise ValueError("response exceeded the bounded size limit")
        payload = json.loads(data.decode())
    if not isinstance(payload, dict):
        raise ValueError("unexpected response shape")
    return response.status, payload


def _request_json(
    url: str, headers: dict[str, str], context: ssl.SSLContext
) -> tuple[int, dict[str, object]]:
    req = request.Request(url, headers=headers, method="GET")
    with cast(
        HTTPResponse, request.urlopen(req, timeout=8, context=context)
    ) as response:
        data = response.read(MAX_RESPONSE_BYTES + 1)
        if len(data) > MAX_RESPONSE_BYTES:
            raise ValueError("response exceeded the bounded size limit")
        decoded = cast(object, json.loads(data))
        if not isinstance(decoded, dict):
            raise ValueError("JSON response root must be an object")
        return response.status, cast(dict[str, object], decoded)


def write_json_0600_atomic(path: Path, value: object) -> None:
    path = path.resolve()
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    temporary = Path(temporary_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            _ = json.dump(value, handle, indent=2, sort_keys=True)
            _ = handle.write("\n")
            handle.flush()
            os.fsync(handle.fileno())
        temporary.chmod(0o600)
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def main() -> int:
    args = parse_args()
    endpoint = validate_endpoint(args.endpoint)
    password = sys.stdin.readline().rstrip("\r\n")
    if not password:
        raise ValueError("a password is required on stdin")
    context = ssl.create_default_context()
    if args.insecure:
        context.check_hostname = False
        context.verify_mode = ssl.CERT_NONE

    summaries: list[ResponseSummary] = []
    failures: list[FailureSummary] = []
    try:
        opener, _csrf = login(endpoint, args.username, password, context)
    except Exception as exc:  # The output intentionally omits body and URL.
        opener = None
        failures.append({"path": "/api/auth/login", "errorType": type(exc).__name__})
    if opener is not None:
        for path in READ_ONLY_PATHS:
            try:
                status, payload = fetch_json(opener, endpoint, path)
                summaries.append(response_summary(path, status, payload))
            except Exception as exc:  # The output intentionally omits body and URL.
                failures.append({"path": path, "errorType": type(exc).__name__})

    output: BaselineOutput = {
        "schemaVersion": 1,
        "probe": "pikvm-read-only",
        "requests": summaries,
        "failures": failures,
    }
    write_json_0600_atomic(Path(args.output), output)
    return 0 if summaries else 1


if __name__ == "__main__":
    raise SystemExit(main())
