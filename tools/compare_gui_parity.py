#!/usr/bin/env python3
"""Compare two GUI parity receipts produced by gui_parity_probe.mjs.

Reads our build receipt and the reference receipt, and reports the menu-label
and tag-count differences into a sanitized private receipt. It reads no cookies,
storage or credentials and performs no mutation.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools import stage_device_prototype as prototype

PROBE = "hardened-blikvm-gui-parity"
MAX_LABELS = 200


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
    return {
        "schemaVersion": 1,
        "probe": "hardened-blikvm-gui-parity-compare",
        "oursRoute": ours.get("route", ""),
        "referenceRoute": reference.get("route", ""),
        "oursTitle": ours.get("title", ""),
        "referenceTitle": reference.get("title", ""),
        "labelsOnlyInOurs": sorted(our_labels - reference_labels),
        "labelsOnlyInReference": sorted(reference_labels - our_labels),
        "sharedLabelCount": len(our_labels & reference_labels),
        "tagCountDeltas": tag_deltas,
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
        f"onlyReference={len(receipt['labelsOnlyInReference'])} receiptSha256={digest}"
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
