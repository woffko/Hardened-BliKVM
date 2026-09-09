import argparse
from contextlib import ExitStack, nullcontext
import json
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from tools import run_application_write_gate as runner


class RunnerTests(unittest.TestCase):
    def test_controller_receipt_requires_exact_reserved_root_and_closure(self) -> None:
        root = "/var/lib/" + runner.native.contract.PREFIX + "a" * 24
        runner.validate_device({"guardRoot": root, "result": "prepared", "phase": "wired", "deadlineArmed": True}, root, "prepared")
        for value in ({"guardRoot": root, "result": "restored", "floor": 8},
                      {"guardRoot": root, "result": "restored", "floor": True, "runtimeWritesClosed": True},
                      {"guardRoot": "foreign", "result": "restored", "floor": 8, "runtimeWritesClosed": True}):
            with self.assertRaises(ValueError):
                runner.validate_device(value, root, "restored")

    def run_flow(self, fail_browser: bool) -> tuple[int, list[str], dict[str, object]]:
        events: list[str] = []
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary) / "receipt.json"
            args = argparse.Namespace(old_package="old", new_package="new", bundle="bundle", endpoint="ssh://example.test:22",
                                      base_url="https://example.test", username="test", output=str(output))
            def ssh(_target: object, _secret: bytearray, script: str, **_kwargs: object) -> subprocess.CompletedProcess[bytes]:
                if script.startswith("python3 "):
                    argv = shlex.split(script)
                    action = argv[3]
                    root = argv[argv.index("--guard-root") + 1]
                    events.append(action)
                    receipt = json.loads(output.read_bytes())
                    self.assertEqual(receipt["guardRoot"], root)
                    self.assertTrue(receipt["prepareSubmitted"])
                    result: dict[str, object] = {"guardRoot": root, "result": "restored", "floor": 8,
                        "generation": 40, "runtimeWritesClosed": True}
                    if action == "prepare":
                        result.update(result="prepared", phase="wired", deadlineArmed=True)
                    elif action == "cleanup":
                        result.update(result="cleaned", guardRetained=False)
                    return subprocess.CompletedProcess([], 0, json.dumps(result).encode(), b"")
                return subprocess.CompletedProcess([], 0, b"", b"")
            def browser(argv: list[str], **_kwargs: object) -> subprocess.CompletedProcess[bytes]:
                mode = argv[2]
                events.append("browser-" + mode)
                if fail_browser and mode == "activate":
                    raise subprocess.TimeoutExpired("browser", 1)
                Path(argv[-1]).write_text(json.dumps({"mode": mode, "result": "passed"}))
                return subprocess.CompletedProcess([], 0, b"", b"")
            with ExitStack() as stack:
                stack.enter_context(patch.object(argparse.ArgumentParser, "parse_args", return_value=args))
                stack.enter_context(patch.object(runner, "build_uploads", return_value={"controller.py": b"public fixture"}))
                stack.enter_context(patch.object(runner.transport, "validate_output_path", return_value=output))
                stack.enter_context(patch.object(runner.transport, "validate_private_file", side_effect=Path))
                stack.enter_context(patch.object(runner.transport, "run_ssh", side_effect=ssh))
                stack.enter_context(patch.object(runner.subprocess, "run", side_effect=browser))
                stack.enter_context(patch.object(runner, "secret_pair_stdin", return_value=nullcontext({"ssh": bytearray(), "gui": bytearray()})))
                result = runner.main()
            return result, events, json.loads(output.read_bytes())

    def test_flow_requires_post_restore_browser_before_cleanup(self) -> None:
        code, events, receipt = self.run_flow(False)
        self.assertEqual(code, 0)
        self.assertEqual(events, ["prepare", "browser-activate", "finalize", "browser-closed", "cleanup"])
        self.assertIs(receipt["guardRetained"], False)

    def test_uncertain_browser_never_retries_activation_and_restores_known_root(self) -> None:
        code, events, receipt = self.run_flow(True)
        self.assertEqual(code, 1)
        self.assertEqual(events, ["prepare", "browser-activate", "restore"])
        self.assertIs(receipt["runtimeWritesClosed"], True)
        self.assertIs(receipt["guardRetained"], True)


if __name__ == "__main__":
    unittest.main()
