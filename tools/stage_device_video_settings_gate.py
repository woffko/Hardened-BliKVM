#!/usr/bin/env python3
"""Run the video-settings gate on a test H616 board.

The gate proves the reference screen settings really reach the video engine:

- it reads the current settings through the board API and the engine's own
  ``/state`` before touching anything;
- with ``--apply-fps`` and ``--apply-quality`` it sends the reviewed values
  through ``POST /api/vm/screen``, then requires both the API read-back and the
  engine state to agree with them;
- with ``--apply-resolution`` it also sets one reviewed capture mode and
  requires the engine's ``source.resolution`` to follow it;
- it reads the engine's ``source.captured_fps`` and the server's ``capturedFps``
  read-back and requires them to agree, so the honest delivered-rate field is
  proven against the engine;
- it restores the values it found and requires the restore to hold, so a run
  leaves the stream as it found it.

The operator password arrives on the second stdin line. Without ``--confirm``
the gate only observes.
"""

from __future__ import annotations

import argparse
from dataclasses import dataclass
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any, TypedDict, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import stage_device_prototype as prototype
from tools import upload_media_image as upload
from tools.capture_pikvm_baseline import write_json_0600_atomic


Json = dict[str, Any]

ENGINE_SOCKET = "/run/hardened-blikvm-ustreamer/ustreamer.sock"
FIRST_LINE_MAX = 512
HEX_SHA256 = re.compile(r"^[0-9a-f]{64}$")
JSON_HEADERS = {"Content-Type": "application/json"}
FACTS = {
    "engine_fps",
    "engine_quality",
    "engine_online",
    "engine_resolution",
    "engine_captured_fps",
}
# captured_fps is a measured rate (the engine samples it over its own window),
# so the server read-back is allowed to differ from the engine's own report by
# a small sampling tolerance instead of being required to match it exactly.
CAPTURED_FPS_TOLERANCE = 2
# The reviewed capture modes, mirrored from
# crates/bli-kvm-core/src/video_settings.rs VIDEO_RESOLUTIONS.
RESOLUTION_MODES = {1080: "1920x1080", 720: "1280x720", 600: "800x600", 480: "640x480"}


class GateError(Exception):
    def __init__(self, stage: str, failure_type: str) -> None:
        super().__init__(stage)
        self.stage = stage
        self.failure_type = failure_type


class Receipt(TypedDict):
    schemaVersion: int
    probe: str
    result: str
    mode: str
    initialFps: int
    initialQuality: int
    targetFps: int
    targetQuality: int
    apiReadBackFps: int
    apiReadBackQuality: int
    engineFps: int
    engineQuality: int
    initialResolution: str
    targetResolution: int
    engineResolution: str
    engineCapturedFps: int
    serverCapturedFps: int
    restoredFps: int
    restoredQuality: int
    engineRestoredFps: int
    engineRestoredQuality: int
    restoredResolution: str
    engineRestoredResolution: str
    engineRestoredCapturedFps: int
    failureStage: str
    failureType: str
    mutationsPerformed: bool
    physicalActionsInvoked: bool
    persistentChangeScope: str


@dataclass(frozen=True)
class Options:
    endpoint: str
    username: str
    known_hosts: str
    output: str
    base_url: str
    api_username: str
    ca_file: str | None
    tls_server_name: str | None
    apply_fps: int
    apply_quality: int
    apply_resolution: int
    confirm: bool
    timeout_seconds: int


def parse_args() -> Options:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--endpoint", required=True)
    _ = parser.add_argument("--username", required=True)
    _ = parser.add_argument("--known-hosts", required=True)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--base-url", required=True)
    _ = parser.add_argument("--api-username", required=True)
    _ = parser.add_argument("--ca-file")
    _ = parser.add_argument("--tls-server-name")
    _ = parser.add_argument("--apply-fps", type=int, default=24)
    _ = parser.add_argument("--apply-quality", type=int, default=100)
    _ = parser.add_argument(
        "--apply-resolution",
        type=int,
        default=0,
        help="apply one reviewed capture mode by its height (default: skip)",
    )
    _ = parser.add_argument(
        "--confirm",
        action="store_true",
        help="apply and restore the reviewed values; without it the gate observes",
    )
    _ = parser.add_argument("--timeout-seconds", type=int, default=60)
    namespace = parser.parse_args()
    if not 1 <= cast(int, namespace.apply_fps) <= 60:
        raise SystemExit("--apply-fps must be between 1 and 60")
    if not 1 <= cast(int, namespace.apply_quality) <= 100:
        raise SystemExit("--apply-quality must be between 1 and 100")
    if cast(int, namespace.apply_resolution) not in (0, *RESOLUTION_MODES):
        raise SystemExit("--apply-resolution must be a reviewed capture-mode height or 0")
    if not 1 <= cast(int, namespace.timeout_seconds) <= 300:
        raise SystemExit("--timeout-seconds is out of range")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        base_url=cast(str, namespace.base_url),
        api_username=cast(str, namespace.api_username),
        ca_file=cast("str | None", namespace.ca_file),
        tls_server_name=cast("str | None", namespace.tls_server_name),
        apply_fps=cast(int, namespace.apply_fps),
        apply_quality=cast(int, namespace.apply_quality),
        apply_resolution=cast(int, namespace.apply_resolution),
        confirm=cast(bool, namespace.confirm),
        timeout_seconds=cast(int, namespace.timeout_seconds),
    )


