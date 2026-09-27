#!/usr/bin/env python3
"""Measure the board owner's shared lock while the network pages load.

HID reports take the owner's shared state. This probe times a cheap request
on that state (the mouse-jiggler read) while idle and while the DNS, IPv6 and
About reads, which read NetworkManager, load in a loop, and passes when the
90th percentile under load stays below the limit. The GUI password is read
from stdin (via project-memory-exec); a receipt is written under
evidence/private.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import ssl
import sys
import threading
import time
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

LOAD = ("/api/network/dns", "/api/network/ipv6", "/api/vm/info")
PROBE = "/api/vm/mouse-jiggler"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--samples", type=int, default=40)
    parser.add_argument("--limit-ms", type=int, default=100)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/"):
        raise SystemExit("base URL must be https://host/")
    password = sys.stdin.read().rstrip("\r\n")
    origin = f"https://{url.netloc}"
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    jar = http.cookiejar.CookieJar()

    def opener() -> urllib.request.OpenerDirector:
        return urllib.request.build_opener(
            urllib.request.HTTPSHandler(context=context), urllib.request.HTTPCookieProcessor(jar)
        )

    def call(route: str, body: dict | None = None, csrf: str = "") -> tuple[int, dict]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(f"{origin}{route}", data=data, method="POST" if body else "GET")
        request.add_header("Origin", origin)
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        started = time.monotonic()
        with opener().open(request, timeout=60) as response:
            parsed = json.loads(response.read() or b"{}")
        return int((time.monotonic() - started) * 1000), parsed

    _, login = call("/api/auth/login", {"username": args.username, "password": password})
    password = ""
    csrf = (login.get("data") or {}).get("csrfToken", "")
    if not csrf:
        raise SystemExit("login failed")

    def sample(count: int) -> list[int]:
        times = []
        for _ in range(count):
            times.append(call(PROBE, csrf=csrf)[0])
            time.sleep(0.05)
        return sorted(times)

    def summary(times: list[int]) -> dict[str, int]:
        pick = lambda q: times[min(len(times) - 1, int(len(times) * q))]
        return {"p50": pick(0.5), "p90": pick(0.9), "max": times[-1], "count": len(times)}

    idle = sample(args.samples // 2)
    stop = threading.Event()
    load_times: list[int] = []

    def load() -> None:
        while not stop.is_set():
            for route in LOAD:
                load_times.append(call(route, csrf=csrf)[0])

    worker = threading.Thread(target=load)
    worker.start()
    time.sleep(0.3)
    loaded = sample(args.samples)
    stop.set()
    worker.join()
    result = summary(loaded)["p90"] < args.limit_ms
    receipt = {
        "schemaVersion": 1,
        "probe": "owner-lock-latency",
        "idle": summary(idle),
        "whileNetworkPagesLoad": summary(loaded),
        "networkReads": summary(sorted(load_times)) if load_times else None,
        "limitMs": args.limit_ms,
        "result": "passed" if result else "failed",
    }
    output.write_text(json.dumps(receipt, indent=1) + "\n", encoding="utf-8")
    output.chmod(0o600)
    print(json.dumps(receipt))
    return 0 if result else 1


if __name__ == "__main__":
    sys.exit(main())
