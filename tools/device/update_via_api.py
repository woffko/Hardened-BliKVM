#!/usr/bin/env python3
"""Upload and activate one signed application bundle through the device API.

The GUI password is read from stdin (for example via project-memory-exec);
the base URL and account come from arguments or the operator environment.
Progress is recorded as a receipt under evidence/private. The TLS certificate
of the test board is self-signed, so its SHA-256 fingerprint is recorded
instead of being validated against a CA.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import os
import socket
import ssl
import sys
import time
import urllib.request
from pathlib import Path
from urllib.parse import urlparse


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--bundle", required=True)
    parser.add_argument("--expected-sha256", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--timeout", type=int, default=240)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    bundle = Path(args.bundle).read_bytes()
    if hashlib.sha256(bundle).hexdigest() != args.expected_sha256:
        raise SystemExit("bundle SHA-256 mismatch")
    password = sys.stdin.read().rstrip("\r\n")
    if not 8 <= len(password) <= 256:
        raise SystemExit("password from stdin is missing")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/"):
        raise SystemExit("base URL must be https://host/")

    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    with socket.create_connection((url.hostname, url.port or 443), timeout=10) as raw:
        with context.wrap_socket(raw, server_hostname=url.hostname) as tls:
            fingerprint = hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest()
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context), urllib.request.HTTPCookieProcessor(jar)
    )
    origin = f"https://{url.netloc}"
    csrf = ""

    def call(method: str, route: str, body: bytes | None = None, kind: str | None = None, timeout: int = 30):
        request = urllib.request.Request(f"{origin}{route}", data=body, method=method)
        request.add_header("Origin", origin)
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        if kind:
            request.add_header("Content-Type", kind)
        try:
            with opener.open(request, timeout=timeout) as response:
                return response.status, json.loads(response.read() or b"{}")
        except urllib.error.HTTPError as error:
            return error.code, json.loads(error.read() or b"{}")

    receipt: dict[str, object] = {
        "schemaVersion": 1,
        "probe": "application-update-via-api",
        "tlsCertificateSha256": fingerprint,
        "bundleSha256": args.expected_sha256,
        "steps": [],
    }
    steps: list[dict[str, object]] = receipt["steps"]  # type: ignore[assignment]

    def record(name: str, status: int, body: dict) -> dict:
        data = body.get("data") if isinstance(body, dict) else None
        steps.append({"step": name, "http": status, "code": body.get("code"), "data": data})
        return data or {}

    def login() -> bool:
        nonlocal csrf
        csrf = ""
        payload = json.dumps({"username": args.username, "password": password}).encode()
        try:
            status, body = call("POST", "/api/auth/login", payload, "application/json", 10)
        except OSError:
            return False
        csrf = (body.get("data") or {}).get("csrfToken", "")
        steps.append({"step": "login", "http": status, "code": body.get("code")})
        return status == 200 and bool(csrf)

    if not login():
        return finish(output, receipt, "login_failed")
    before = record("status_before", *call("GET", "/api/application-update/status"))
    status, body = call("POST", "/api/application-update/upload", bundle, "application/octet-stream", 300)
    uploaded = record("upload", status, body)
    slot = (uploaded.get("slot") or {}).get("slotId")
    if status >= 300 or not slot:
        return finish(output, receipt, "upload_failed")
    generation = before.get("generation")
    activation = json.dumps({"slotId": slot, "expectedGeneration": generation}).encode()
    status, body = call("POST", "/api/application-update/activate", activation, "application/json")
    activated = record("activate", status, body)
    receipt["activationPhase"] = activated.get("phase")
    if status >= 300:
        return finish(output, receipt, "activation_refused")
    deadline = time.monotonic() + args.timeout
    last: dict = {}
    while time.monotonic() < deadline:
        time.sleep(5)
        try:
            status, body = call("GET", "/api/application-update/status", timeout=10)
        except OSError:
            continue
        if status == 401:
            # The activation restarts the server, which drops its sessions.
            login()
            continue
        if status != 200:
            continue
        last = body.get("data") or {}
        if not last.get("pending"):
            break
    steps.append({"step": "status_after", "data": last})
    confirmed = (last.get("confirmed") or {}).get("slotId")
    return finish(output, receipt, "confirmed" if confirmed == slot else "not_confirmed")


def finish(output: Path, receipt: dict, result: str) -> int:
    receipt["result"] = result
    descriptor = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        json.dump(receipt, handle, indent=1)
    print(result)
    return 0 if result == "confirmed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