def read_credentials(*, dispatch: bool) -> tuple[bytearray, str]:
    stream = getattr(sys.stdin, "buffer", sys.stdin)

    def read_line(limit: int) -> bytes:
        raw = stream.readline()
        if isinstance(raw, str):
            raw = raw.encode("utf-8", "replace")
        if not raw or len(raw) > limit:
            return b""
        return raw.rstrip(b"\r\n")

    secret = bytearray(read_line(FIRST_LINE_MAX))
    if not secret:
        raise SystemExit("the SSH secret is missing on stdin")
    password = ""
    if dispatch:
        password = read_line(FIRST_LINE_MAX).decode("utf-8", "replace")
        if len(password) < 8:
            raise SystemExit("the operator password is missing on stdin")
    return secret, password


def engine_script() -> str:
    """Read the engine's own state over its unix socket."""
    return (
        "set +e\n"
        f"""curl -s --max-time 5 --unix-socket {ENGINE_SOCKET} http://localhost/state | """
        """python3 -c '
import json, sys
try:
    result = json.load(sys.stdin)["result"]
except Exception:
    print("engine_online=no")
    raise SystemExit(0)
source = result.get("source", {})
encoder = result.get("encoder", {})
resolution = source.get("resolution") or {}
print("engine_online=%s" % ("yes" if source.get("online") else "no"))
print("engine_fps=%s" % int(source.get("desired_fps", 0)))
print("engine_quality=%s" % int(encoder.get("quality", 0)))
print("engine_resolution=%sx%s" % (int(resolution.get("width", 0)), int(resolution.get("height", 0))))
print("engine_captured_fps=%s" % int(source.get("captured_fps", 0)))
'
"""
    )


def resolution_height(mode: str) -> int:
    """Height of a ``WxH`` engine mode, or 0 when it is not parseable."""
    parts = mode.split("x")
    if len(parts) != 2:
        return 0
    try:
        return int(parts[1])
    except ValueError:
        return 0


def parse_facts(output: str) -> dict[str, str]:
    facts: dict[str, str] = {}
    for line in output.splitlines():
        if "=" not in line or line.startswith(" "):
            continue
        key, value = line.split("=", 1)
        if re.fullmatch(r"[A-Za-z0-9_]+", key):
            facts[key] = value
    return facts


def require_ssh(
    options: Options, secret: bytearray, script: str, timeout: int
) -> dict[str, str]:
    host, ssh_port = prototype.parse_endpoint(options.endpoint)
    target = prototype.RemoteTarget(
        host, ssh_port, options.username, Path(options.known_hosts)
    )
    result = prototype.run_ssh(target, secret, script, timeout=timeout)
    stdout = (result.stdout or b"").decode("utf-8", "replace")
    print(stdout)
    return parse_facts(stdout)


