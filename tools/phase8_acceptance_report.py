#!/usr/bin/env python3
"""Audit the canonical plan against its real artifacts, read-only.

The report walks an explicit, reviewed manifest: the host artifacts each
canonical phase must ship and the device receipts each gate must have produced.
A phase is only `complete` when every host artifact exists and every device item
matches a recorded receipt with the expected marker. A missing receipt is
reported as `unrun`; it is never inferred from code, commit messages or earlier
work. Nothing here proves a device behaviour by itself, and the tool reads only
the receipt files it names, never credentials or cookies.
"""

from __future__ import annotations

import argparse
import glob
import json
import sys
from pathlib import Path
from typing import Any

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

MAX_RECEIPT_BYTES = 1_048_576


def _marker(path: str) -> tuple[str, Any]:
    key, _, raw = path.partition("=")
    if not raw:
        return key, True
    try:
        return key, json.loads(raw)
    except json.JSONDecodeError:
        return key, raw


def _read_marker(document: dict, key: str) -> Any:
    value: Any = document
    for part in key.split("."):
        if not isinstance(value, dict) or part not in value:
            return None
        value = value[part]
    return value


PHASES: tuple[dict[str, Any], ...] = (
    {
        "phase": "0",
        "title": "recon and parity matrix",
        "host": (
            "docs/design/gui-parity-plan.md",
            "docs/baselines/upstream.md",
            "tools/capture_device_kvm_topology.py",
            "tools/test_capture_device_kvm_topology.py",
        ),
        "device": (
            {
                "name": "topology capture",
                "glob": "device-kvm-topology-*.json",
                "probe": "hardened-blikvm-kvm-topology",
            },
        ),
    },
    {
        "phase": "1",
        "title": "read-only platform API",
        "host": (
            "crates/bli-kvm-server/src/lib.rs",
            "crates/bli-kvm-server/src/system_time.rs",
            "crates/bli-kvm-server/src/system_log.rs",
            "tools/test_plan_route_drift.py",
        ),
        "device": (),
    },
    {
        "phase": "2",
        "title": "native board daemon and ATX",
        "host": (
            "crates/bli-kvm-atx-gpio-executor/src/lib.rs",
            "crates/bli-kvm-gpio-chardev/src/lib.rs",
            "crates/bli-kvm-platform-bli/src/hardware.rs",
            "tools/stage_device_atx_live_gate.py",
        ),
        "device": (
            {
                "name": "ATX live gate (fixed routes)",
                "glob": "atx-live-gate-*.json",
                "marker": "facts.power_short=ok",
                "require": ("facts.reset", "facts.power_long"),
            },
            {
                "name": "native ATX GPIO bench confirmation",
                "glob": "board-handoff-gate-*.json",
                "probe": "hardened-blikvm-board-profile-handoff",
                "marker": "atxBenchFacts.power_short=ok",
            },
        ),
    },
    {
        "phase": "3",
        "title": "Rust USB gadget (HID and mass storage)",
        "host": (
            "crates/bli-kvm-usb-gadget/src/lib.rs",
            "crates/bli-kvm-usb-gadget/src/hid_descriptors.rs",
            "tools/stage_device_board_profile_gate.py",
            "tools/capture_device_hid_descriptors.py",
            "tools/playwright/virtual_media_acceptance.mjs",
            "docs/baselines/board-handoff-gate.md",
        ),
        "device": (
            {
                "name": "HID descriptor capture",
                "glob": "device-hid-descriptors-*.json",
                "probe": "hardened-blikvm-hid-descriptors",
            },
            {
                "name": "board-owner handoff gate",
                "glob": "board-handoff-gate-*.json",
                "probe": "hardened-blikvm-board-profile-handoff",
                "marker": "applyFacts.socket_present=yes",
            },
            {
                "name": "restricted power GUI",
                "glob": "restricted-power-*.json",
                "probe": "hardened-blikvm-restricted-power",
                "marker": "result=passed",
            },
            {
                "name": "media attach/eject gate",
                "glob": "restricted-control-media-gate-*.json",
                "probe": "restricted-control-media-attach-eject",
            },
            {
                "name": "composite gadget acceptance",
                "glob": "board-handoff-acceptance-*.json",
                "probe": "hardened-blikvm-board-profile-handoff",
                "marker": "acceptanceFacts.gadget_present=yes",
            },
        ),
    },
    {
        "phase": "4",
        "title": "video gates",
        "host": (
            "tools/playwright/video_two_client_probe.mjs",
            "tools/playwright/video_soak_probe.mjs",
            "tools/playwright/mjpeg_completion_probe.mjs",
        ),
        "device": (
            {
                "name": "two-client video",
                "glob": "video-two-client-probe-*.json",
                "marker": "result=passed",
                "require": ("streamCount", "streams"),
            },
            {
                "name": "KVMD removal",
                "glob": "kvmd-removal-*.json",
                "probe": "hardened-blikvm-kvmd-removal",
                "marker": "removal.kvmd_stopped=yes",
            },
            {
                "name": "frame timing",
                "glob": "video-frame-timing-*.json",
                "probe": "hardened-blikvm-video-frame-timing",
                "marker": "result=passed",
            },
            {
                "name": "video soak",
                "glob": "video-soak-probe-*.json",
                "marker": "result=passed",
                "require": ("durationSeconds", "samples"),
            },
        ),
    },
    {
        "phase": "6",
        "title": "system update and extensions",
        "host": (
            "tools/build_production_update_bundle.py",
            "tools/verify_production_update_bundle.py",
            "docs/baselines/application-update-manager.md",
        ),
        "device": (
            {
                "name": "signed application update commit cycle",
                "glob": "persistent-write-*-commit-*.json",
                "probe": "persistent-write-commit-cycle",
            },
            {
                "name": "runtime package install",
                "glob": "device-update-runtime-package.json",
                "probe": "hardened-blikvm-update-runtime-package-install",
            },
        ),
    },
    {
        "phase": "8",
        "title": "GUI parity and acceptance",
        "host": (
            "tools/playwright/gui_parity_probe.mjs",
            "tools/compare_gui_parity.py",
            "tools/test_compare_gui_parity.py",
            "tools/phase8_acceptance_report.py",
        ),
        "device": (
            {
                "name": "GUI parity capture",
                "glob": "gui-parity-ours-auth-*.json",
                "probe": "hardened-blikvm-gui-parity",
            },
            {
                "name": "GUI parity comparison",
                "glob": "gui-parity-compare-*.json",
                "probe": "hardened-blikvm-gui-parity-compare",
            },
            {
                "name": "virtual media acceptance",
                "glob": "virtual-media-acceptance-*.json",
                "probe": "hardened-blikvm-virtual-media-acceptance",
                "marker": "result=passed",
            },
        ),
    },
)

