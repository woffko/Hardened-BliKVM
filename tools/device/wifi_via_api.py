#!/usr/bin/env python3
"""Exercise the board's Wi-Fi through the device API (docs/design/wifi-v1.md).

Steps run in order:

- ``status``: the Wi-Fi state and the interface owning the default route.
- ``scan``: a new scan; networks in range are recorded as counts, signal and
  security only, never by name.
- ``radio-cycle``: the radio off (the scan must come back empty) and on again.
- ``wrong-password:SSID``: joining with a password that cannot be right must
  fail and leave no saved profile for SSID.
- ``join:SSID``: joins with the Wi-Fi password, then checks the state, the
  saved profile and that Ethernet keeps the default route.
- ``autoconnect-off:SSID`` / ``autoconnect-on:SSID`` / ``forget:SSID`` /
  ``disconnect``: act on the saved profile or the device.

The GUI password is read from stdin (via project-memory-exec). For ``join`` the
stdin carries project-memory-exec's two-secret envelope with roles ``gui`` and
``wifi``. Secrets are never printed or recorded. A receipt is written under
evidence/private.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import http.cookiejar
import json
import ssl
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlparse

# Never a valid WPA passphrase for a real network: it is refused by the owner
# only after NetworkManager has tried it.
WRONG_PASSWORD = "not-the-password-0"


def read_secrets() -> tuple[str, str]:
    raw = sys.stdin.buffer.read(65537)
    try:
        envelope = json.loads(raw.decode("ascii"))
    except (UnicodeDecodeError, json.JSONDecodeError):
        envelope = None
    if isinstance(envelope, dict) and envelope.get("schemaVersion") == 1:
        secrets = envelope.get("secrets") or {}
        decode = lambda role: base64.b64decode(secrets.get(role, "")).decode("utf-8")
        return decode("gui").rstrip("\r\n"), decode("wifi").rstrip("\r\n")
    return raw.decode("utf-8").rstrip("\r\n"), ""


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", required=True)
    parser.add_argument("--username", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("steps", nargs="+")
    args = parser.parse_args()
    output = Path(args.output).resolve()
    if output.parent != Path("evidence/private").resolve() or output.exists():
        raise SystemExit("output must be a new file under evidence/private")
    url = urlparse(args.base_url)
    if url.scheme != "https" or url.path not in ("", "/"):
        raise SystemExit("base URL must be https://host/")
    gui_password, wifi_password = read_secrets()
    if not 8 <= len(gui_password) <= 256:
        raise SystemExit("the GUI password is missing")

    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    opener = urllib.request.build_opener(
        urllib.request.HTTPSHandler(context=context),
        urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
    )
    origin = f"https://{url.netloc}"
    csrf = ""

    def call(method: str, route: str, body: dict | None = None, timeout: int = 60) -> tuple[int, dict, int]:
        data = None if body is None else json.dumps(body).encode()
        request = urllib.request.Request(f"{origin}{route}", data=data, method=method)
        request.add_header("Origin", origin)
        if csrf:
            request.add_header("X-CSRF-Token", csrf)
        if data is not None:
            request.add_header("Content-Type", "application/json")
        started = time.monotonic()
        try:
            with opener.open(request, timeout=timeout) as response:
                status, raw = response.status, response.read()
        except urllib.error.HTTPError as error:
            status, raw = error.code, error.read()
        elapsed = int((time.monotonic() - started) * 1000)
        try:
            parsed = json.loads(raw)
        except json.JSONDecodeError:
            parsed = {}
        return status, parsed if isinstance(parsed, dict) else {}, elapsed

    receipt: dict[str, object] = {"schemaVersion": 1, "probe": "wifi-via-api", "steps": []}
    steps: list[dict[str, object]] = receipt["steps"]  # type: ignore[assignment]

    def record(name: str, **values: object) -> None:
        entry = {"step": name, **values}
        steps.append(entry)
        print(json.dumps(entry)[:400])

    def data(route: str) -> dict:
        status, body, _ = call("GET", route)
        if status != 200 or body.get("code") != 0:
            raise RuntimeError(f"{route} answered {status} {body.get('msg')}")
        return body.get("data") or {}

    def state() -> dict:
        wifi = data("/api/network/wifi")
        route = data("/api/network/interfaces").get("defaultRoute")
        # The state names the operator's network only; the address is kept.
        return {
            "available": wifi.get("available"),
            "radio": wifi.get("radio"),
            "connected": wifi.get("connected"),
            "state": wifi.get("state"),
            "signal": wifi.get("signal"),
            "security": wifi.get("security"),
            "hasAddress": bool(wifi.get("address")),
            "defaultRoute": route,
            "ssid": wifi.get("ssid"),
        }

    def scan(rescan: bool) -> list[dict]:
        return data(f"/api/network/wifi/scan?rescan={'true' if rescan else 'false'}").get("networks", [])

    def fresh_scan() -> list[dict]:
        scan(True)
        time.sleep(5)
        return scan(False)

    def profiles(ssid: str) -> list[dict]:
        return [profile for profile in data("/api/network/wifi/saved").get("profiles", []) if profile.get("ssid") == ssid]

    def act(route: str, body: dict | None = None) -> tuple[bool, object, int]:
        status, answer, elapsed = call("POST", route, body)
        result = answer.get("data") or {}
        return status == 200 and result.get("applied") is True, result.get("rejection") or answer.get("msg"), elapsed

    status, body, _ = call("POST", "/api/auth/login", {"username": args.username, "password": gui_password}, 15)
    gui_password = ""
    csrf = (body.get("data") or {}).get("csrfToken", "")
    if status != 200 or not csrf:
        raise SystemExit("login failed")

    passed = True
    try:
        for step in args.steps:
            name, _, ssid = step.partition(":")
            if name == "status":
                record("status", **state())
            elif name == "scan":
                networks = fresh_scan()
                record(
                    "scan",
                    count=len(networks),
                    security=sorted({network.get("security") for network in networks}),
                    strongest=max((network.get("signal", 0) for network in networks), default=0),
                    nameDigests=sorted(hashlib.sha256(network["ssid"].encode()).hexdigest()[:8] for network in networks),
                )
                passed &= len(networks) > 0
            elif name == "radio-cycle":
                off, off_reason, off_ms = act("/api/network/wifi/radio", {"enabled": False})
                time.sleep(3)
                while_off = state()
                in_range_off = len(scan(False))
                on, on_reason, on_ms = act("/api/network/wifi/radio", {"enabled": True})
                time.sleep(8)
                while_on = state()
                in_range_on = len(fresh_scan())
                record(
                    "radio_cycle",
                    off=off, offRejection=off_reason, offMs=off_ms, radioWhileOff=while_off["radio"],
                    networksWhileOff=in_range_off, on=on, onRejection=on_reason, onMs=on_ms,
                    radioWhileOn=while_on["radio"], networksWhileOn=in_range_on,
                )
                passed &= off and on and while_off["radio"] is False and in_range_off == 0
                passed &= while_on["radio"] is True and in_range_on > 0
            elif name == "wrong-password":
                before = len(profiles(ssid))
                joined, reason, elapsed = act(
                    "/api/network/wifi/connect", {"ssid": ssid, "password": WRONG_PASSWORD, "hidden": False}
                )
                after = len(profiles(ssid))
                record("wrong_password", ssid=ssid, joined=joined, rejection=reason, ms=elapsed,
                       profilesBefore=before, profilesAfter=after, state=state())
                passed &= not joined and after == before
            elif name == "join":
                if not wifi_password:
                    raise RuntimeError("join needs the Wi-Fi password in the envelope")
                joined, reason, elapsed = act(
                    "/api/network/wifi/connect", {"ssid": ssid, "password": wifi_password, "hidden": False}
                )
                wifi_password = ""
                observed = state()
                saved = profiles(ssid)
                record("join", ssid=ssid, joined=joined, rejection=reason, ms=elapsed, state=observed,
                       savedProfiles=len(saved), savedActive=[profile.get("active") for profile in saved])
                passed &= joined and observed["connected"] and observed["ssid"] == ssid
                passed &= len(saved) == 1 and observed["defaultRoute"] != "wlan0"
            elif name in ("autoconnect-off", "autoconnect-on", "forget"):
                saved = profiles(ssid)
                if len(saved) != 1:
                    raise RuntimeError(f"{name} needs exactly one saved profile for the network")
                action = {"autoconnect-off": "autoconnect_off", "autoconnect-on": "autoconnect_on", "forget": "forget"}[name]
                done, reason, elapsed = act("/api/network/wifi/saved", {"id": saved[0]["id"], "action": action})
                after = profiles(ssid)
                record(name.replace("-", "_"), ssid=ssid, applied=done, rejection=reason, ms=elapsed,
                       profilesAfter=len(after), autoconnect=[profile.get("autoconnect") for profile in after],
                       state=state())
                if name == "forget":
                    passed &= done and not after
                else:
                    passed &= done and after[0].get("autoconnect") is (name == "autoconnect-on")
            elif name == "disconnect":
                done, reason, elapsed = act("/api/network/wifi/disconnect")
                observed = state()
                record("disconnect", applied=done, rejection=reason, ms=elapsed, state=observed)
                passed &= done and not observed["connected"]
            else:
                raise SystemExit(f"unknown step {step}")
    except (RuntimeError, OSError) as error:
        record("error", message=str(error)[:300])
        passed = False
    receipt["result"] = "passed" if passed else "failed"
    output.write_text(json.dumps(receipt, indent=1) + "\n", encoding="utf-8")
    output.chmod(0o600)
    print(f"result: {receipt['result']}")
    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
