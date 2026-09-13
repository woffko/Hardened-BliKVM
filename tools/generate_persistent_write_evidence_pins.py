#!/usr/bin/env python3
"""Generate persistent_write_evidence_pins.py from reviewed producer sources."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
TOOLS = ROOT / "tools"

if not __package__:
    sys.path.insert(0, str(ROOT))
from tools import persistent_write_activation_receipt as activation
from tools import persistent_write_gui_failure_receipt as gui_failure
from tools import persistent_write_host_receipt as host_receipt
from tools import persistent_write_recovery_receipt as recovery
from tools import persistent_write_upload_receipt as upload_receipt

HISTORICAL_COMMITS = {"gui_happy_path": "07a0b39", "native_activation": "07a0b39"}
TARGET = TOOLS / "persistent_write_evidence_pins.py"


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def git_show(commit: str, path: str) -> bytes:
    return subprocess.check_output(["git", "show", commit + ":" + path], cwd=ROOT, timeout=30)


def browser_sources() -> tuple[str, ...]:
    return tuple(sorted(activation.BROWSER_SOURCES))


def build() -> dict[str, object]:
    current = {
        "runner": digest((TOOLS / "run_persistent_write_rehearsal.py").read_bytes()),
        "browser_sources": {name: digest((TOOLS / "playwright" / name).read_bytes()) for name in browser_sources()},
        "failure_browser_sources": {name: digest((TOOLS / "playwright" / name).read_bytes())
                                    for name in sorted(gui_failure.NEGATIVE_BROWSER_SOURCES)},
        "host_sources": {path: digest((ROOT / path).read_bytes()) for path in host_receipt.source_paths()},
        "host_tool": digest((TOOLS / "capture_persistent_write_host_gate.py").read_bytes()),
        "upload_sources": {name: digest((TOOLS / name).read_bytes()) for name in upload_receipt.SOURCES},
        "upload_tool": digest((TOOLS / "capture_persistent_write_upload_gate.py").read_bytes()),
        "probe_sources": {action: {name: digest((TOOLS / name).read_bytes())
                                   for name in sorted(recovery.PROBES[action])}
                          for action in sorted(recovery.PROBES)},
    }
    historical: dict[str, object] = {}
    for gate, revision in sorted(HISTORICAL_COMMITS.items()):
        commit = subprocess.check_output(["git", "rev-parse", revision], cwd=ROOT, text=True, timeout=30).strip()
        historical[gate] = {
            "commit": commit,
            "runner": digest(git_show(revision, "tools/run_persistent_write_rehearsal.py")),
            "browser_sources": {name: digest(git_show(revision, "tools/playwright/" + name))
                                for name in browser_sources()},
        }
    return {"CURRENT_PINS": current, "HISTORICAL_PINS": historical}


def render(value: dict[str, object]) -> str:
    return ('"""Generated producer pins for persistent-write evidence."""\n\n'
            f"CURRENT_PINS = {json.dumps(value['CURRENT_PINS'], sort_keys=True, indent=4)}\n\n"
            f"HISTORICAL_PINS = {json.dumps(value['HISTORICAL_PINS'], sort_keys=True, indent=4)}\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="fail when the committed pins are stale")
    args = parser.parse_args()
    rendered = render(build())
    if args.check:
        return 0 if TARGET.read_text() == rendered else 1
    TARGET.write_text(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
