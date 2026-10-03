#!/usr/bin/env python3
"""Switches the board's H.264 engine through the API, as the video menu
does, and measures when each switch shows in `GET /api/vm/screen`
(docs/design/h264-engine-choice-and-quality-monitor-v1.md, gate C1).

usage: engine_switch_probe.py --base-url https://HOST/ --username NAME
         --switches N --period SECONDS --output evidence/private/NAME.json
         [--stop-file PATH]

The GUI password is read from stdin. The script logs in once, reads the
screen's settings, and then chooses x264 and the video engine in turn
(`POST /api/vm/screen` with `h264_engine` 0 and 1), N times in all, one
every PERIOD seconds; after each choice it reads the screen until
`h264Encoder` names what was chosen, at most five seconds. Where the stop
file appears it switches no more. It ends with the choice it found, whatever
happens, and logs out. It asks for no lease and
touches nothing else. A receipt without tokens is written under
evidence/private; the status is 0 where every switch showed within two
seconds.
"""

from __future__ import annotations

import argparse
import hashlib
import http.cookiejar
import json
import socket
import ssl
import statistics
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

EFFECT_LIMIT = 2.0
WAIT_LIMIT = 5.0
ENCODER_OF = {0: "x264", 1: "hardware"}
VALUE_OF = {"software": 0, "hardware": 1}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--switches", type=int, required=True)
    parser.add_argument("--period", type=float, required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--stop-file")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    if not 1 <= args.switches <= 400 or not 1.0 <= args.period <= 60.0:
        raise SystemExit("switches or period out of range")
    password = sys.stdin.read().rstrip("\r\n")
    if not 8 <= len(password) <= 256:
        raise SystemExit("password from stdin is missing")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/"):
        raise SystemExit("base URL must be https://host/")

    # The test board's certificate is its own: its fingerprint is recorded
    # instead of a CA's word.
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    with socket.create_connection((url.hostname, url.port or 443), timeout=10) as raw:
        with context.wrap_socket(raw, server_hostname=url.hostname) as tls:
            fingerprint = hashlib.sha256(tls.getpeercert(binary_form=True)).hexdigest()
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    )
    origin = f"https://{url.netloc}"
    csrf = ""

    def call(method: str, route: str, body: object | None = None) -> tuple[int, dict]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(f"{origin}{route}", data=data, method=method)
        request.add_header("Origin", origin)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        try:
            with opener.open(request, timeout=15) as response:
                return response.status, json.loads(response.read() or b"{}")
        except urllib.error.HTTPError as error:
            return error.code, {}

    receipt: dict[str, object] = {
        "schemaVersion": 1,
        "probe": "h264-engine-switch-via-api",
        "tlsCertificateSha256": fingerprint,
        "switches": args.switches,
        "periodSeconds": args.period,
    }
    rows: list[dict[str, object]] = []
    result = "failed"
    found_value: int | None = None
    try:
        status, body = call("POST", "/api/auth/login", {"username": args.username, "password": password})
        password = ""
        csrf = (body.get("data") or {}).get("csrfToken", "")
        if status != 200 or not csrf:
            receipt["error"] = f"login answered {status}"
            return finish(output, receipt, rows, result)
        status, body = call("GET", "/api/vm/screen")
        screen = body.get("data") or {}
        receipt["screenBefore"] = {
            key: screen.get(key)
            for key in ("h264Engine", "h264Encoder", "h264HardwareAvailable", "h264HardwareReason", "webrtcKbps", "fps", "quality")
        }
        if screen.get("h264Engine") not in VALUE_OF:
            receipt["error"] = "the board does not offer the engine choice"
            return finish(output, receipt, rows, result)
        found_value = VALUE_OF[screen["h264Engine"]]
        for index in range(args.switches):
            if args.stop_file and Path(args.stop_file).exists():
                receipt["stopped"] = index
                break
            value = 0 if index % 2 == 0 else 1
            sent = time.monotonic()
            status, body = call("POST", "/api/vm/screen", {"type": "h264_engine", "value": value})
            answered = time.monotonic() - sent
            data = body.get("data") or {}
            effect = None
            reads = 0
            seen = None
            while time.monotonic() - sent < WAIT_LIMIT:
                reads += 1
                _, screen_body = call("GET", "/api/vm/screen")
                seen = (screen_body.get("data") or {}).get("h264Encoder")
                if seen == ENCODER_OF[value]:
                    effect = time.monotonic() - sent
                    break
                time.sleep(0.05)
            rows.append(
                {
                    "value": value,
                    "http": status,
                    "applied": data.get("applied"),
                    "answeredSeconds": round(answered, 3),
                    "effectSeconds": None if effect is None else round(effect, 3),
                    "reads": reads,
                    "lastSeen": seen,
                }
            )
            time.sleep(max(0.0, sent + args.period - time.monotonic()))
        result = "passed" if all(
            row["applied"] is True and row["effectSeconds"] is not None and row["effectSeconds"] <= EFFECT_LIMIT
            for row in rows
        ) and len(rows) == args.switches else "failed"
    except (OSError, ValueError) as error:
        receipt["error"] = str(error)[:200]
        result = "failed"
    finally:
        # The choice as it was found.
        if csrf and found_value is not None:
            last = rows[-1]["value"] if rows else found_value
            if last != found_value:
                status, body = call("POST", "/api/vm/screen", {"type": "h264_engine", "value": found_value})
                receipt["restored"] = (body.get("data") or {}).get("h264Engine")
            _, body = call("GET", "/api/vm/screen")
            screen = body.get("data") or {}
            receipt["screenAfter"] = {
                key: screen.get(key) for key in ("h264Engine", "h264Encoder", "h264HardwareAvailable", "webrtcKbps")
            }
        if csrf:
            call("POST", "/api/auth/logout", {})
    return finish(output, receipt, rows, result)


def finish(output: Path, receipt: dict[str, object], rows: list[dict[str, object]], result: str) -> int:
    def spread(values: list[float]) -> dict[str, float] | None:
        if not values:
            return None
        ordered = sorted(values)
        return {
            "median": round(statistics.median(ordered), 3),
            "p95": round(ordered[min(len(ordered) - 1, int(len(ordered) * 0.95))], 3),
            "highest": round(ordered[-1], 3),
        }

    for value, name in ((0, "toX264"), (1, "toHardware")):
        mine = [row for row in rows if row["value"] == value]
        receipt[name] = {
            "switches": len(mine),
            "effectSeconds": spread([float(row["effectSeconds"]) for row in mine if row["effectSeconds"] is not None]),
            "missed": sum(
                1
                for row in mine
                if row["effectSeconds"] is None or float(row["effectSeconds"]) > EFFECT_LIMIT or row["applied"] is not True
            ),
        }
    receipt["rows"] = rows
    receipt["result"] = result
    output.write_text(json.dumps(receipt, indent=1) + "\n")
    print(json.dumps({key: receipt.get(key) for key in ("toX264", "toHardware", "screenBefore", "screenAfter", "result", "error")}))
    return 0 if result == "passed" else 1


if __name__ == "__main__":
    sys.exit(main())
