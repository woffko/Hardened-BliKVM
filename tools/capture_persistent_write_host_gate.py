#!/usr/bin/env python3
"""Run the bounded host controller suite and bind its result to a transaction."""

import argparse
from collections.abc import Callable
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import re
import subprocess
import sys
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import persistent_write_activation_receipt as receipts
from tools import persistent_write_contract as contract
from tools import persistent_write_host_receipt as host
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

ROOT = Path(__file__).resolve().parent.parent
TOOL = Path(__file__).resolve()
Runner = Callable[..., subprocess.CompletedProcess[bytes]]


def read_binding(value: object) -> contract.EvidenceBinding:
    document = receipts.object_value(value)
    prepared = document.get('prepared')
    if isinstance(prepared, dict):
        wrapped = cast(dict[str, object], prepared)
        return receipts.read_binding(wrapped.get('binding'))
    return receipts.read_binding(document.get('binding'))


def source_hashes() -> dict[str, str]:
    return {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in host.source_paths()}


def run_suite(runner: Runner = subprocess.run) -> dict[str, object]:
    result = runner(host.COMMAND, cwd=str(ROOT), capture_output=True, timeout=600, check=False)
    text = (result.stdout + result.stderr).decode('utf-8', 'replace')
    match = re.search(r'Ran (\d+) tests? in', text)
    common.require(result.returncode == 0 and match is not None and re.search(r'\nOK\b', text) is not None,
                   'host suite did not pass')
    assert match is not None
    return {'command': host.COMMAND, 'tests': int(match.group(1)), 'result': 'passed',
            'sourceHashes': source_hashes()}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binding-receipt', required=True)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    binding = read_binding(json.loads(common.transport.validate_private_file(args.binding_receipt).read_bytes()))
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), 'new host gate receipt required')
    receipt: dict[str, object] = {'probe': 'persistent-write-host-gate', 'schemaVersion': 1, 'result': 'failed',
        'binding': asdict(binding), 'toolSha256': hashlib.sha256(TOOL.read_bytes()).hexdigest()}
    write_json_0600_atomic(output, receipt)
    try:
        suite = run_suite()
        candidate = {**receipt, 'suite': suite, 'result': 'passed'}
        host.parse(json.dumps(candidate, sort_keys=True, separators=(',', ':')).encode(),
                   expected_python_sources=host_source_pins(), expected_tool=str(receipt['toolSha256']))
        receipt.update(result='passed', suite=suite)
    finally:
        write_json_0600_atomic(output, receipt)
    return 0 if receipt['result'] == 'passed' else 1


def host_source_pins() -> dict[str, str]:
    return source_hashes()


if __name__ == '__main__':
    raise SystemExit(main())
