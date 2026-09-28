#!/usr/bin/env python3
"""Measure how far MJPEG viewers trail uStreamer (plan J1, J5).

uStreamer stamps every part with `X-Timestamp`, the board's wall clock when
the frame left it (the board keeps NTP time). Two viewers read
`/api/stream/mjpeg` at once: one at full speed, and one standing for a
slower link (a capped reading rate and a small receive buffer, so frames
cannot wait in its own socket). A frame's delay is its complete arrival in
true time minus its stamp. Arrivals are taken on this host's monotonic clock
and converted to true time with two SNTP readings, before and after: a WSL
host's monotonic clock was seen running at a steady 0.9 of true time while
its wall clock was stepped back and forth.

The GUI password is read from stdin (via project-memory-exec); a receipt is
written under evidence/private.
"""

from __future__ import annotations

import argparse
import http.client
import http.cookiejar
import json
import socket
import ssl
import struct
import sys
import threading
import time
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

NTP_EPOCH = 2208988800


def sntp_anchor(server: str) -> tuple[float, float]:
    """(this host's monotonic time, true time) at one SNTP reading."""
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.settimeout(3)
        sent = time.monotonic()
        udp.sendto(b"\x1b" + 47 * b"\0", (server, 123))
        reply, _ = udp.recvfrom(48)
        received = time.monotonic()

    def stamp(offset: int) -> float:
        seconds, fraction = struct.unpack("!II", reply[offset : offset + 8])
        return seconds - NTP_EPOCH + fraction / 2**32

    return (sent + received) / 2, (stamp(32) + stamp(40)) / 2


def percentile(values: list[float], fraction: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(fraction * len(ordered)))]


def watch(
    host: str,
    headers: dict[str, str],
    seconds: float,
    rate_mbit: float,
    receive_buffer: int,
    context: ssl.SSLContext,
    arrivals: list[tuple[float, float]],
) -> None:
    """Records (stamp, monotonic time) for each frame that arrived whole."""
    raw = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    if receive_buffer:
        raw.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, receive_buffer)
    raw.settimeout(30)
    raw.connect((host, 443))
    connection = http.client.HTTPSConnection(host, context=context)
    connection.sock = context.wrap_socket(raw, server_hostname=host)
    connection.request("GET", "/api/stream/mjpeg", headers=headers)
    response = connection.getresponse()
    rate = rate_mbit * 1_000_000 / 8
    buffer = b""
    total = 0
    start = time.monotonic()
    while time.monotonic() - start < seconds:
        chunk = response.read1(65536)
        if not chunk:
            break
        total += len(chunk)
        if rate:
            delay = start + total / rate - time.monotonic()
            if delay > 0:
                time.sleep(delay)
        buffer += chunk
        while True:
            stamp = buffer.find(b"X-Timestamp: ")
            line_end = buffer.find(b"\r\n", stamp) if stamp >= 0 else -1
            frame_end = buffer.find(b"\xff\xd9", line_end) if line_end >= 0 else -1
            if frame_end < 0:
                break
            arrivals.append((float(buffer[stamp + 13 : line_end]), time.monotonic()))
            buffer = buffer[frame_end + 2 :]
    connection.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--ntp-server", default="time.google.com")
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--slow-mbit", type=float, default=20)
    parser.add_argument("--slow-receive-buffer", type=int, default=64 * 1024)
    parser.add_argument("--limit-ms", type=float, default=300)
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/") or not url.hostname:
        raise SystemExit("base URL must be https://host/")
    origin = f"https://{url.netloc}"
    password = sys.stdin.read().rstrip("\r\n")
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context), urllib.request.HTTPCookieProcessor(jar)
    )
    login = urllib.request.Request(
        f"{origin}/api/auth/login",
        data=json.dumps({"username": args.username, "password": password}).encode(),
        method="POST",
    )
    password = ""
    login.add_header("Origin", origin)
    login.add_header("Content-Type", "application/json")
    if json.loads(opener.open(login, timeout=30).read()).get("code") != 0:
        raise SystemExit("login failed")
    headers = {
        "Host": url.netloc,
        "Origin": origin,
        "Cookie": "; ".join(f"{cookie.name}={cookie.value}" for cookie in jar),
    }

    viewers = {
        "fullSpeed": (0.0, 0),
        f"slow{args.slow_mbit:g}Mbit": (args.slow_mbit, args.slow_receive_buffer),
    }
    arrivals: dict[str, list[tuple[float, float]]] = {name: [] for name in viewers}
    before = sntp_anchor(args.ntp_server)
    threads = [
        threading.Thread(
            target=watch,
            args=(url.hostname, headers, args.seconds, rate, buffer, context, arrivals[name]),
        )
        for name, (rate, buffer) in viewers.items()
    ]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    after = sntp_anchor(args.ntp_server)
    rate = (after[1] - before[1]) / (after[0] - before[0])

    def true_time(monotonic: float) -> float:
        return before[1] + (monotonic - before[0]) * rate

    receipt: dict = {
        "schemaVersion": 1,
        "probe": "mjpeg-latency",
        "seconds": args.seconds,
        "hostMonotonicRate": round(rate, 5),
        "viewers": {},
    }
    passed = True
    for name, seen in arrivals.items():
        if not seen:
            receipt["viewers"][name] = {"frames": 0}
            passed = False
            continue
        # The first second includes the connection's start-up.
        first = seen[0][1] + 1
        lags = [(true_time(arrival) - stamp) * 1000 for stamp, arrival in seen if arrival >= first]
        view = {
            "frames": len(seen),
            "fps": round(len(seen) / args.seconds, 1),
            "lagMs": {
                "p50": round(percentile(lags, 0.5)),
                "p90": round(percentile(lags, 0.9)),
                "max": round(max(lags)),
                "min": round(min(lags)),
            },
        }
        passed &= view["lagMs"]["p90"] <= args.limit_ms
        receipt["viewers"][name] = view
    receipt["result"] = "passed" if passed else "failed"
    output.write_text(json.dumps(receipt, indent=1) + "\n", encoding="utf-8")
    output.chmod(0o600)
    print(json.dumps(receipt))
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
