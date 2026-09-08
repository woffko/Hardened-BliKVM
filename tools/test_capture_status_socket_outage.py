import ast
import contextlib
import copy
import io
import json
from pathlib import Path
import shlex
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from tools import capture_status_socket_outage as capture
from tools.test_status_timer_firing import OPERATION, handoff


class CaptureOutageTests(unittest.TestCase):
    def test_bootstrap_imports_without_executing_device_phases(self) -> None:
        for phase in ("arm", "stop", "restore", "finish"):
            source, hashes = capture.program(OPERATION, phase, handoff())
            tree = ast.parse(source)
            boundary = next(i for i, node in enumerate(tree.body) if isinstance(node, ast.Assign)
                            and any(isinstance(target, ast.Name) and target.id == "result" for target in node.targets))
            code = ast.unparse(ast.Module(body=tree.body[:boundary], type_ignores=[]))
            result = subprocess.run([sys.executable, "-B", "-c", code], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(len(hashes), 12)
            self.assertLess(len(source.encode()), 110000)

    def test_handoff_before_stop_logout_before_finish_and_failure_recovery(self) -> None:
        for failure in ("none", "arm", "stop", "restore", "finish", "browser"):
            with self.subTest(failure=failure), contextlib.ExitStack() as stack:
                events: list[str] = []
                receipts: list[dict[str, object]] = []
                output = Path("/tmp/synthetic-outage-receipt")
                browser = MagicMock()
                browser.stdin = io.BytesIO()
                browser.poll.return_value = None
                def wait(**_kwargs: object) -> int:
                    events.append("browser-exit")
                    result = 1 if failure == "browser" else 0
                    browser.poll.return_value = result
                    return result
                browser.wait.side_effect = wait
                channel = MagicMock()
                channel.receive.side_effect = [(1, "stop"), (2, "restore"), None]
                facts = {key: True for key in ("staleVerified", "coldErrorVerified", "recoveryVerified", "videoPreserved")}
                browser_receipt = {"result": "passed", "runTag": "hardened-blikvm-status-api-" + OPERATION + "-enabled",
                                   "statusOutageAcceptance": facts}
                def write(_path: Path, value: dict[str, object]) -> None:
                    events.append("write")
                    receipts.append(copy.deepcopy(value))
                def ssh(_target: object, _secret: bytearray, command: str, **_kwargs: object) -> SimpleNamespace:
                    phase = "emergency" if command.startswith("systemctl start ") else shlex.split(command)[-1]
                    events.append(phase)
                    if phase == "stop":
                        self.assertEqual(receipts[-1]["handoff"], handoff())
                        self.assertIs(receipts[-1]["guardMayRemain"], True)
                    if phase == "finish":
                        self.assertIn("browser-exit", events)
                    if phase == failure:
                        return SimpleNamespace(returncode=1, stdout=b"")
                    if phase == "arm":
                        value = handoff()
                    elif phase == "emergency":
                        self.assertEqual(command, shlex.join(capture.timer.policy.restore_command()))
                        value = {}
                    else:
                        value = {"operation": OPERATION, "result": {"stop": "stopped", "restore": "restored", "finish": "finished"}[phase],
                                 "guardMayRemain": phase != "finish", "servingIdentityUnchanged": True,
                                 ("workersDrained" if phase == "stop" else "statusUnchanged"): True}
                    return SimpleNamespace(returncode=0, stdout=json.dumps(value).encode())
                transport = capture.timer.transport
                def program(_op: str, phase: str, _state: object = None) -> tuple[str, dict[str, str]]:
                    return phase, {"fixed": "hash"}
                stack.enter_context(patch.object(sys, "argv", ["probe", "--endpoint", "synthetic", "--base-url", "https://synthetic",
                                                                "--username", "acceptance", "--output", str(output)]))
                stack.enter_context(patch.object(transport, "validate_output_path", return_value=output))
                stack.enter_context(patch.object(transport, "validate_private_file", return_value=Path("/tmp/synthetic-known-hosts")))
                stack.enter_context(patch.object(transport, "parse_endpoint", return_value=("synthetic", 22)))
                stack.enter_context(patch.object(transport, "run_ssh", side_effect=ssh))
                stack.enter_context(patch.object(capture, "program", side_effect=program))
                stack.enter_context(patch.object(capture, "secret_pair_stdin", return_value=contextlib.nullcontext(
                    {"ssh": bytearray(b"synthetic-ssh-secret"), "gui": bytearray(b"synthetic-gui-secret")})))
                stack.enter_context(patch.object(capture.secrets, "token_hex", return_value=OPERATION))
                stack.enter_context(patch.object(capture, "write_json_0600_atomic", side_effect=write))
                stack.enter_context(patch.object(capture, "PhaseChannel", return_value=channel))
                stack.enter_context(patch.object(capture.subprocess, "Popen", return_value=browser))
                stack.enter_context(patch.object(Path, "exists", return_value=False))
                stack.enter_context(patch.object(Path, "stat", return_value=SimpleNamespace(st_size=100)))
                stack.enter_context(patch.object(Path, "read_bytes", return_value=json.dumps(browser_receipt).encode()))
                if failure == "none":
                    capture.main()
                    self.assertEqual(receipts[-1]["result"], "passed")
                    self.assertIs(receipts[-1]["guardMayRemain"], False)
                    self.assertNotIn("emergency", events)
                else:
                    with self.assertRaises(SystemExit):
                        capture.main()
                    self.assertEqual(receipts[-1]["result"], "failed")
                    self.assertIs(receipts[-1]["guardMayRemain"], True)
                    self.assertEqual(events.count("emergency"), 1)
                    self.assertLessEqual(events.count("stop"), 1)
                    if failure != "finish":
                        self.assertNotIn("finish", events)


if __name__ == "__main__":
    unittest.main()
