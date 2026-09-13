#!/usr/bin/env python3
"""Run the reviewed permanent-admission commit cycle inside one guard window."""

import argparse
from dataclasses import asdict
import hashlib
import json
from pathlib import Path
import secrets
import shlex
import subprocess
import sys
import tempfile
from typing import IO, cast

if not __package__:
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from tools import persistent_write_activation_receipt as activation_receipt
from tools import persistent_write_evidence_pins as pins
from tools import persistent_write_recovery_receipt as recovery_receipt
from tools import run_persistent_write_rehearsal as common
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin

FRESH_GATE_FILES = {"host": "host", "native_upload": "upload", "gui_failure_retry": "gui",
                    "gui_cancel": "gui", "gui_logout": "gui"}


def pair_envelope(pair: dict[str, bytearray]) -> bytes:
    """Render the reviewed two-secret stdin envelope from held buffers."""
    import base64
    secrets_value = {role: base64.b64encode(bytes(value)).decode("ascii") for role, value in pair.items()}
    return json.dumps({"schemaVersion": 1, "encoding": "base64", "secrets": secrets_value},
                      sort_keys=True, separators=(",", ":")).encode() + b"\n"


def validate_commit_result(value: object, expected_receipts: object) -> dict[str, object]:
    result = common.native.object_value(value)
    common.require(result.get("result") == "committed" and result.get("retentionApproved") is True
                   and result.get("gateReceiptHashes") == expected_receipts, "permanent commit was not confirmed")
    return result


