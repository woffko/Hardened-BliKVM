#!/usr/bin/env python3
"""Login timing and size limits on a board (review R1, R3).

Usage: login_time_probe.py https://BOARD < password-file-or-pipe

The GUI password arrives on standard input (run this file under
project-memory-exec, never a heredoc) and is never printed. The probe times
logins with the right name and password and logins with unknown names; both
must cost the same (one Argon2 verification), within 25 %. It never sends a
wrong password for the real account, so it cannot lock the operator out, and
it checks that oversized login bodies are refused before parsing.
"""
import http.cookiejar
import json
import ssl
import statistics
import sys
import time
import urllib.error
import urllib.request

origin = sys.argv[1].rstrip("/")
password = sys.stdin.read().rstrip("\r\n")
context = ssl.create_default_context()
context.check_hostname = False
context.verify_mode = ssl.CERT_NONE
USER = "admin"


def post(route, body, jar=None, csrf=""):
    data = body if isinstance(body, bytes) else json.dumps(body).encode()
    request = urllib.request.Request(origin + route, data=data, method="POST")
    request.add_header("Origin", origin)
    request.add_header("Content-Type", "application/json")
    if csrf:
        request.add_header("X-CSRF-Token", csrf)
    handlers = [urllib.request.HTTPSHandler(context=context)]
    if jar is not None:
        handlers.append(urllib.request.HTTPCookieProcessor(jar))
    started = time.monotonic()
    try:
        with urllib.request.build_opener(*handlers).open(request, timeout=30) as response:
            status, text = response.status, response.read()
    except urllib.error.HTTPError as error:
        status, text = error.code, error.read()
    elapsed = (time.monotonic() - started) * 1000
    try:
        parsed = json.loads(text or b"{}")
    except ValueError:
        parsed = {}
    return elapsed, status, parsed


def right_login():
    jar = http.cookiejar.CookieJar()
    elapsed, _, body = post("/api/auth/login", {"username": USER, "password": password}, jar)
    csrf = (body.get("data") or {}).get("csrfToken", "")
    if csrf:
        post("/api/auth/logout", {}, jar, csrf)
    return elapsed, bool(csrf)


right, wrong = [], []
for _ in range(4):
    elapsed, ok = right_login()
    if not ok:
        raise SystemExit("login with the stored record failed")
    right.append(elapsed)
for index in range(4):
    elapsed, status, body = post(
        "/api/auth/login", {"username": f"nobody{index}", "password": "not the password"}
    )
    wrong.append(elapsed)
password = ""
big_status = post("/api/auth/login", {"username": "x" * 65536, "password": "p"})[1]
mid_status = post("/api/auth/login", b'{"username":"' + b"x" * 5000 + b'","password":"p"}')[1]
long_name = post("/api/auth/login", {"username": "x" * 200, "password": "p"})[2].get("code")
right_ms, wrong_ms = statistics.median(right), statistics.median(wrong)
ratio = min(right_ms, wrong_ms) / max(right_ms, wrong_ms)
result = {
    "rightNameMedianMs": round(right_ms),
    "unknownNameMedianMs": round(wrong_ms),
    "ratio": round(ratio, 2),
    "hugeBodyStatus": big_status,
    "fiveKiBBodyStatus": mid_status,
    "longNameCode": long_name,
}
result["pass"] = ratio >= 0.75 and big_status == 413 and mid_status == 413 and long_name == -1
print(json.dumps(result, indent=1))
sys.exit(0 if result["pass"] else 1)
