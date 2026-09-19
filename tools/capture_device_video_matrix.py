#!/usr/bin/env python3
"""Sweep the reviewed capture resolutions against the requested frame rates.

The operator asked for a verified table of what the HDMI/MJPEG bridge actually
delivers instead of trusting the UI list. This gate applies every reviewed
resolution against the requested FPS values, waits for the engine to settle and
records what the engine and the server really report:

- ``source.resolution`` for the applied mode;
- ``source.desired_fps`` against the requested value;
- ``source.captured_fps`` (the measured capture rate);
- ``stream.queued_fps``;
- the server's ``capturedFps`` read-back.

Every applied value must be read back; a combination the engine cannot hold is
recorded as observed, never assumed. The gate restores the settings it found and
requires the restore to hold. Without ``--confirm`` it only observes.

The operator password arrives on the second stdin line.
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
JSON_HEADERS = {"Content-Type": "application/json"}
FACTS = {
    "engine_online",
    "engine_fps",
    "engine_quality",
    "engine_resolution",
    "engine_captured_fps",
    "engine_queued_fps",
}
# The reviewed capture modes by height, mirrored from
# crates/bli-kvm-core/src/video_settings.rs VIDEO_RESOLUTIONS.
RESOLUTION_MODES = {1080: "1920x1080", 720: "1280x720", 600: "800x600", 480: "640x480"}
REVIEWED_RESOLUTIONS = (1080, 720, 600, 480)
REQUESTED_FPS = (10, 20, 30, 40, 50, 60)
CAPTURED_FPS_TOLERANCE = 2
SETTLE_SECONDS = 12
# ustreamer's --slowdown drops the capture rate while nobody is watching, so the
# sweep holds one client open on the engine socket for the run and the reported
# rate is the delivered one.
ENGINE_SOCKET = "/run/hardened-blikvm-ustreamer/ustreamer.sock"
VIEWER_START = (
    "set +e\n"
    # Each apply restarts ustreamer and drops the client, so the viewer must
    # reconnect for the whole sweep.
    f"setsid sh -c 'while true; do curl -s --max-time 30 --unix-socket {ENGINE_SOCKET} "
    "http://localhost/stream >/dev/null 2>&1; sleep 1; done' "
    "</dev/null >/dev/null 2>&1 &\n"
    "sleep 2\n"
    "echo viewer=started\n"
)
VIEWER_STOP = (
    "set +e\n"
    f"pkill -f 'ustreamer.sock http://localhost/stream' 2>/dev/null\n"
    "echo viewer=stopped\n"
)


class MatrixError(Exception):
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
    initialResolution: str
    reviewedResolutions: list[int]
    requestedFps: list[int]
    rows: list[dict[str, Any]]
    viewerHeld: bool
    restoredFps: int
    restoredResolution: str
    failureStage: str
    failureType: str
    mutationsPerformed: bool
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
    _ = parser.add_argument(
        "--confirm",
        action="store_true",
        help="run the sweep and restore; without it the gate observes",
    )
    _ = parser.add_argument("--timeout-seconds", type=int, default=60)
    namespace = parser.parse_args()
    if not 1 <= cast(int, namespace.timeout_seconds) <= 300:
        raise SystemExit("--timeout-seconds is out of range")
    return Options(
        endpoint=cast(str, namespace.endpoint),
        username=cast(str, namespace.username),
        known_hosts=cast(str, namespace.known_hosts),
        output=cast(str, namespace.output),
        base_url=cast(str, namespace.base_url),
        api_username=cast(str, namespace.api_username),
        ca_file=cast(str | None, namespace.ca_file),
        tls_server_name=cast(str | None, namespace.tls_server_name),
        confirm=bool(namespace.confirm),
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
    """Read the engine's own state over its unix socket, including queued fps."""
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
stream = result.get("stream", {})
resolution = source.get("resolution") or {}
print("engine_online=%s" % ("yes" if source.get("online") else "no"))
print("engine_fps=%s" % int(source.get("desired_fps", 0)))
print("engine_quality=%s" % int(encoder.get("quality", 0)))
print("engine_resolution=%sx%s" % (int(resolution.get("width", 0)), int(resolution.get("height", 0))))
print("engine_captured_fps=%s" % int(source.get("captured_fps", 0)))
print("engine_queued_fps=%s" % int(stream.get("queued_fps", 0)))
'
"""
    )


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


def resolution_height(mode: str) -> int:
    parts = mode.split("x")
    if len(parts) != 2:
        return 0
    try:
        return int(parts[1])
    except ValueError:
        return 0


class BoardApi:
    csrf: str | None

    def __init__(self, options: Options, password: str) -> None:
        self.csrf = None
        self.base = options.base_url.rstrip("/")
        parsed = urllib.parse.urlparse(self.base)
        if parsed.scheme not in ("http", "https") or not parsed.hostname:
            raise MatrixError("api", "base_url_invalid")
        if parsed.scheme == "http" and parsed.hostname not in ("127.0.0.1", "localhost"):
            raise MatrixError("api", "insecure_base_url")
        self.opener = upload.build_opener(options.ca_file, options.tls_server_name)
        login = self._request(
            "POST",
            "/api/auth/login",
            {"username": options.api_username, "password": password},
            csrf=None,
        )
        if login.get("code") != 0:
            raise MatrixError("api", "login_failed")
        token = (cast(Json, login.get("data") or {})).get("csrfToken")
        if not isinstance(token, str) or not token:
            raise MatrixError("api", "csrf_missing")
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
            raise MatrixError("api", "read_back_failed")
        return cast(Json, body.get("data") or {})

    def settings(self) -> tuple[int, int]:
        data = self.screen_state()
        return int(data.get("fps", 0)), int(data.get("quality", 0))

    def captured_fps(self) -> int | None:
        value = self.screen_state().get("capturedFps")
        return None if value is None else int(value)

    def apply(self, kind: str, value: int) -> Json:
        body = self._request("POST", "/api/vm/screen", {"type": kind, "value": value})
        if body.get("code") != 0:
            raise MatrixError("api", f"apply_failed_{body.get('code')}")
        return cast(Json, body.get("data") or {})


def execute(options: Options) -> int:
    output = prototype.validate_output_path(options.output)
    secret, password = read_credentials(dispatch=options.confirm)
    receipt: Receipt = {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-video-matrix-gate",
        "result": "observed",
        "mode": "apply" if options.confirm else "plan",
        "initialFps": 0,
        "initialResolution": "",
        "reviewedResolutions": list(REVIEWED_RESOLUTIONS),
        "requestedFps": list(REQUESTED_FPS),
        "rows": [],
        "viewerHeld": False,
        "restoredFps": 0,
        "restoredResolution": "",
        "failureStage": "",
        "failureType": "",
        "mutationsPerformed": False,
        "persistentChangeScope": "none",
    }
    password_holder = password
    password = ""
    stage = "read"
    api: BoardApi | None = None
    initial_fps = 0
    initial_height = 0
    before: dict[str, str] = {}

    def engine() -> dict[str, str]:
        facts = require_ssh(
            options, secret, engine_script(), min(options.timeout_seconds, 30)
        )
        if not FACTS.issubset(facts):
            raise MatrixError(stage, "engine_state_unavailable")
        return facts

    def restore() -> None:
        assert api is not None
        if initial_height in RESOLUTION_MODES:
            api.apply("resolution", initial_height)
        if initial_fps > 0:
            api.apply("fps", initial_fps)
        settle = 0
        final_facts: dict[str, str] = {}
        while settle < SETTLE_SECONDS:
            final_facts = engine()
            if (
                int(final_facts["engine_fps"]) == initial_fps
                and final_facts["engine_resolution"] == before["engine_resolution"]
            ):
                break
            time.sleep(1.0)
            settle += 1
        receipt["restoredFps"] = int(final_facts.get("engine_fps", 0))
        receipt["restoredResolution"] = final_facts.get("engine_resolution", "")

    try:
        api = BoardApi(options, password_holder)
        password_holder = ""
        initial_fps, _initial_quality = api.settings()
        receipt["initialFps"] = initial_fps
        before = engine()
        if before["engine_online"] != "yes":
            raise MatrixError(stage, "engine_offline")
        receipt["initialResolution"] = before["engine_resolution"]
        initial_height = resolution_height(before["engine_resolution"])
        if initial_height not in RESOLUTION_MODES:
            raise MatrixError(stage, "initial_resolution_unsupported")

        if not options.confirm:
            write_json_0600_atomic(output, receipt)
            return 0

        stage = "viewer"
        require_ssh(options, secret, VIEWER_START, min(options.timeout_seconds, 30))
        receipt["viewerHeld"] = True

        stage = "sweep"
        for height in REVIEWED_RESOLUTIONS:
            expected_mode = RESOLUTION_MODES[height]
            applied = api.apply("resolution", height)
            if applied.get("applied") is not True:
                raise MatrixError(stage, f"resolution_{height}_rejected")
            for fps in REQUESTED_FPS:
                applied_fps = api.apply("fps", fps)
                if applied_fps.get("applied") is not True:
                    raise MatrixError(stage, f"fps_{fps}_rejected")
                settle = 0
                after: dict[str, str] = {}
                server_captured: int | None = None
                read_back_fps = 0
                engine_captured = 0
                captured_matches = False
                while settle < SETTLE_SECONDS:
                    read_back_fps, _quality = api.settings()
                    server_captured = api.captured_fps()
                    after = engine()
                    engine_captured = int(after.get("engine_captured_fps", 0))
                    captured_matches = (
                        server_captured is not None
                        and abs(server_captured - engine_captured) <= CAPTURED_FPS_TOLERANCE
                    )
                    if (
                        int(after["engine_fps"]) == fps
                        and after["engine_resolution"] == expected_mode
                        # The engine samples captured_fps over its own window and
                        # reports a low value right after a restart, so wait for
                        # the measured read-back to agree before judging it.
                        and captured_matches
                    ):
                        break
                    time.sleep(1.0)
                    settle += 1
                row: dict[str, Any] = {
                    "resolutionHeight": height,
                    "requestedFps": fps,
                    "apiReadBackFps": read_back_fps,
                    "engineDesiredFps": int(after.get("engine_fps", 0)),
                    "engineResolution": after.get("engine_resolution", ""),
                    "engineCapturedFps": engine_captured,
                    "engineQueuedFps": int(after.get("engine_queued_fps", 0)),
                    "serverCapturedFps": -1 if server_captured is None else server_captured,
                    "capturedMatchesServer": captured_matches,
                }
                receipt["rows"].append(row)
                receipt["mutationsPerformed"] = True
                receipt["persistentChangeScope"] = (
                    "streamer environment file and unit restarts, restored after the run"
                )
                if int(after.get("engine_fps", 0)) != fps:
                    raise MatrixError(stage, f"engine_fps_mismatch_{fps}")
                if after.get("engine_resolution", "") != expected_mode:
                    raise MatrixError(stage, f"engine_resolution_mismatch_{height}")
                if not captured_matches:
                    raise MatrixError(stage, f"server_captured_mismatch_{height}_{fps}")

        stage = "restore"
        restore()
        require_ssh(options, secret, VIEWER_STOP, min(options.timeout_seconds, 30))
        receipt["viewerHeld"] = False
        if receipt["restoredFps"] != initial_fps:
            raise MatrixError(stage, "fps_restore_mismatch")
        if receipt["restoredResolution"] != before["engine_resolution"]:
            raise MatrixError(stage, "resolution_restore_mismatch")
    except MatrixError as error:
        if receipt["mutationsPerformed"] and api is not None:
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