def validate_historical(activation: Path, boot: Path, pending: Path) -> None:
    """Fail fast on unreadable historical prerequisites before any device change."""
    historical = pins.HISTORICAL_PINS.get("native_activation")
    common.require(isinstance(historical, dict), "historical activation pins missing")
    entry = cast(dict[str, object], historical)
    activation_receipt.parse(common.transport.validate_private_file(str(activation)).read_bytes(),
                             expected_runner=cast(str, entry["runner"]),
                             expected_browser_sources=cast(dict[str, str], entry["browser_sources"]))
    probes = cast(dict[str, dict[str, str]], pins.CURRENT_PINS["probe_sources"])
    for action, path in (("boot", boot), ("pending", pending)):
        recovery_receipt.parse(common.transport.validate_private_file(str(path)).read_bytes(), action=action,
                               expected_runner=cast(str, pins.CURRENT_PINS["runner"]),
                               expected_probe_sources=probes[action])


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("endpoint", "old-package", "new-package", "bundle", "base-url", "username", "output",
                 "activation-receipt", "boot-receipt", "pending-receipt"):
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--application-profile", choices=("seq9", "seq10"), default="seq10")
    args = parser.parse_args()
    host, port = common.transport.parse_endpoint(args.endpoint)
    output = common.transport.validate_output_path(args.output)
    common.require(not output.exists(), "new commit cycle receipt required")
    target = common.transport.RemoteTarget(host, port, "root",
        common.transport.validate_private_file("evidence/private/ssh-known-hosts"))
    activation = Path(args.activation_receipt)
    boot_receipt = Path(args.boot_receipt)
    pending_receipt = Path(args.pending_receipt)
    validate_historical(activation, boot_receipt, pending_receipt)
    uploads = common.build_uploads(args.old_package, args.new_package)
    run_id = secrets.token_hex(12)
    root = Path("/var/lib/hardened-blikvm-write-retain-" + run_id)
    stage = "/tmp/blikvm-write-commit-" + run_id
    binding_file = Path("/tmp/blikvm-write-commit-" + run_id + "-binding.json")
    q = shlex.quote
    receipt: dict[str, object] = {"probe": "persistent-write-commit-cycle", "result": "failed",
        "guardRoot": str(root), "stageRoot": stage, "applicationProfile": args.application_profile,
        "prepareSubmitted": False, "runtimeWritesClosed": False, "guardRetained": False,
        "retentionApproved": False, "targetPowerControl": False,
        "sourceHashes": {name: hashlib.sha256(data).hexdigest() for name, data in uploads.items()}}
    write_json_0600_atomic(output, receipt)

    with secret_pair_stdin() as pair:
        ssh_secret = bytes(pair["ssh"])

        def ssh(script: str, timeout: float = 60,
                stdin: int | IO[bytes] = subprocess.DEVNULL) -> subprocess.CompletedProcess[bytes]:
            return common.transport.run_ssh(target, bytearray(ssh_secret), script, timeout=timeout, stdin=stdin)

        def tool(argv: list[str], role: bytes, timeout: float = 600) -> int:
            return subprocess.run(argv, input=role, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                  timeout=timeout, check=False).returncode

        try:
            common.require(ssh(f"set -eu\ntest ! -e {q(stage)} && test ! -L {q(stage)}\nmkdir -m 700 {q(stage)}").returncode == 0,
                           "commit staging failed")
            for name, data in uploads.items():
                remote = stage + "/" + name
                script = (f"set -eu\numask 077\ncat > {q(remote)}\n"
                          f"test \"$(sha256sum {q(remote)} | cut -d ' ' -f 1)\" = {hashlib.sha256(data).hexdigest()}")
                with tempfile.TemporaryFile() as stream:
                    stream.write(data)
                    stream.seek(0)
                    common.require(common.transport.run_ssh(target, bytearray(ssh_secret), script,
                                                            stdin=stream, timeout=60).returncode == 0,
                                   "commit asset upload failed")
            receipt["prepareSubmitted"] = True
            write_json_0600_atomic(output, receipt)
            response = ssh(shlex.join(["python3", "-B", stage + "/installed_persistent_write.py", "prepare",
                                       "--guard-root", str(root), "--assets", stage,
                                       "--application-profile", args.application_profile]), timeout=260)
            common.require(response.returncode == 0 and len(response.stdout) <= 65536, "commit preparation failed")
            prepared = common.native.object_value(json.loads(response.stdout))
            binding = common.native.binding({"binding": prepared.get("binding")})
            write_json_0600_atomic(binding_file, {"binding": asdict(binding)})
            receipt["prepared"] = prepared
            receipt["binding"] = asdict(binding)
            write_json_0600_atomic(output, receipt)

            produced: dict[str, Path] = {}
            for name, suffix in (("host", "host"), ("upload", "upload"), ("gui", "gui")):
                produced[name] = output.with_name(output.stem + "-" + suffix + ".json")
            if tool([sys.executable, "-B", "tools/capture_persistent_write_host_gate.py",
                     "--binding-receipt", str(binding_file), "--output", str(produced["host"])], b"") != 0:
                raise ValueError("host gate failed")
            receipt["host"] = json.loads(produced["host"].read_bytes())
            write_json_0600_atomic(output, receipt)
            if tool([sys.executable, "-B", "tools/capture_persistent_write_upload_gate.py", "--endpoint", args.endpoint,
                     "--guard-root", str(root), "--bundle", args.bundle, "--binding-receipt", str(binding_file),
                     "--output", str(produced["upload"])], ssh_secret) != 0:
                raise ValueError("native upload gate failed")
            receipt["upload"] = json.loads(produced["upload"].read_bytes())
            write_json_0600_atomic(output, receipt)
            if tool([sys.executable, "-B", "tools/capture_persistent_write_gui_gate.py", "--endpoint", args.endpoint,
                     "--base-url", args.base_url, "--username", args.username, "--guard-root", str(root),
                     "--bundle", args.bundle, "--binding-receipt", str(binding_file),
                     "--output", str(produced["gui"])], pair_envelope(pair)) != 0:
                raise ValueError("negative GUI gate failed")
            receipt["gui"] = json.loads(produced["gui"].read_bytes())
            write_json_0600_atomic(output, receipt)

            evidence_file = output.with_name(output.stem + "-evidence.json")
            evidence_argv = [sys.executable, "-B", "tools/stage_persistent_write_evidence.py", "--endpoint",
                             args.endpoint, "--guard-root", str(root), "--output", str(evidence_file)]
            for gate, suffix in FRESH_GATE_FILES.items():
                evidence_argv += ["--gate", gate + "=" + str(produced[suffix])]
            evidence_argv += ["--gate", "native_activation=" + str(activation), "--gate", "gui_happy_path=" + str(activation),
                              "--gate", "boot_restore=" + str(boot_receipt), "--gate", "disable_drain=" + str(pending_receipt)]
            if tool(evidence_argv, ssh_secret) != 0:
                raise ValueError("evidence sealing failed")
            evidence = common.native.object_value(json.loads(evidence_file.read_bytes()))
            receipt["evidence"] = evidence
            write_json_0600_atomic(output, receipt)

            commit_response = ssh(shlex.join(["python3", "-B", str(root / "controller.py"), "commit", "--guard-root",
                                              str(root), "--evidence-seal", str(evidence["sealSha256"]),
                                              "--approve-retention"]), timeout=120)
            common.require(commit_response.returncode == 0 and len(commit_response.stdout) <= 65536,
                           "permanent commit failed")
            receipt["commit"] = validate_commit_result(json.loads(commit_response.stdout), evidence["receipts"])
            receipt.update(result="passed", retentionApproved=True, guardRetained=True, runtimeWritesClosed=False)
            write_json_0600_atomic(output, receipt)
            return 0
        except (OSError, ValueError, KeyError, subprocess.SubprocessError):
            if receipt.get("prepareSubmitted") is True and receipt.get("guardRetained") is not True:
                try:
                    ssh(shlex.join(["python3", "-B", str(root / "controller.py"), "restore", "--guard-root", str(root)]),
                        timeout=300)
                    ssh(shlex.join(["python3", "-B", str(root / "controller.py"), "cleanup", "--guard-root", str(root)]),
                        timeout=180)
                    receipt.update(runtimeWritesClosed=True, guardRetained=False)
                except (OSError, ValueError, subprocess.SubprocessError):
                    receipt["independentInspectionRequired"] = True
            raise
        finally:
            try:
                paths = " ".join(q(stage + "/" + name) for name in uploads)
                ssh(f"set -eu\ntest ! -L {q(stage)}\nrm -f -- {paths}\nrmdir {q(stage)}")
                receipt["controllerStagingRemoved"] = True
            except (OSError, ValueError, subprocess.SubprocessError):
                receipt["controllerStagingRemoved"] = False
            if binding_file.exists():
                binding_file.unlink()
            write_json_0600_atomic(output, receipt)


if __name__ == "__main__":
    raise SystemExit(main())
