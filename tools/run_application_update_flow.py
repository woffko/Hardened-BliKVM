#!/usr/bin/env python3
"""Run reviewed application phases without model-turn gaps in the guard window."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import secrets
import subprocess
import sys
import time
from typing import cast
from urllib.parse import urlsplit

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import application_update_flow as flow
from tools import stage_installed_application_update as driver
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin


def unique_object(pairs: list[tuple[str, object]]) -> dict[str, object]:
    value: dict[str, object] = {}
    for key, item in pairs:
        driver.require(key not in value, "duplicate phase receipt field")
        value[key] = item
    return value


def read_receipt(path: Path) -> dict[str, object]:
    source = driver.transport.validate_private_file(str(path))
    driver.require(source.stat().st_size <= 131072, "oversize phase receipt")
    value: object = json.loads(source.read_bytes(), object_pairs_hook=unique_object)
    driver.require(isinstance(value, dict), "invalid phase receipt")
    return cast(dict[str, object], value)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=flow.PROFILES, default=flow.PROFILE)
    for name in ("old-package", "new-package", "bundle", "endpoint", "base-url", "username", "output"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args()
    driver.native.select_profile(args.profile)
    host, _ = driver.transport.parse_endpoint(args.endpoint)
    url = urlsplit(cast(str, args.base_url))
    driver.require(url.scheme == "https" and url.hostname == host and not url.username and not url.password
                   and not url.query and not url.fragment and url.path in ("", "/"), "invalid browser endpoint")
    driver.require(re.fullmatch(r"[a-z_][a-z0-9_-]{0,31}", args.username) is not None, "invalid browser username")
    output = driver.transport.validate_output_path(args.output)
    driver.require(not output.exists() and re.fullmatch(r"[a-z][a-z0-9-]{0,80}", output.stem) is not None,
                   "new bounded flow output required")
    run_id = secrets.token_hex(12)
    deadline = time.monotonic() + 450
    checkpoint: Path | None = None
    browser_receipts: dict[str, Path] = {}
    counts: dict[str, int] = {}
    recovering = False

    def destination(phase: str) -> Path:
        counts[phase] = counts.get(phase, 0) + 1
        path = output.with_name(f"{output.stem}-{run_id}-{phase}-{counts[phase]}.json")
        driver.require(not path.exists(), "phase output already exists")
        return path

    with secret_pair_stdin() as pair:
        def child(argv: list[str], role: str, limit: int, *, recovery: bool = False) -> int:
            remaining = deadline - time.monotonic()
            if not recovery:
                driver.require(remaining >= limit, "insufficient guard-window budget for next phase")
            return subprocess.run(argv, input=pair[role], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                  timeout=limit, check=False, close_fds=True).returncode

        def app(action: str) -> dict[str, object]:
            nonlocal checkpoint, recovering
            if action == "recover":
                recovering = True
            target = destination(action)
            argv = [sys.executable, "-B", "tools/stage_installed_application_update.py", action,
                    "--profile", args.profile, "--endpoint", args.endpoint, "--output", str(target)]
            if action == "prepare":
                argv += ["--old-package", args.old_package, "--new-package", args.new_package, "--bundle", args.bundle]
            else:
                driver.require(checkpoint is not None, "missing application checkpoint")
                argv += ["--guard-receipt", str(checkpoint)]
            if action in ("activate", "commit"):
                phase = "rehearsal" if action == "activate" else "confirmed"
                argv += ["--browser-receipt", str(browser_receipts[phase])]
            code = child(argv, "ssh", {"prepare": 240, "activate": 120, "commit": 45,
                                      "cleanup": 60, "recover": 150}[action], recovery=recovering)
            value = read_receipt(target)
            expected_guard = driver.checkpoint(str(checkpoint)) if checkpoint is not None else None
            flow.envelope(value, action, expected_guard, profile_name=args.profile)
            checkpoint = target
            if code != 0:
                value["result"] = "failed"
            return value

        def browser(phase: str, guard: str) -> None:
            target = destination("browser-" + phase)
            code = child(["node", "tools/playwright/device_existing_account_smoke.mjs",
                          "/home/w0w/.local/share/playwright/node_modules/playwright/index.mjs",
                          "/home/w0w/.cache/ms-playwright/chromium-1228/chrome-linux64/chrome",
                          args.base_url.rstrip("/"), args.username, str(target), Path(guard).name + "-" + phase], "gui", 60)
            driver.require(code == 0, "phase browser failed")
            driver.browser_gate(str(target), guard, phase)
            browser_receipts[phase] = target

        def record(value: dict[str, object]) -> None:
            write_json_0600_atomic(output, {**value, "runId": run_id,
                "lastCheckpoint": str(checkpoint) if checkpoint else None,
                "browserReceipts": {key: str(path) for key, path in browser_receipts.items()}})

        result = flow.run_flow(app, browser, record, profile_name=args.profile)
    if result["result"] != "passed":
        raise SystemExit(1)


if __name__ == "__main__":
    main()
