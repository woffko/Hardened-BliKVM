#!/usr/bin/env python3
"""Exercise a signed application bundle through the host Rust supervisor.

All slot/journal writes are confined to a fresh temporary host directory.
No payload binary is executed and no device is contacted.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
from typing import cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from tools import build_production_update_bundle as builder
from tools.capture_pikvm_baseline import write_json_0600_atomic


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def run(binary: Path, config: Path, *args: str, rejected: bool = False) -> dict[str, object]:
    result = subprocess.run(
        [str(binary), "--config", str(config), *args],
        stdin=subprocess.DEVNULL, capture_output=True, timeout=15, check=False,
    )
    if rejected:
        require(result.returncode != 0, "invalid artifact unexpectedly accepted")
        return {"exitCode": result.returncode}
    require(result.returncode == 0, "Rust supervisor rejected the valid bundle flow")
    return cast(dict[str, object], json.loads(result.stdout))


def verify(binary: Path, data: bytes, request: dict[str, object]) -> dict[str, object]:
    with tempfile.TemporaryDirectory(prefix="blikvm-bundle-verification-", dir="/tmp") as temporary:
        root = Path(temporary)
        for name in ("slots", "state", "runtime"):
            (root / name).mkdir(mode=0o700)
        config_data = {
            "schemaVersion": 1, "product": builder.PRODUCT, "target": builder.TARGET,
            "keyId": request["keyId"], "publicKeyHex": request["publicKeyHex"],
            "slotRoot": str(root / "slots"), "stateRoot": str(root / "state"),
            "runtimeRoot": str(root / "runtime"), "systemSequence": 1, "healthTimeoutMs": 60000,
        }
        config = root / "config.json"
        builder.write_new(config, json.dumps(config_data).encode())
        source = root / "application.tar"
        builder.write_new(source, data)
        staged = run(binary, config, "stage", str(source))
        require(staged.get("result") == "published", "slot not published")
        require(staged.get("sequence") == 1, "expected initial sequence one")
        slot_id = cast(str, staged["slotId"])
        require(run(binary, config, "stage", str(source)).get("result") == "already_present",
                "staging is not idempotent")
        slot = root / "slots" / slot_id
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as archive:
            manifest_stream = archive.extractfile("manifest.json")
            require(manifest_stream is not None, "manifest missing")
            assert manifest_stream is not None
            manifest = cast(dict[str, object], json.loads(manifest_stream.read()))
            payload_entry = archive.getmember("payload/share/web/index.html")
        files = cast(list[dict[str, object]], manifest["files"])
        for item in files:
            path = slot / cast(str, item["path"])
            meta = path.stat()
            require(meta.st_uid == os.geteuid() and meta.st_nlink == 1, "slot ownership/link mismatch")
            require(stat.S_IMODE(meta.st_mode) == (cast(int, item["mode"]) & ~0o222), "slot not immutable")
            require(hashlib.sha256(path.read_bytes()).hexdigest() == item["sha256"], "slot hash mismatch")
        initialized = run(binary, config, "initialize", slot_id)
        require(initialized.get("result") == "initialized", "journal not initialized")
        status = run(binary, config, "status")
        for name, value in {
            "generation": 1, "floorSequence": 1, "activeSlotId": slot_id,
            "confirmedSlotId": slot_id, "previousSlotId": None, "pending": None,
        }.items():
            require(status.get(name) == value, "initial journal mismatch: " + name)
        require(os.readlink(root / "slots/current") == slot_id, "pointer mismatch")
        journal_before = (root / "state/application-state.json").read_bytes()
        run(binary, config, "initialize", slot_id, rejected=True)

        altered = bytearray(data)
        altered[payload_entry.offset_data] ^= 1
        tampered = root / "tampered.tar"
        builder.write_new(tampered, bytes(altered))
        tamper_result = run(binary, config, "stage", str(tampered), rejected=True)
        require(tamper_result["exitCode"] == 40, "tamper did not fail in stager")

        # Valid, fixed public test point; never a production signing key.
        config_data["publicKeyHex"] = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
        wrong_config = root / "wrong-key.json"
        builder.write_new(wrong_config, json.dumps(config_data).encode())
        wrong_key = run(binary, wrong_config, "stage", str(source), rejected=True)
        require(wrong_key["exitCode"] == 40, "wrong-key rejection did not reach stager")
        require(run(binary, config, "status") == status, "negative checks changed manager state")
        require((root / "state/application-state.json").read_bytes() == journal_before, "journal changed")
        require(not list((root / "runtime").iterdir()), "unexpected health challenge")
        return {
            "slotId": slot_id, "payloadFiles": len(files), "status": status,
            "idempotentStage": True, "repeatInitializeRejected": True,
            "payloadTamperRejected": True, "wrongKeyRejected": True,
            "negativeChecksPreservedState": True, "payloadExecuted": False,
        }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("bundle", "second-bundle", "expected-sha256", "request", "expected-request-sha256",
                 "key-id", "supervisor", "output"):
        parser.add_argument("--" + name, required=True)
    args = vars(parser.parse_args())
    values = cast(dict[str, str], args)
    first = Path(values["bundle"]).read_bytes()
    second = Path(values["second_bundle"]).read_bytes()
    require(first == second, "independent bundle builds differ")
    require(hashlib.sha256(first).hexdigest() == values["expected_sha256"], "bundle hash mismatch")
    enrollment = builder.load_enrollment(values["request"], values["expected_request_sha256"], values["key_id"])
    binary = Path(values["supervisor"]).resolve(strict=True)
    result = verify(binary, first, {"keyId": enrollment.key_id, "publicKeyHex": enrollment.public_key.hex()})
    receipt = {
        "schemaVersion": 1, "probe": "production-application-host-verification", "result": "passed",
        "bundleSha256": values["expected_sha256"], "requestSha256": enrollment.request_sha256,
        "keyId": enrollment.key_id, "supervisorSha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "independentBuildsIdentical": True, "temporaryRootsRemoved": True,
        "deviceContacted": False, "checks": result,
    }
    write_json_0600_atomic(Path(values["output"]), receipt)
    print(json.dumps(receipt, sort_keys=True))


if __name__ == "__main__":
    main()
