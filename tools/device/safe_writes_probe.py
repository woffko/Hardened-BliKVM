#!/usr/bin/env python3
"""Exercise the board's settings writes and set each back to its value.

Session lock, mDNS, hostname, the video frame rate, the SSH switch, the
system-log test message and scripts (the switch on, a harmless script
uploaded, run and deleted, the switch back to its value). Nothing here
touches the target machine: no HID report, power line or media command.
The GUI password is read from stdin (via project-memory-exec); a receipt is
written under evidence/private.
"""

from __future__ import annotations

import argparse
import http.cookiejar
import json
import ssl
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path
from urllib.parse import urlparse

SCRIPT_NAME = "hardened-blikvm-probe.sh"
SCRIPT = b"#!/bin/sh\necho hardened-blikvm-probe\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
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
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    )
    csrf = ""

    def call(method: str, route: str, body: object = None, raw: bytes | None = None, kind: str | None = None) -> dict:
        data = raw if raw is not None else (None if body is None else json.dumps(body).encode())
        request = urllib.request.Request(f"{origin}{route}", data=data, method=method)
        request.add_header("Origin", origin)
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        if kind or (data is not None and raw is None):
            request.add_header("Content-Type", kind or "application/json")
        try:
            with opener.open(request, timeout=60) as response:
                return json.loads(response.read() or b"{}")
        except urllib.error.HTTPError as error:
            try:
                return json.loads(error.read() or b"{}") | {"http": error.code}
            except json.JSONDecodeError:
                return {"code": error.code}

    login = call("POST", "/api/auth/login", {"username": args.username, "password": password})
    password = ""
    csrf = (login.get("data") or {}).get("csrfToken", "")
    if not csrf:
        raise SystemExit("login failed")
    data = lambda answer: answer.get("data") or {}
    steps: list[dict] = []
    passed = True

    def record(name: str, ok: bool, **values: object) -> None:
        nonlocal passed
        passed &= ok
        entry = {"step": name, "ok": ok, **values}
        steps.append(entry)
        print(json.dumps(entry)[:300])

    # Session lock: the same duration back.
    duration = data(call("GET", "/api/vm/session-lock")).get("duration")
    answer = call("POST", "/api/vm/session-lock", {"duration": duration})
    after = data(call("GET", "/api/vm/session-lock")).get("duration")
    record("session_lock", answer.get("code") == 0 and after == duration, duration=duration)

    # mDNS: switched the other way and back.
    enabled = bool(data(call("GET", "/api/vm/mdns")).get("enabled"))
    first = call("POST", "/api/vm/mdns/" + ("disable" if enabled else "enable"))
    flipped = bool(data(call("GET", "/api/vm/mdns")).get("enabled"))
    second = call("POST", "/api/vm/mdns/" + ("enable" if enabled else "disable"))
    restored = bool(data(call("GET", "/api/vm/mdns")).get("enabled"))
    record(
        "mdns",
        first.get("code") == 0 and second.get("code") == 0 and flipped != enabled and restored == enabled,
        enabled=enabled,
    )

    # Hostname: the same name again.
    hostname = data(call("GET", "/api/vm/hostname")).get("hostname")
    answer = call("POST", "/api/vm/hostname", {"hostname": hostname})
    after = data(call("GET", "/api/vm/hostname")).get("hostname")
    record("hostname", answer.get("code") == 0 and after == hostname, hostname=hostname)

    # Video frame rate: the same value again.
    fps = data(call("GET", "/api/vm/screen")).get("fps")
    answer = call("POST", "/api/vm/screen", {"type": "fps", "value": fps})
    after = data(call("GET", "/api/vm/screen")).get("fps")
    record("video_fps", answer.get("code") == 0 and data(answer).get("applied") is not False and after == fps, fps=fps)

    # System log: the test message.
    answer = call("POST", "/api/system-log/test")
    record("system_log_test", answer.get("code") == 0)

    # SSH: off and back on (or on and back off).
    ssh = bool(data(call("GET", "/api/vm/ssh")).get("enabled"))
    first = call("POST", "/api/vm/ssh/" + ("disable" if ssh else "enable"))
    time.sleep(2)
    flipped = bool(data(call("GET", "/api/vm/ssh")).get("enabled"))
    second = call("POST", "/api/vm/ssh/" + ("enable" if ssh else "disable"))
    time.sleep(2)
    restored = bool(data(call("GET", "/api/vm/ssh")).get("enabled"))
    record(
        "ssh_switch",
        first.get("code") == 0 and second.get("code") == 0 and flipped != ssh and restored == ssh,
        enabled=ssh,
    )

    # Scripts: switched on for the check, a harmless script, back as it was.
    scripts_on = bool(data(call("GET", "/api/admin/features")).get("scripts"))
    call("POST", "/api/admin/features", {"feature": "scripts", "enabled": True})
    boundary = "----hardened-blikvm-" + uuid.uuid4().hex
    body = (
        f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{SCRIPT_NAME}\"\r\n"
        "Content-Type: application/octet-stream\r\n\r\n"
    ).encode() + SCRIPT + f"\r\n--{boundary}--\r\n".encode()
    uploaded = call("POST", "/api/vm/script/upload", raw=body, kind=f"multipart/form-data; boundary={boundary}")
    listed = SCRIPT_NAME in (data(call("GET", "/api/vm/script")).get("files") or [])
    ran = call("POST", "/api/vm/script/run", {"name": SCRIPT_NAME, "type": "foreground"})
    output_ok = "hardened-blikvm-probe" in str(data(ran).get("log", ""))
    deleted = call("DELETE", "/api/vm/script", {"name": SCRIPT_NAME})
    gone = SCRIPT_NAME not in (data(call("GET", "/api/vm/script")).get("files") or [])
    call("POST", "/api/admin/features", {"feature": "scripts", "enabled": scripts_on})
    back = bool(data(call("GET", "/api/admin/features")).get("scripts")) == scripts_on
    record(
        "scripts",
        uploaded.get("code") == 0 and listed and output_ok and deleted.get("code") == 0 and gone and back,
        uploaded=uploaded.get("code"),
        listed=listed,
        ran=output_ok,
        deleted=deleted.get("code"),
        switchRestored=back,
    )

    receipt = {"schemaVersion": 1, "probe": "safe-writes", "steps": steps, "result": "passed" if passed else "failed"}
    output.write_text(json.dumps(receipt, indent=1) + "\n", encoding="utf-8")
    output.chmod(0o600)
    print(f"result: {receipt['result']}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
