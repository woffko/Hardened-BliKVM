import ast
import contextlib
import copy
import json
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import capture_status_timer_firing as capture
from tools.test_status_timer_firing import handoff, OPERATION


def completion() -> dict[str, object]:
    return {"result": "passed", "operation": OPERATION, "guardMayRemain": False,
            "guardRemoved": True, "statusUnchanged": True, "servingIdentityUnchanged": True,
            "statusSocketNeverStopped": True, "completion": {"jobId": 123,
                "unit": capture.timer.policy.guard_name(OPERATION) + ".service", "result": "done",
                "receivedMonotonicUsec": 201_000_000, "timingSource": "observer-receive-clock"}}


class CaptureTests(unittest.TestCase):
    def test_both_bundles_import_without_running_device_actions(self) -> None:
        for phase in ("arm", "observe"):
            source, hashes = capture.program(OPERATION, phase, handoff())
            tree = ast.parse(source)
            boundary = next(i for i, node in enumerate(tree.body) if isinstance(node, ast.Assign)
                            and any(isinstance(target, ast.Name) and target.id == "result" for target in node.targets))
            code = ast.unparse(ast.Module(body=tree.body[:boundary], type_ignores=[]))
            answer = subprocess.run([sys.executable, "-B", "-c", code], capture_output=True, timeout=5)
            self.assertEqual(answer.returncode, 0, answer.stderr.decode())
            self.assertLessEqual(len(source.encode()), 100000)
            self.assertEqual(len(hashes), 12)

    def test_observer_runs_only_after_arm_exit_and_durable_handoff(self) -> None:
        for failed_arm in (False, True):
            events: list[str] = []
            receipts: list[dict[str, object]] = []
            secret = bytearray(b"synthetic-ssh-secret")
            output = Path("/tmp/unused-timer-receipt")
            def ssh(*_args: object, **_kwargs: object) -> SimpleNamespace:
                if not events:
                    events.append("arm-return")
                    value = ({"result": "failed", "operation": OPERATION, "phase": "arm",
                              "errorType": "ValueError", "guardMayRemain": False} if failed_arm else handoff())
                    return SimpleNamespace(returncode=1 if failed_arm else 0, stdout=json.dumps(value).encode())
                self.assertEqual(events, ["arm-return", "write"])
                self.assertTrue(receipts[0]["armingCommandExited"])
                events.append("observe")
                return SimpleNamespace(returncode=0, stdout=json.dumps(completion()).encode())
            def write(_path: Path, value: dict[str, object]) -> None:
                events.append("write")
                receipts.append(copy.deepcopy(value))
            transport = capture.timer.transport
            with contextlib.ExitStack() as stack:
                stack.enter_context(patch.object(sys, "argv", ["probe", "--endpoint", "synthetic", "--output", str(output)]))
                stack.enter_context(patch.object(transport, "validate_output_path", return_value=output))
                stack.enter_context(patch.object(Path, "exists", return_value=False))
                stack.enter_context(patch.object(transport, "parse_endpoint", return_value=("synthetic", 22)))
                stack.enter_context(patch.object(transport, "validate_private_file", return_value=Path("/tmp/synthetic-known-hosts")))
                stack.enter_context(patch.object(transport, "read_secret_stdin", return_value=secret))
                stack.enter_context(patch.object(transport, "run_ssh", side_effect=ssh))
                stack.enter_context(patch.object(capture.secrets, "token_hex", return_value=OPERATION))
                stack.enter_context(patch.object(capture, "program", return_value=("synthetic-source", {"module": "a" * 64})))
                stack.enter_context(patch.object(capture, "write_json_0600_atomic", side_effect=write))
                if failed_arm:
                    with self.assertRaises(SystemExit):
                        capture.main()
                else:
                    capture.main()
            self.assertEqual(events, ["arm-return", "write"] if failed_arm else ["arm-return", "write", "observe", "write"])
            self.assertFalse(receipts[-1]["guardMayRemain"])
            self.assertEqual(secret, bytearray(len(secret)))

    def test_completion_rejects_wrong_time_and_non_boolean_claims(self) -> None:
        self.assertEqual(capture.validate_completion(completion(), handoff()), completion())
        value = completion()
        value["guardRemoved"] = 1
        with self.assertRaises(ValueError):
            capture.validate_completion(value, handoff())
        value = completion()
        value["completion"] = {"jobId": 1, "unit": capture.timer.policy.guard_name(OPERATION) + ".service",
                               "result": "done", "receivedMonotonicUsec": 199_000_000,
                               "timingSource": "observer-receive-clock"}
        with self.assertRaises(ValueError):
            capture.validate_completion(value, handoff())


if __name__ == "__main__":
    unittest.main()
