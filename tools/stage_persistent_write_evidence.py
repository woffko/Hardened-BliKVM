#!/usr/bin/env python3
"""Stream reviewed gate receipts into the device evidence store and seal them."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import shlex
import sys
import tempfile

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import persistent_write_contract as contract
from tools import persistent_write_evidence_store as storage
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic

ENVELOPE_LIMIT = 16 * 1024 * 1024


def load_receipts(values: list[str]) -> dict[str, bytes]:
    receipts: dict[str, bytes] = {}
    for raw in values:
        gate, separator, path = raw.partition("=")
        common.require(bool(separator) and gate in contract.REQUIRED_GATES and gate not in receipts,
                       "unique gate=path receipts required")
        data = common.transport.validate_private_file(path).read_bytes()
        common.require(0 < len(data) <= storage.MAX_BYTES, "gate receipt size outside bounds")
        receipts[gate] = data
    common.require(set(receipts) == set(contract.REQUIRED_GATES), "complete gate receipts required")
    return receipts


def build_envelope(receipts: dict[str, bytes]) -> bytes:
    gates = {gate: base64.b64encode(receipts[gate]).decode("ascii") for gate in sorted(receipts)}
    return json.dumps({"schemaVersion": 1, "gates": gates}, sort_keys=True, separators=(",", ":")).encode()


def validate_device_result(value: object, receipts: dict[str, bytes], transaction: str) -> dict[str, object]:
    result = common.native.object_value(value)
    seal = result.get("sealSha256")
    expected = {gate: hashlib.sha256(receipts[gate]).hexdigest() for gate in receipts}
    expected_root = str(storage.PARENT / (storage.PREFIX + transaction))
    common.require(result.get("result") == "sealed" and result.get("evidenceRoot") == expected_root
                   and isinstance(seal, str) and common.native.re.fullmatch(r"[a-f0-9]{64}", seal) is not None
                   and result.get("receipts") == expected, "device evidence seal differs")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "guard-root", "output"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--gate", action="append", required=True, metavar="GATE=PATH")
    args = parser.parse_args()
    root = Path(args.guard_root)
    contract.guard_units(root)
    transaction = root.name[len(contract.PREFIX):]
    receipts = load_receipts(args.gate)
    envelope = build_envelope(receipts)
    common.require(len(envelope) <= ENVELOPE_LIMIT, "evidence envelope exceeds bound")
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new evidence receipt required")
    host, port = common.transport.parse_endpoint(args.endpoint)
    target = common.transport.RemoteTarget(host, port, "root",
        common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    receipt: dict[str, object] = {"probe": "persistent-write-evidence-stage", "result": "failed",
        "guardRoot": str(root), "gateSha256": {gate: hashlib.sha256(receipts[gate]).hexdigest() for gate in receipts}}
    write_json_0600_atomic(output, receipt)
    secret = common.transport.read_secret_stdin()
    try:
        with tempfile.TemporaryFile() as stream:
            stream.write(envelope)
            stream.seek(0)
            response = common.transport.run_ssh(target, secret, shlex.join(
                ["python3", "-B", str(root / "controller.py"), "evidence", "--guard-root", str(root)]),
                stdin=stream, timeout=120)
        common.require(response.returncode == 0 and len(response.stdout) <= 65536, "evidence staging failed")
        value = validate_device_result(json.loads(response.stdout), receipts, transaction)
        receipt.update(result="passed", sealSha256=value["sealSha256"], evidenceRoot=value["evidenceRoot"],
                       receipts=value["receipts"])
    finally:
        secret[:] = b"\0" * len(secret)
        write_json_0600_atomic(output, receipt)
    return 0 if receipt["result"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