NOT_AUDITED = {
    "5": "network management: command/device bound, no reviewed host artifact set yet",
    "7": "exec/agent features: hidden by capability; verified through the manifest and probes",
}


def audit_host(root: Path, paths: tuple[str, ...]) -> dict[str, Any]:
    present: list[str] = []
    missing: list[str] = []
    for relative in paths:
        if (root / relative).is_file():
            present.append(relative)
        else:
            missing.append(relative)
    return {"present": present, "missing": missing}


def audit_device(evidence: Path, items: tuple[dict[str, Any], ...]) -> list[dict[str, Any]]:
    results: list[dict[str, Any]] = []
    for item in items:
        matches = sorted(glob.glob(str(evidence / item["glob"])))
        status = "unrun"
        detail = ""
        matched_file = ""
        for candidate in reversed(matches):
            path = Path(candidate)
            try:
                if path.stat().st_size > MAX_RECEIPT_BYTES:
                    detail = "receipt exceeds the size limit"
                    continue
                document = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, UnicodeError, json.JSONDecodeError):
                detail = "receipt is unreadable"
                continue
            if not isinstance(document, dict):
                detail = "receipt is not an object"
                continue
            expected = item.get("probe")
            if expected and document.get("probe") != expected and document.get("gate") != expected:
                detail = f"probe is {document.get('probe') or document.get('gate')!r}"
                continue
            required = item.get("require", ())
            missing = next(
                (key for key in required if _read_marker(document, key) is None), None
            )
            if missing is not None:
                detail = f"receipt is missing {missing}"
                continue
            marker = item.get("marker")
            if marker:
                key, want = _marker(marker)
                got = _read_marker(document, key)
                if got != want:
                    detail = f"marker {key}={got!r}, expected {want!r}"
                    continue
            status = "recorded"
            matched_file = path.name
            detail = ""
            break
        results.append(
            {
                "name": item["name"],
                "glob": item["glob"],
                "status": status,
                "receipt": matched_file,
                "detail": detail,
                "matches": len(matches),
            }
        )
    return results


def build_report(root: Path, evidence: Path) -> dict[str, Any]:
    phases: list[dict[str, Any]] = []
    for spec in PHASES:
        host = audit_host(root, spec["host"])
        device = audit_device(evidence, spec["device"])
        complete = not host["missing"] and all(item["status"] == "recorded" for item in device)
        phases.append(
            {
                "phase": spec["phase"],
                "title": spec["title"],
                "status": "complete" if complete else "incomplete",
                "host": host,
                "device": device,
            }
        )
    return {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-phase8-acceptance",
        "overall": "complete"
        if all(phase["status"] == "complete" for phase in phases)
        else "incomplete",
        "phases": phases,
        "notAudited": NOT_AUDITED,
        "policy": {
            "credentialsRead": False,
            "mutationsPerformed": False,
            "deviceReceiptsRequired": True,
            "inferenceFromCode": False,
        },
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--output", required=True)
    _ = parser.add_argument("--root", default=".")
    _ = parser.add_argument("--evidence", default="evidence/private")
    return parser.parse_args()


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    root = Path(args.root).resolve(strict=True)
    evidence = Path(args.evidence)
    if not evidence.is_absolute():
        evidence = root / evidence
    report = build_report(root, evidence)
    write_json_0600_atomic(output, report)
    unrun = sum(
        1 for phase in report["phases"] for item in phase["device"] if item["status"] != "recorded"
    )
    missing = sum(len(phase["host"]["missing"]) for phase in report["phases"])
    print(
        f"phase8_acceptance={output} overall={report['overall']} "
        f"hostMissing={missing} deviceUnrun={unrun}"
    )
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError) as error:
        _ = error
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
