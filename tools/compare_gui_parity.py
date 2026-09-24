#!/usr/bin/env python3
"""Compare two GUI parity receipts produced by gui_parity_probe.mjs.

Reads our build receipt and the reference receipt, and reports menu-label,
settings, request, marker, tag-count and capability differences into a sanitized
private receipt. Empty observed collections are compared rather than treated as
missing data. The reference's known capability endpoint 404 is recorded as an
explicit asymmetry; incomplete capture coverage or any other parity gap produces
`result=failed`. It reads no cookies, storage or credentials and performs no
mutation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import TypeGuard, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

PROBE = "hardened-blikvm-gui-parity"
MAX_LABELS = 200
MAX_CAPABILITIES = 200
MAX_CAPABILITY_VALUE = 32
MAX_FAILED_REQUESTS = 20
MAX_SETTINGS_TABS = 20
COVERAGE_FIELDS = (
    "menuLabels",
    "tagCounts",
    "settingsTabs",
    "failedRequests",
    "markers",
    "capabilitiesRequest",
)

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


Receipt = dict[str, object]


def _is_object(value: object) -> TypeGuard[Receipt]:
    return isinstance(value, dict) and all(
        isinstance(key, str) for key in cast(dict[object, object], value)
    )


def _is_list(value: object) -> TypeGuard[list[object]]:
    return isinstance(value, list)


def _labels(receipt: Receipt) -> list[str]:
    values = receipt.get("menuLabels")
    if not _is_list(values) or len(values) > MAX_LABELS:
        raise ValueError("invalid menu labels")
    if not all(isinstance(value, str) for value in values):
        raise ValueError("invalid menu label entry")
    return cast(list[str], values)


def _console_errors(receipt: Receipt) -> int:
    value = receipt.get("consoleErrors", 0)
    if type(value) is not int or not 0 <= value <= 10_000:
        raise ValueError("invalid console error count")
    return value


def _size(value: object) -> int:
    if _is_object(value) or _is_list(value):
        return len(value)
    raise ValueError("invalid comparison collection")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    _ = parser.add_argument("--ours", required=True)
    _ = parser.add_argument("--reference", required=True)
    _ = parser.add_argument("--output", required=True)
    return parser.parse_args()


def _evidence_context(receipt: Receipt) -> dict[str, str]:
    value = receipt.get("evidenceContext")
    if not _is_object(value) or set(value) != {
        "acceptanceRunId",
        "reviewedSourceCommit",
    }:
        raise ValueError("invalid evidence context")
    if not all(isinstance(item, str) and item for item in value.values()):
        raise ValueError("invalid evidence context value")
    return cast(dict[str, str], value)


def _coverage(receipt: Receipt) -> dict[str, str]:
    value = receipt.get("coverage")
    if not _is_object(value) or set(value) != set(COVERAGE_FIELDS):
        raise ValueError("invalid GUI coverage")
    if not all(
        isinstance(state, str) and state in {"complete", "failed", "truncated"}
        for state in value.values()
    ):
        raise ValueError("invalid GUI coverage state")
    return cast(dict[str, str], value)


def _capability_request(receipt: Receipt) -> dict[str, object]:
    value = receipt.get("capabilitiesRequest")
    if not _is_object(value) or set(value) != {"method", "route", "outcome", "status"}:
        raise ValueError("invalid capability request")
    if value["method"] != "GET" or value["route"] != "/api/capabilities":
        raise ValueError("unexpected capability request")
    if not isinstance(value["outcome"], str) or value["outcome"] not in {
        "http",
        "error",
        "not_applicable",
    }:
        raise ValueError("invalid capability request outcome")
    if type(value["status"]) is not int or not 0 <= value["status"] <= 599:
        raise ValueError("invalid capability request status")
    return value


def load_receipt(path: Path, expected_role: str | None = None) -> Receipt:
    content = path.read_text(encoding="utf-8")
    if len(content) > 1_048_576:
        raise ValueError("parity receipt is too large")
    receipt: object = json.loads(content)
    if not _is_object(receipt):
        raise ValueError("invalid parity receipt object")
    if receipt.get("schemaVersion") != 2 or receipt.get("probe") != PROBE:
        raise ValueError("unexpected parity probe name")
    role = receipt.get("targetRole")
    if not isinstance(role, str) or role not in {"ours", "reference"} or (expected_role and role != expected_role):
        raise ValueError("unexpected parity target role")
    result = receipt.get("result")
    if not isinstance(result, str) or result not in {"passed", "failed"}:
        raise ValueError("invalid parity capture result")
    _evidence_context(receipt)
    _coverage(receipt)
    _capability_request(receipt)
    _labels(receipt)
    console_errors = receipt.get("consoleErrors")
    if type(console_errors) is not int or not 0 <= console_errors <= 10_000:
        raise ValueError("invalid console error count")
    _capabilities(receipt)
    _markers(receipt)
    _settings_tabs(receipt)
    _failed_requests(receipt)
    _tag_counts(receipt)
    return receipt


def _capabilities(receipt: Receipt) -> dict[str, str]:
    values = receipt.get("capabilities")
    if not _is_object(values) or len(values) > MAX_CAPABILITIES:
        raise ValueError("invalid capabilities")
    result: dict[str, str] = {}
    for key, value in values.items():
        if len(key) > 80:
            raise ValueError("invalid capability path")
        if not isinstance(value, str) or len(value) > MAX_CAPABILITY_VALUE:
            raise ValueError("invalid capability value")
        result[key] = value
    return result


def _markers(receipt: Receipt) -> dict[str, int]:
    values = receipt.get("markers")
    if not _is_object(values) or len(values) > 16:
        raise ValueError("invalid markers")
    result: dict[str, int] = {}
    for key, value in values.items():
        if type(value) is not int or value < 0:
            raise ValueError("invalid marker entry")
        result[key] = value
    return result


def _is_expected(label: str) -> bool:
    return any(pattern.fullmatch(label) for pattern in EXPECTED_LABEL_DIFFERENCES)


def _settings_tabs(receipt: Receipt) -> list[str]:
    values = receipt.get("settingsTabs")
    if not _is_list(values) or len(values) > MAX_SETTINGS_TABS:
        raise ValueError("invalid settings tabs")
    if not all(isinstance(value, str) and 0 < len(value) <= 40 for value in values):
        raise ValueError("invalid settings tab entry")
    return cast(list[str], values)


def _failed_requests(receipt: Receipt) -> list[dict[str, str]]:
    values = receipt.get("failedRequests")
    if not _is_list(values) or len(values) > MAX_FAILED_REQUESTS:
        raise ValueError("invalid failed requests")
    result: list[dict[str, str]] = []
    for value in values:
        if not _is_object(value):
            raise ValueError("invalid failed request entry")
        method = value.get("method")
        route = value.get("route")
        status = value.get("status")
        if not isinstance(method, str) or len(method) > 8:
            raise ValueError("invalid failed request method")
        if not isinstance(route, str) or not route.startswith("/") or len(route) > 120:
            raise ValueError("invalid failed request route")
        if type(status) is not int or not 400 <= status <= 599:
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


def _tag_counts(receipt: Receipt) -> dict[str, int]:
    counts = receipt.get("tagCounts")
    if not _is_object(counts):
        raise ValueError("invalid tag counts")
    result: dict[str, int] = {}
    for key, value in counts.items():
        if type(value) is not int or value < 0:
            raise ValueError("invalid tag count entry")
        result[key] = value
    return result


def compare(ours: Receipt, reference: Receipt) -> Receipt:
    our_context = _evidence_context(ours)
    reference_context = _evidence_context(reference)
    our_coverage = _coverage(ours)
    reference_coverage = _coverage(reference)
    coverage_gaps: list[str] = []
    if ours.get("targetRole") != "ours":
        coverage_gaps.append("ours.targetRole")
    if reference.get("targetRole") != "reference":
        coverage_gaps.append("reference.targetRole")
    if ours.get("result") != "passed":
        coverage_gaps.append("ours.result")
    if reference.get("result") != "passed":
        coverage_gaps.append("reference.result")
    if our_context != reference_context:
        coverage_gaps.append("evidenceContext")
    for side, coverage in (("ours", our_coverage), ("reference", reference_coverage)):
        for name in COVERAGE_FIELDS:
            if coverage[name] != "complete":
                coverage_gaps.append(f"{side}.coverage.{name}")

    our_labels = set(_labels(ours))
    reference_labels = set(_labels(reference))
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
    our_capability_request = _capability_request(ours)
    reference_capability_request = _capability_request(reference)
    capabilities_compared = False
    expected_asymmetries: list[dict[str, object]] = []
    if (
        our_capability_request["outcome"] == "http"
        and our_capability_request["status"] == 200
        and reference_capability_request["outcome"] == "http"
        and reference_capability_request["status"] == 404
    ):
        expected_asymmetries.append(
            {
                "side": "reference",
                "method": "GET",
                "route": "/api/capabilities",
                "oursStatus": 200,
                "referenceStatus": 404,
            }
        )
    elif (
        our_capability_request == reference_capability_request
        and our_capability_request["outcome"] in {"http", "not_applicable"}
        and our_capability_request["status"] in {0, 200}
    ):
        capabilities_compared = True
    else:
        coverage_gaps.append("capabilitiesRequest")
    capability_diffs: dict[str, dict[str, str | None]] = {}
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
    failed_requests_compared = True
    our_failures = {
        f"{entry['method']} {entry['route']}:{entry['status']}"
        for entry in our_failure_list
    }
    reference_failures = {
        f"{entry['method']} {entry['route']}:{entry['status']}"
        for entry in reference_failure_list
    }

    our_tabs = set(_settings_tabs(ours))
    reference_tabs = set(_settings_tabs(reference))
    settings_tabs_compared = True

    our_markers = _markers(ours)
    reference_markers = _markers(reference)
    markers_compared = True
    marker_diffs = {
        name: {"ours": our_markers.get(name), "reference": reference_markers.get(name)}
        for name in sorted(set(our_markers) | set(reference_markers))
        if our_markers.get(name) != reference_markers.get(name)
    }

    expected_ours = [
        label
        for label in labels_only_in_ours
        if label not in explained_ours and _is_expected(label)
    ]
    expected_reference = [
        label
        for label in labels_only_in_reference
        if label not in explained_reference and _is_expected(label)
    ]
    unexplained_ours = [
        label
        for label in labels_only_in_ours
        if label not in explained_ours and not _is_expected(label)
    ]
    unexplained_reference = [
        label
        for label in labels_only_in_reference
        if label not in explained_reference and not _is_expected(label)
    ]
    settings_only_ours = sorted(our_tabs - reference_tabs)
    settings_only_reference = sorted(reference_tabs - our_tabs)
    failures_only_ours = sorted(our_failures - reference_failures)
    failures_only_reference = sorted(reference_failures - our_failures)
    ours_console_errors = _console_errors(ours)
    reference_console_errors = _console_errors(reference)
    parity_failed = any(
        (
            capability_diffs,
            unexplained_ours,
            unexplained_reference,
            settings_only_ours,
            settings_only_reference,
            failures_only_ours,
            failures_only_reference,
            marker_diffs,
        )
    ) or ours_console_errors != 0
    result = "passed" if not coverage_gaps and not parity_failed else "failed"
    if coverage_gaps:
        comparison_outcome = "coverage-incomplete"
    elif parity_failed:
        comparison_outcome = "parity-mismatch"
    elif expected_asymmetries:
        comparison_outcome = "accepted-with-expected-asymmetry"
    else:
        comparison_outcome = "passed"

    return {
        "schemaVersion": 2,
        "probe": "hardened-blikvm-gui-parity-compare",
        "result": result,
        "comparisonOutcome": comparison_outcome,
        "evidenceContext": our_context,
        "coverageGaps": coverage_gaps,
        "expectedAsymmetries": expected_asymmetries,
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
        "expectedLabelsOnlyInOurs": expected_ours,
        "expectedLabelsOnlyInReference": expected_reference,
        "unexplainedLabelsOnlyInOurs": unexplained_ours,
        "unexplainedLabelsOnlyInReference": unexplained_reference,
        "settingsTabsCompared": settings_tabs_compared,
        "settingsTabsOnlyInOurs": settings_only_ours,
        "settingsTabsOnlyInReference": settings_only_reference,
        "failedRequestsCompared": failed_requests_compared,
        "failedRequestsOnlyInOurs": failures_only_ours,
        "failedRequestsOnlyInReference": failures_only_reference,
        "markersCompared": markers_compared,
        "markerDiffs": marker_diffs,
        "oursConsoleErrors": ours_console_errors,
        "referenceConsoleErrors": reference_console_errors,
        "consoleErrorDelta": ours_console_errors - reference_console_errors,
        "mutationsPerformed": False,
    }


def execute(args: argparse.Namespace) -> int:
    output = prototype.validate_output_path(args.output)
    ours_path = Path(args.ours).resolve(strict=True)
    reference_path = Path(args.reference).resolve(strict=True)
    ours = load_receipt(ours_path, "ours")
    reference = load_receipt(reference_path, "reference")
    receipt = compare(ours, reference)
    receipt["inputSha256"] = {
        "ours": hashlib.sha256(ours_path.read_bytes()).hexdigest(),
        "reference": hashlib.sha256(reference_path.read_bytes()).hexdigest(),
    }
    serialized = json.dumps(receipt, sort_keys=True)
    digest = hashlib.sha256(serialized.encode()).hexdigest()
    write_json_0600_atomic(output, receipt)
    print(
        f"parity_compare={output} onlyOurs={_size(receipt['labelsOnlyInOurs'])} "
        f"onlyReference={_size(receipt['labelsOnlyInReference'])} "
        f"explainedReference={_size(receipt['explainedLabelsOnlyInReference'])} "
        f"unexplainedReference={_size(receipt['unexplainedLabelsOnlyInReference'])} "
        f"capabilityDiffs={_size(receipt['capabilityDiffs'])} "
        f"failedOnlyInOurs={_size(receipt['failedRequestsOnlyInOurs'])} "
        f"result={receipt['result']} receiptSha256={digest}"
    )
    return 0 if receipt["result"] == "passed" else 1


def main() -> int:
    try:
        return execute(parse_args())
    except (OSError, UnicodeError, ValueError, json.JSONDecodeError) as error:
        _ = error
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
