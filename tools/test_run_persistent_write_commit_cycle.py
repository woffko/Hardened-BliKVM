import io
from contextlib import nullcontext
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from typing import cast
from unittest.mock import patch

from tools import persistent_write_contract as contract
from tools import run_persistent_write_commit_cycle as cycle
from tools.secret_pair_stdin import secret_pair_stdin

BINDING = contract.EvidenceBinding("a" * 24, "b" * 64, contract.CONFIG_SHA256,
    "app-00000000000000000010-" + "c" * 64, 10, 46)
RECEIPTS = {gate: ("d" * 63) + str(index % 10) for index, gate in enumerate(sorted(contract.REQUIRED_GATES))}


class CommitCycleTests(unittest.TestCase):
    def test_pair_envelope_round_trips_through_the_reviewed_reader(self) -> None:
        pair = {"ssh": bytearray(b"ssh-secret\n"), "gui": bytearray(b"gui-secret")}
        raw = cycle.pair_envelope(pair)
        with secret_pair_stdin(io.BytesIO(raw)) as decoded:
            self.assertEqual(decoded["ssh"], pair["ssh"])
            self.assertEqual(decoded["gui"], pair["gui"])
        self.assertNotIn(b"ssh-secret", raw)
        self.assertNotIn(b"gui-secret", raw)

    def test_commit_result_requires_the_exact_receipt_map(self) -> None:
        digests = {gate: "a" * 64 for gate in contract.REQUIRED_GATES}
        good = {"result": "committed", "retentionApproved": True, "gateReceiptHashes": digests}
        self.assertEqual(cycle.validate_commit_result(good, digests), good)
        for changed in ({**good, "result": "failed"}, {**good, "retentionApproved": False},
                        {**good, "gateReceiptHashes": {}}):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                cycle.validate_commit_result(changed, digests)

    def test_fresh_gate_files_cover_the_reviewed_gates(self) -> None:
        self.assertEqual(set(cycle.FRESH_GATE_FILES),
                         {"host", "native_upload", "gui_failure_retry", "gui_cancel", "gui_logout"})



    def test_historical_prerequisites_are_parsed_with_pinned_producers(self) -> None:
        def resolve(value: str) -> Path:
            return Path(value)

        probes = cast(dict[str, dict[str, str]], cycle.pins.CURRENT_PINS["probe_sources"])
        historical = cast(dict[str, dict[str, object]], cycle.pins.HISTORICAL_PINS)
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(cycle.common.transport, "validate_private_file", side_effect=resolve), \
                patch.object(cycle.activation_receipt, "parse") as activate, \
                patch.object(cycle.recovery_receipt, "parse") as recover:
            paths: list[Path] = []
            for name in ("activation.json", "boot.json", "pending.json"):
                path = Path(directory, name)
                path.write_bytes(b"{}")
                paths.append(path)
            cycle.validate_historical(paths[0], paths[1], paths[2])
        activate.assert_called_once()
        self.assertEqual(activate.call_args.kwargs["expected_runner"], historical["native_activation"]["runner"])
        self.assertEqual(recover.call_args_list[0].kwargs["expected_probe_sources"], probes["boot"])
        self.assertEqual(recover.call_args_list[1].kwargs["expected_probe_sources"], probes["pending"])



class PairStdin:
    def __enter__(self) -> dict[str, bytearray]:
        return {"ssh": bytearray(b"ssh-secret"), "gui": bytearray(b"gui-secret")}

    def __exit__(self, *_args: object) -> None:
        return None


def completed(argv: list[str], returncode: int = 0, stdout: bytes = b"") -> subprocess.CompletedProcess[bytes]:
    return subprocess.CompletedProcess(argv, returncode, stdout, b"")