class BoardApi:
    csrf: str | None

    def __init__(self, options: Options, password: str) -> None:
        self.csrf = None
        self.base = options.base_url.rstrip("/")
        parsed = urllib.parse.urlparse(self.base)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise GateError("api", "base_url_invalid")
        if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
            raise GateError("api", "insecure_base_url")
        self.opener = upload.build_opener(options.ca_file, options.tls_server_name)
        login = self._request(
            "POST",
            "/api/auth/login",
            {"username": options.api_username, "password": password},
            csrf=None,
        )
        if login.get("code") != 0:
            raise GateError("api", "login_failed")
        token = (cast(Json, login.get("data") or {})).get("csrfToken")
        if not isinstance(token, str) or not token:
            raise GateError("api", "csrf_missing")
        self.csrf = token

    def _request(
        self, method: str, route: str, payload: Json | None, csrf: str | None = None
    ) -> Json:
        token = self.csrf if csrf is None else csrf
        headers = JSON_HEADERS | {"Origin": self.base}
        if token:
            headers["x-csrf-token"] = token
        request = urllib.request.Request(
            urllib.parse.urljoin(self.base, route),
            data=None if payload is None else json.dumps(payload).encode(),
            headers=headers,
            method=method,
        )
        try:
            with self.opener.open(request, timeout=30) as response:
                return cast(Json, json.loads(response.read().decode() or "{}"))
        except urllib.error.HTTPError as error:
            return cast(Json, json.loads(error.read().decode() or "{}"))

    def screen_state(self) -> Json:
        body = self._request("GET", "/api/vm/screen", None)
        if body.get("code") != 0:
            raise GateError("api", "read_back_failed")
        return cast(Json, body.get("data") or {})

    def settings(self) -> tuple[int, int]:
        data = self.screen_state()
        return int(data.get("fps", 0)), int(data.get("quality", 0))

    def captured_fps(self) -> int | None:
        value = self.screen_state().get("capturedFps")
        return None if value is None else int(value)

    def apply(self, kind: str, value: int) -> dict[str, Any]:
        body = self._request(
            "POST", "/api/vm/screen", {"type": kind, "value": value}
        )
        if body.get("code") != 0:
            raise GateError("api", f"apply_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-video-settings-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "initialFps": 0,
        "initialQuality": 0,
        "targetFps": options.apply_fps,
        "targetQuality": options.apply_quality,
        "apiReadBackFps": 0,
        "apiReadBackQuality": 0,
        "engineFps": 0,
        "engineQuality": 0,
        "initialResolution": "",
        "targetResolution": options.apply_resolution,
        "engineResolution": "",
        "engineCapturedFps": 0,
        "serverCapturedFps": 0,
        "restoredFps": 0,
        "restoredQuality": 0,
        "engineRestoredFps": 0,
        "engineRestoredQuality": 0,
        "restoredResolution": "",
        "engineRestoredResolution": "",
        "engineRestoredCapturedFps": 0,
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "physicalActionsInvoked": False,
        "persistentChangeScope": "none",
    }
    password_holder = password
    password = ""
    stage = "read"

    def engine() -> dict[str, str]:
        facts = require_ssh(
            options, secret, engine_script(), min(options.timeout_seconds, 30)
        )
        if not FACTS.issubset(facts):
            raise GateError(stage, "engine_state_unavailable")
        return facts

    def restore() -> None:
        final_fps = receipt["apiReadBackFps"]
        final_quality = receipt["apiReadBackQuality"]
        restored_engine: dict[str, str] = engine()
        if options.apply_resolution:
            restored = api.apply("resolution", initial_height)
            if restored.get("applied") is not True:
                raise GateError("restore", "resolution_restore_rejected")
        if initial_fps > 0:
            restored = api.apply("fps", initial_fps)
            if restored.get("applied") is not True:
                raise GateError("restore", "fps_restore_rejected")
        if initial_quality > 0:
            restored = api.apply("quality", initial_quality)
            if restored.get("applied") is not True:
                raise GateError("restore", "quality_restore_rejected")
        settle = 0
        while settle < 10:
            final_fps, final_quality = api.settings()
            restored_engine = engine()
            if (
                final_fps == initial_fps
                and final_quality == initial_quality
                and int(restored_engine["engine_fps"]) == initial_fps
                and restored_engine["engine_resolution"] == before["engine_resolution"]
            ):
                break
            time.sleep(1.0)
            settle += 1
        receipt["restoredFps"] = final_fps
        receipt["restoredQuality"] = final_quality
        receipt["engineRestoredFps"] = int(restored_engine["engine_fps"])
        receipt["engineRestoredQuality"] = int(restored_engine["engine_quality"])
        receipt["restoredResolution"] = RESOLUTION_MODES[initial_height]
        receipt["engineRestoredResolution"] = restored_engine["engine_resolution"]
        receipt["engineRestoredCapturedFps"] = int(restored_engine["engine_captured_fps"])
        if final_fps != initial_fps or final_quality != initial_quality:
            raise GateError("restore", "api_restore_mismatch")
        if int(restored_engine["engine_fps"]) != initial_fps:
            raise GateError("restore", "engine_restore_mismatch")
        if restored_engine["engine_resolution"] != before["engine_resolution"]:
            raise GateError("restore", "engine_resolution_restore_mismatch")

    try:
        api = BoardApi(options, password_holder)
        password_holder = ""
        initial_fps, initial_quality = api.settings()
        receipt["initialFps"] = initial_fps
        receipt["initialQuality"] = initial_quality
        before = engine()
        if before["engine_online"] != "yes":
            raise GateError(stage, "engine_offline")
        receipt["engineFps"] = int(before["engine_fps"])
        receipt["engineQuality"] = int(before["engine_quality"])
        receipt["initialResolution"] = before["engine_resolution"]
        receipt["engineCapturedFps"] = int(before["engine_captured_fps"])
        initial_height = resolution_height(before["engine_resolution"])
        if initial_height not in RESOLUTION_MODES:
            raise GateError(stage, "initial_resolution_unsupported")

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "apply"
        if options.apply_resolution:
            applied_resolution = api.apply("resolution", options.apply_resolution)
            if applied_resolution.get("applied") is not True:
                raise GateError(
                    stage,
                    "resolution_"
                    + (applied_resolution.get("rejection") or "not_applied"),
                )
        applied_fps = api.apply("fps", options.apply_fps)
        if applied_fps.get("applied") is not True:
            raise GateError(stage, f"fps_{applied_fps.get('rejection') or 'not_applied'}")
        applied_quality = api.apply("quality", options.apply_quality)
        if applied_quality.get("applied") is not True:
            raise GateError(
                stage, f"quality_{applied_quality.get('rejection') or 'not_applied'}"
            )
        receipt["mutationsPerformed"] = True
        receipt["persistentChangeScope"] = (
            "streamer environment file and one unit restart, restored after the run"
        )
        # The engine restarts before the owner answers, so the read-back is
        # immediate; a short settle keeps a slow re-enumeration honest.
        expected_mode = RESOLUTION_MODES.get(options.apply_resolution, "")
        settle = 0
        read_back_fps = read_back_quality = 0
        server_captured: int | None = None
        after: dict[str, str] = {}
        while settle < 10:
            read_back_fps, read_back_quality = api.settings()
            server_captured = api.captured_fps()
            after = engine()
            if (
                read_back_fps == options.apply_fps
                and int(after["engine_fps"]) == options.apply_fps
                and (not expected_mode or after["engine_resolution"] == expected_mode)
                # The engine samples captured_fps over its own window and the
                # server read is skipped when the engine socket does not answer,
                # so wait for the measured read-back to appear before judging it.
                and server_captured is not None
            ):
                break
            time.sleep(1.0)
            settle += 1
        receipt["apiReadBackFps"] = read_back_fps
        receipt["apiReadBackQuality"] = read_back_quality
        receipt["engineFps"] = int(after["engine_fps"])
        receipt["engineQuality"] = int(after["engine_quality"])
        receipt["engineResolution"] = after["engine_resolution"]
        receipt["engineCapturedFps"] = int(after["engine_captured_fps"])
        receipt["serverCapturedFps"] = 0 if server_captured is None else server_captured
        if read_back_fps != options.apply_fps:
            raise GateError("verify", "api_fps_mismatch")
        if read_back_quality != options.apply_quality:
            raise GateError("verify", "api_quality_mismatch")
        if int(after["engine_fps"]) != options.apply_fps:
            raise GateError("verify", "engine_fps_mismatch")
        if expected_mode and after["engine_resolution"] != expected_mode:
            raise GateError("verify", "engine_resolution_mismatch")
        if server_captured is None:
            raise GateError("verify", "server_captured_missing")
        if abs(server_captured - int(after["engine_captured_fps"])) > CAPTURED_FPS_TOLERANCE:
            raise GateError("verify", "server_captured_mismatch")

        stage = "restore"
        restore()
    except GateError as error:
        if receipt["mutationsPerformed"]:
            try:
                stage = "restore_after_failure"
                restore()
            except Exception:  # noqa: BLE001 - best effort; the failure is primary
                pass
        receipt["result"] = "failed"
        receipt["failureStage"] = error.stage
        receipt["failureType"] = error.failure_type
        write_json_0600_atomic(output, receipt)
        return 1

    receipt["result"] = "passed"
    write_json_0600_atomic(output, receipt)
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError):
        return 2
    except Exception as error:  # noqa: BLE001 - the operator log is the surface
        import traceback

        traceback.print_exc()
        print(f"gate failed before a receipt: {error!r}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
