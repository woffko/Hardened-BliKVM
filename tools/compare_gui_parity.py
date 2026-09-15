#!/usr/bin/env python3
"""Compare two GUI parity receipts produced by gui_parity_probe.mjs.

Reads our build receipt and the reference receipt, and reports the menu-label,
tag-count and capability differences into a sanitized private receipt. Label
differences that a differing capability explains (for example a HID label that
matches a capability our build hides by design) are separated from unexplained
differences, which are the real parity gaps. It reads no cookies, storage or
credentials and performs no mutation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

PROBE = "hardened-blikvm-gui-parity"
MAX_LABELS = 200
MAX_CAPABILITIES = 200
MAX_CAPABILITY_VALUE = 32
MAX_FAILED_REQUESTS = 20

# Label differences that are intentional product branding, not parity gaps.
EXPECTED_LABEL_DIFFERENCES = (
    re.compile(r"^Hardened (Nano|Bli)KVM$"),
)

# Label topics mapped to the capability path fragment that owns them.
EXPLAINERS = (
    (re.compile(r"keyboard|key ?map|hotkey|shortcut|combination|on-screen", re.I), "hid"),
    (re.compile(r"mouse|pointer|cursor|trackpad", re.I), "hid"),
    (re.compile(r"image|media|mount|iso|mass storage|cd.?rom|storage|upload|download", re.I), "virtualmedia"),
    (re.compile(r"power|reset|atx|reboot|shutdown", re.I), "power"),
    (re.compile(r"video|stream|h\.?264|mjpeg|direct|resolution|quality|fps|capture", re.I), "stream"),
    (re.compile(r"terminal|shell|console|command", re.I), "terminal"),
    (re.compile(r"picoclaw|agent|chat", re.I), "picoclaw"),
)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--ours", required=True)
    _ = parser.add_argument("--reference", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def load_receipt(path: Path) -> dict:
    content = path.read_text(encoding="utf-8")
    if len(content) > 1_048_576:
        raise ValueError("parity receipt is too large")
    receipt = json.loads(content)
    if receipt.get("probe") != PROBE:
        raise ValueError("unexpected parity probe name")
    labels = receipt.get("menuLabels")
    if not isinstance(labels, list) or len(labels) > MAX_LABELS:
        raise ValueError("invalid menu labels")
    if not all(isinstance(label, str) for label in labels):
        raise ValueError("invalid menu label entry")
    return receipt


def _capabilities(receipt: dict) -> dict[str, str]:
    values = receipt.get("capabilities") or {}
    if not isinstance(values, dict) or len(values) > MAX_CAPABILITIES:
        raise ValueError("invalid capabilities")
    result: dict[str, str] = {}
    for key, value in values.items():
        if not isinstance(key, str) or len(key) > 80:
            raise ValueError("invalid capability path")
        if not isinstance(value, str) or len(value) > MAX_CAPABILITY_VALUE:
            raise ValueError("invalid capability value")
        result[key] = value
    return result


def _markers(receipt: dict) -> dict[str, int]:
    values = receipt.get("markers") or {}
    if not isinstance(values, dict) or len(values) > 16:
        raise ValueError("invalid markers")
    result: dict[str, int] = {}
    for key, value in values.items():
        if not isinstance(key, str) or not isinstance(value, int) or value < 0:
            raise ValueError("invalid marker entry")
        result[key] = value
    return result


def _is_expected(label: str) -> bool:
    return any(pattern.fullmatch(label) for pattern in EXPECTED_LABEL_DIFFERENCES)


def _failed_requests(receipt: dict) -> list[dict[str, str]]:
    values = receipt.get("failedRequests") or []
    if not isinstance(values, list) or len(values) > MAX_FAILED_REQUESTS:
        raise ValueError("invalid failed requests")
    result: list[dict[str, str]] = []
    for value in values:
        if not isinstance(value, dict):
            raise ValueError("invalid failed request entry")
        method = value.get("method")
        route = value.get("route")
        status = value.get("status")
        if not isinstance(method, str) or len(method) > 8:
            raise ValueError("invalid failed request method")
        if not isinstance(route, str) or not route.startswith("/") or len(route) > 120:
            raise ValueError("invalid failed request route")
        if not isinstance(status, int) or not 400 <= status <= 599:
            raise ValueError("invalid failed request status")
        result.append({"method": method, "route": route, "status": str(status)})
    return result


def _explain(label: str, changed_paths: set[str]) -> str | None:
    for pattern, topic in EXPLAINERS:
        if pattern.search(label):
            for path in sorted(changed_paths):
                if topic in path.lower():
                    return path
            return None
    return None


def _tag_counts(receipt: dict) -> dict[str, int]:
    counts = receipt.get("tagCounts")
    if not isinstance(counts, dict):
        raise ValueError("invalid tag counts")
    result: dict[str, int] = {}
    for key, value in counts.items():
        if not isinstance(key, str) or not isinstance(value, int) or value < 0:
            raise ValueError("invalid tag count entry")
        result[key] = value
    return result


def compare(ours: dict, reference: dict) -> dict:
    our_labels = set(ours["menuLabels"])
    reference_labels = set(reference["menuLabels"])
    our_tags = _tag_counts(ours)
    reference_tags = _tag_counts(reference)
    tags = sorted(set(our_tags) | set(reference_tags))
    tag_deltas = {
        tag: our_tags.get(tag, 0) - reference_tags.get(tag, 0)
        for tag in tags
        if our_tags.get(tag, 0) != reference_tags.get(tag, 0)
    }
    our_capabilities = _capabilities(ours)
    reference_capabilities = _capabilities(reference)
    capabilities_compared = bool(our_capabilities) and bool(reference_capabilities)
    capability_diffs = {}
    if capabilities_compared:
        for path in sorted(set(our_capabilities) | set(reference_capabilities)):
            ours_value = our_capabilities.get(path)
            reference_value = reference_capabilities.get(path)
            if ours_value != reference_value:
                capability_diffs[path] = {
                    "ours": ours_value,
                    "reference": reference_value,
                }
    changed_paths = set(capability_diffs)

    labels_only_in_ours = sorted(our_labels - reference_labels)
    labels_only_in_reference = sorted(reference_labels - our_labels)
    explained_ours: dict[str, str] = {}
    explained_reference: dict[str, str] = {}
    for label in labels_only_in_ours:
        path = _explain(label, changed_paths)
        if path:
            explained_ours[label] = path
    for label in labels_only_in_reference:
        path = _explain(label, changed_paths)
        if path:
            explained_reference[label] = path

    our_failure_list = _failed_requests(ours)
    reference_failure_list = _failed_requests(reference)
    failed_requests_compared = bool(our_failure_list) and bool(reference_failure_list)
    our_failures = (
        {
            f"{entry['method']} {entry['route']}:{entry['status']}"
            for entry in our_failure_list
        }
        if failed_requests_compared
        else set()
    )
    reference_failures = (
        {
            f"{entry['method']} {entry['route']}:{entry['status']}"
            for entry in reference_failure_list
        }
        if failed_requests_compared
        else set()
    )

    our_markers = _markers(ours)
    reference_markers = _markers(reference)
    markers_compared = bool(our_markers) and bool(reference_markers)
    marker_diffs = {}
    if markers_compared:
        marker_diffs = {
            name: {"ours": our_markers.get(name), "reference": reference_markers.get(name)}
            for name in sorted(set(our_markers) | set(reference_markers))
            if our_markers.get(name) != reference_markers.get(name)
        }

    return {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-gui-parity-compare",
        "oursRoute": ours.get("route", ""),
        "referenceRoute": reference.get("route", ""),
        "oursTitle": ours.get("title", ""),
        "referenceTitle": reference.get("title", ""),
        "labelsOnlyInOurs": labels_only_in_ours,
        "labelsOnlyInReference": labels_only_in_reference,
        "sharedLabelCount": len(our_labels & reference_labels),
        "tagCountDeltas": tag_deltas,
        "capabilitiesCompared": capabilities_compared,
        "capabilityDiffs": capability_diffs,
        "explainedLabelsOnlyInOurs": explained_ours,
        "explainedLabelsOnlyInReference": explained_reference,
        "expectedLabelsOnlyInOurs": [
            label
            for label in labels_only_in_ours
            if label not in explained_ours and _is_expected(label)
        ],
        "expectedLabelsOnlyInReference": [
            label
            for label in labels_only_in_reference
            if label not in explained_reference and _is_expected(label)
        ],
        "unexplainedLabelsOnlyInOurs": [
            label
            for label in labels_only_in_ours
            if label not in explained_ours and not _is_expected(label)
        ],
        "unexplainedLabelsOnlyInReference": [
            label
            for label in labels_only_in_reference
            if label not in explained_reference and not _is_expected(label)
        ],
        "failedRequestsCompared": failed_requests_compared,
        "failedRequestsOnlyInOurs": sorted(our_failures - reference_failures),
        "failedRequestsOnlyInReference": sorted(reference_failures - our_failures),
        "markersCompared": markers_compared,
        "markerDiffs": marker_diffs,
        "consoleErrorDelta": int(ours.get("consoleErrors", 0))
        - int(reference.get("consoleErrors", 0)),
        "mutationsPerformed": False,
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    ours = load_receipt(Path(args.ours).resolve(strict=True))
    reference = load_receipt(Path(args.reference).resolve(strict=True))
    receipt = compare(ours, reference)
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    print(
        f"parity_compare={output} onlyOurs={len(receipt['labelsOnlyInOurs'])} "
        f"onlyReference={len(receipt['labelsOnlyInReference'])} "
        f"explainedReference={len(receipt['explainedLabelsOnlyInReference'])} "
        f"unexplainedReference={len(receipt['unexplainedLabelsOnlyInReference'])} "
        f"capabilityDiffs={len(receipt['capabilityDiffs'])} "
        f"failedOnlyInOurs={len(receipt['failedRequestsOnlyInOurs'])} receiptSha256={digest}"
    )
    return 0


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as error:
        _ = error
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
