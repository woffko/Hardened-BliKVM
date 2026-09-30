#!/usr/bin/env python3
"""Time HID report round trips while video plays (plan I5, goal).

Confirms focus like the GUI, takes the HID lease, sends relative mouse
reports with no movement and no buttons (the target's pointer stays where it
is), times each report's request, and releases the lease. An MJPEG viewer
reads the stream meanwhile, so the reports compete with video as they do in
use. Run only while the operator is not controlling the board: taking the
lease takes control away from them.

The GUI password is read from stdin (via project-memory-exec); a receipt is
written under evidence/private.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import secrets
import ssl
import sys
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlparse


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--samples", type=int, default=200)
    parser.add_argument("--limit-ms", type=float, default=40)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/"):
        raise SystemExit("base URL must be https://host/")
    origin = f"https://{url.netloc}"
    password = sys.stdin.read().rstrip("\r\n")
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    )
    csrf = ""

    def call(method: str, route: str, body: object = None) -> dict:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(f"{origin}{route}", data=data, method=method)
        request.add_header("Origin", origin)
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        try:
            with opener.open(request, timeout=30) as response:
                return json.loads(response.read() or b"{}")
        except urllib.error.HTTPError as error:
            return {"http": error.code}

    operation = lambda: secrets.token_hex(16)  # noqa: E731
    login = call("POST", "/api/auth/login", {"username": args.username, "password": password})
    password = ""
    csrf = (login.get("data") or {}).get("csrfToken", "")
    if not csrf:
        raise SystemExit("login failed")
    platform = (call("GET", "/api/capabilities").get("data") or {}).get("platform", {}).get("id", "")
    target = hashlib.sha256(f"hardened-blikvm-target-v1:{platform}".encode()).digest()[:16].hex()

    watching = threading.Event()
    stop = threading.Event()

    def watch() -> None:
        request = urllib.request.Request(f"{origin}/api/stream/mjpeg")
        request.add_header("Origin", origin)
        with opener.open(request, timeout=30) as stream:
            watching.set()
            while not stop.is_set() and stream.read1(65536):
                pass

    viewer = threading.Thread(target=watch, daemon=True)
    viewer.start()
    watching.wait(15)

    challenge = (call("GET", "/api/hid/focus").get("data") or {}).get("challenge", "")
    focus = call(
        "POST",
        "/api/hid/focus",
        {"operationId": operation(), "challenge": challenge, "targetId": target, "operatorConfirmed": True},
    )
    if not (focus.get("data") or {}).get("attested"):
        raise SystemExit(f"focus not attested: {focus}")
    grant = call("POST", "/api/hid/lease", {"action": "acquire", "operationId": operation(), "ttlSeconds": 30})
    lease = (grant.get("data") or {}).get("leaseId")
    if not lease:
        raise SystemExit(f"lease not granted: {grant}")
    trips: list[float] = []
    refused = 0
    try:
        for _ in range(args.samples):
            start = time.monotonic()
            answer = call(
                "POST",
                "/api/hid/lease",
                {
                    "action": "report",
                    "operationId": operation(),
                    "leaseId": lease,
                    "report": {"mode": "relative_mouse", "data": [0, 0, 0, 0]},
                },
            )
            trips.append((time.monotonic() - start) * 1000)
            refused += not (answer.get("data") or {}).get("applied")
    finally:
        call("POST", "/api/hid/lease", {"action": "release", "operationId": operation(), "leaseId": lease})
        stop.set()
    receipt = {
        "schemaVersion": 1,
        "probe": "hid-report-latency",
        "videoPlaying": watching.is_set(),
        "reports": len(trips),
        "refused": refused,
        "roundTripMs": {
            "p50": round(percentile(trips, 0.5), 1),
            "p90": round(percentile(trips, 0.9), 1),
            "max": round(max(trips), 1),
        },
    }
    receipt["result"] = (
        "passed" if receipt["roundTripMs"]["p90"] <= args.limit_ms and not refused and watching.is_set() else "failed"
    )
    output.write_text(json.dumps(receipt, indent=1) + "\n", encoding="utf-8")
    output.chmod(0o600)
    print(json.dumps(receipt))
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
