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
        "phase": "E0",
        "title": "video replacement",
        "host": (
            "packaging/systemd/hardened-blikvm-ustreamer.service",
            "packaging/systemd/hardened-blikvm-board-video-proxy.service",
            "packaging/udev/99-hardened-blikvm-video.rules",
            "tools/playwright/video_frame_timing_probe.mjs",
            "tools/playwright/video_two_client_probe.mjs",
            "tools/playwright/video_soak_probe.mjs",
        ),
        "device": (
            {
                "name": "frame timing",
                "glob": "video-frame-timing-*.json",
                "probe": "hardened-blikvm-video-frame-timing",
                "marker": "result=passed",
            },
            {
                "name": "two-client video",
                "glob": "video-two-client-probe-*.json",
                "marker": "result=passed",
                "require": ("streamCount",),
            },
            {
                "name": "video soak",
                "glob": "video-soak-probe-*.json",
                "marker": "result=passed",
                "require": ("durationSeconds",),
            },
        ),
    },
    {
        "phase": "E1",
        "title": "storage and media",
        "host": (
            "crates/bli-kvm-server/src/media.rs",
            "crates/bli-kvm-usb-gadget/src/lib.rs",
            "crates/bli-kvm-media-catalog/src/lib.rs",
            "tools/storage_acceptance_probe.py",
            "tools/upload_media_image.py",
            "packaging/tmpfiles.d/hardened-blikvm-gadget.conf",
        ),
        "device": (
            {
                "name": "storage gate",
                "glob": "storage-acceptance-*.json",
                "probe": "hardened-blikvm-storage-acceptance",
                "marker": "result=passed",
            },
            {
                "name": "media upload",
                "glob": "storage-upload-*.json",
                "probe": "hardened-blikvm-media-upload",
                "marker": "applied=true",
            },
            {
                "name": "virtual media GUI acceptance",
                "glob": "virtual-media-acceptance-*.json",
                "probe": "hardened-blikvm-virtual-media-acceptance",
                "marker": "result=passed",
            },
        ),
    },
    {
        "phase": "E2",
        "title": "board daemon instead of the vendor C daemon",
        "host": (
            "crates/bli-kvm-boardd/src/lib.rs",
            "crates/bli-kvm-boardd/src/fan.rs",
            "crates/bli-kvm-boardd/src/display.rs",
            "crates/bli-kvm-boardd/src/led.rs",
            "crates/bli-kvm-boardd/src/button.rs",
            "crates/bli-kvm-boardd/src/spi.rs",
            "packaging/systemd/hardened-blikvm-boardd.service",
            "docs/design/board-daemon-v1.md",
            "tools/capture_oled_spi_topology.py",
            "tools/capture_serial_topology.py",
        ),
        "device": (
            {
                "name": "board daemon fan preflight",
                "glob": "boardd-fan-preflight-*.json",
            },
            {
                "name": "fan gate",
                "glob": "boardd-fan-gate-*.json",
            },
            {
                "name": "panel gate",
                "glob": "boardd-panel-gate-*.json",
            },
            {
                "name": "activity LED gate",
                "glob": "boardd-led-gate-*.json",
            },
            {
                "name": "display button gate",
                "glob": "boardd-button-gate-*.json",
            },
            {
                "name": "KVMD retirement",
                "glob": "kvmd-removal-*.json",
                "probe": "hardened-blikvm-kvmd-removal",
                "marker": "removal.kvmd_stopped=yes",
            },
        ),
    },
    {
        "phase": "E3",
        "title": "system actions and network management",
        "host": (
            "crates/bli-kvm-server/src/network.rs",
            "crates/bli-kvm-system-action-policy/src/lib.rs",
            "crates/bli-kvm-system-action-journal/src/lib.rs",
            "crates/bli-kvm-system-action-store/src/lib.rs",
            "tools/stage_device_system_action_gate.py",
        ),
        "device": (
            {
                "name": "system action store",
                "glob": "device-system-action-store-*.json",
                "probe": "hardened-blikvm-system-action-store-native",
                "marker": "result=passed",
            },
            {
                "name": "system action maintenance dispatcher",
                "glob": "device-system-action-maintenance.json",
                "probe": "hardened-blikvm-system-action-maintenance",
                "marker": "result=passed",
            },
            {
                "name": "system action end-to-end gate",
                "glob": "system-action-gate-*.json",
            },
            {
                "name": "network write gate",
                "glob": "network-write-gate-*.json",
            },
        ),
    },
    {
        "phase": "E4",
        "title": "retained signed update path",
        "host": (
            "tools/build_debian_runtime_package.py",
            "tools/build_production_update_bundle.py",
            "tools/verify_production_update_bundle.py",
            "packaging/systemd/hardened-blikvm-server-current.service",
            "docs/baselines/device-installed-application-update.md",
            "docs/design/retained-application-update-runbook.md",
        ),
        "device": (
            {
                "name": "signed application update commit cycle",
                "glob": "persistent-write-*-commit-*.json",
                "probe": "persistent-write-commit-cycle",
            },
            {
                "name": "slot serving cutover",
                "glob": "update-slot-serving-*.json",
            },
        ),
    },
    {
        "phase": "E5",
        "title": "GUI parity and acceptance",
        "host": (
            "tools/playwright/gui_parity_probe.mjs",
            "tools/compare_gui_parity.py",
            "tools/phase8_acceptance_report.py",
            "tools/test_plan_route_drift.py",
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
                "name": "vendor replacement summary",
                "glob": "vendor-replacement-*.json",
            },
        ),
    },
)

NOT_AUDITED = {
    "exec": (
        "exec, terminal and script features stay off by default and are hidden by "
        "capability; they are verified through the manifest and probes rather than "
        "a device gate"
    ),
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