class CommitCycleFlowTests(unittest.TestCase):
    def run_cycle(self, *, prepare_rc: int = 0) -> tuple[list[str], list[str], dict[str, object]]:
        def resolve(value: str) -> Path:
            return Path(value)

        ssh_calls: list[str] = []
        tool_calls: list[str] = []
        prepared = {"result": "prepared", "guardRoot": "", "phase": "wired", "binding": cycle.asdict(BINDING)}
        committed = {"result": "committed", "retentionApproved": True, "gateReceiptHashes": RECEIPTS}

        def fake_ssh(_target: object, _secret: object, script: str, **_kwargs: object) -> subprocess.CompletedProcess[bytes]:
            ssh_calls.append(script)
            if "prepare" in script:
                stdout = (json.dumps(prepared) + "\n").encode() if prepare_rc == 0 else b""
                return completed(["ssh"], prepare_rc, stdout)
            if "controller.py" in script and "commit" in script:
                return completed(["ssh"], 0, (json.dumps(committed) + "\n").encode())
            return completed(["ssh"])

        def fake_run(argv: list[str], **_kwargs: object) -> subprocess.CompletedProcess[bytes]:
            name = next((part for part in argv if part.endswith(".py") and "capture" in part or part.endswith("stage_persistent_write_evidence.py")), "")
            tool_calls.append(name)
            output = Path(argv[argv.index("--output") + 1])
            if "stage_persistent_write_evidence" in name:
                output.write_bytes((json.dumps({"sealSha256": "e" * 64, "receipts": RECEIPTS})).encode())
            else:
                output.write_bytes(b'{"probe": "fake"}')
            return completed(argv)

        def fake_uploads(_old: str, _new: str) -> dict[str, bytes]:
            return {"bindings.json": b"{}", "new.deb": b"pkg", "installed_persistent_write.py": b"ctrl"}

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "commit.json"
            argv = ["run_persistent_write_commit_cycle.py", "--endpoint", "ssh://h:22", "--old-package", "old.deb",
                    "--new-package", "new.deb", "--bundle", "app.tar", "--base-url", "https://h/", "--username", "admin",
                    "--application-profile", "seq10", "--activation-receipt", "a.json", "--boot-receipt", "b.json",
                    "--pending-receipt", "c.json", "--output", str(output)]
            with patch.object(sys, "argv", argv), \
                    patch.object(cycle.common.transport, "parse_endpoint", return_value=("h", 22)), \
                    patch.object(cycle.common.transport, "validate_output_path", return_value=output), \
                    patch.object(cycle.common.transport, "validate_private_file", side_effect=resolve), \
                    patch.object(cycle.common.transport, "RemoteTarget"), \
                    patch.object(cycle.common.transport, "run_ssh", side_effect=fake_ssh), \
                    patch.object(cycle.common, "build_uploads", side_effect=fake_uploads), \
                    patch.object(cycle, "validate_historical"), \
                    patch.object(cycle, "secret_pair_stdin", PairStdin), \
                    patch.object(cycle.subprocess, "run", side_effect=fake_run):
                with self.assertRaises(ValueError) if prepare_rc != 0 else nullcontext():
                    cycle.main()
            receipt = json.loads(output.read_bytes())
        return ssh_calls, tool_calls, receipt

    def test_commit_cycle_runs_pinned_steps_and_retains(self) -> None:
        ssh_calls, tool_calls, receipt = self.run_cycle()
        self.assertEqual([Path(name).name for name in tool_calls],
                         ["capture_persistent_write_host_gate.py", "capture_persistent_write_upload_gate.py",
                          "capture_persistent_write_gui_gate.py", "stage_persistent_write_evidence.py"])
        self.assertTrue(any("prepare" in script for script in ssh_calls))
        self.assertTrue(any("commit" in script for script in ssh_calls))
        self.assertTrue(any("rmdir" in script for script in ssh_calls))
        self.assertEqual(receipt["result"], "passed")
        self.assertTrue(receipt["retentionApproved"])
        self.assertTrue(receipt["guardRetained"])
        self.assertFalse(receipt["runtimeWritesClosed"])
        commit = cast(dict[str, object], receipt["commit"])
        self.assertEqual(commit["gateReceiptHashes"], RECEIPTS)

    def test_commit_cycle_failure_restores_and_closes(self) -> None:
        ssh_calls, _tool_calls, receipt = self.run_cycle(prepare_rc=1)
        joined = "\n".join(ssh_calls)
        self.assertIn("restore", joined)
        self.assertIn("cleanup", joined)
        self.assertEqual(receipt["result"], "failed")
        self.assertTrue(receipt["runtimeWritesClosed"])
        self.assertFalse(receipt["guardRetained"])


if __name__ == "__main__":
    unittest.main()
