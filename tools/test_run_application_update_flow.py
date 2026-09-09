import base64
import contextlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
from typing import cast
import unittest
from unittest.mock import patch

from tools import run_application_update_flow as runner
from tools.capture_pikvm_baseline import write_json_0600_atomic
from tools.secret_pair_stdin import secret_pair_stdin
from tools.test_application_update_flow import response


class ApplicationFlowRunnerTests(unittest.TestCase):
    def test_real_receipt_paths_secret_roles_and_expired_normal_budget(self) -> None:
        for failure in ("none", "confirmed-browser", "budget"):
            with self.subTest(failure=failure), tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
                stack.enter_context(patch.object(runner.driver.native, "active_profile", runner.driver.native.active_profile))
                output = Path(temporary) / "application-flow.json"
                now = 0.0
                floor = 5
                events: list[str] = []
                secret_values = {"ssh": b"synthetic-ssh-input", "gui": b"synthetic-gui-input"}
                encoded = json.dumps({"schemaVersion": 1, "encoding": "base64", "secrets": {
                    key: base64.b64encode(value).decode() for key, value in secret_values.items()}}).encode()
                def child(argv: list[str], **kwargs: object) -> SimpleNamespace:
                    nonlocal now, floor
                    browser = argv[0] == "node"
                    self.assertEqual(kwargs["input"], secret_values["gui" if browser else "ssh"])
                    self.assertIs(kwargs["close_fds"], True)
                    self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
                    self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)
                    if browser:
                        phase = "rehearsal" if argv[-1].endswith("-rehearsal") else "confirmed"
                        events.append("browser-" + phase)
                        target = Path(argv[6])
                        value: dict[str, object] = {"probe": "hardened-blikvm-existing-account-browser", "result": "passed", "runTag": argv[7]}
                        value.update({name: True for name in ("videoDecoded", "streamRevoked", "secureSessionCookie", "secureCsrfCookie", "sessionCleared", "legacyBrandingAbsent")})
                        value.update({name: 200 for name in ("loginStatus", "inventoryStatus", "diagnosticsStatus", "videoStatus", "logoutStatus")})
                        value.update({"controlRequests": 0, "webSockets": 0, "secretEmitted": False})
                        failed = failure == "confirmed-browser" and phase == "confirmed"
                        if failed:
                            value["result"] = "failed"
                        write_json_0600_atomic(target, value)
                        return SimpleNamespace(returncode=1 if failed else 0)
                    action = argv[3]
                    events.append(action)
                    if action == "activate":
                        floor = 6
                    if action == "prepare" and failure == "budget":
                        now = 500
                    target = Path(argv[argv.index("--output") + 1])
                    write_json_0600_atomic(target, response(action, floor))
                    return SimpleNamespace(returncode=0)
                stack.enter_context(patch.object(sys, "argv", ["flow", "--old-package", "old", "--new-package", "new", "--bundle", "bundle",
                                                                "--endpoint", "ssh://synthetic:22", "--base-url", "https://synthetic", "--username", "admin", "--output", str(output)]))
                stack.enter_context(patch.object(runner.driver.transport, "validate_output_path", return_value=output))
                stack.enter_context(patch.object(runner, "secret_pair_stdin", return_value=secret_pair_stdin(io.BytesIO(encoded))))
                stack.enter_context(patch.object(runner.subprocess, "run", side_effect=child))
                stack.enter_context(patch.object(runner.time, "monotonic", side_effect=lambda: now))
                if failure == "none":
                    runner.main()
                    self.assertEqual(events, ["prepare", "browser-rehearsal", "activate", "browser-confirmed", "commit", "cleanup"])
                else:
                    with self.assertRaises(SystemExit):
                        runner.main()
                    self.assertEqual(events[-2:], ["recover", "cleanup"])
                    self.assertNotIn("commit", events)
                value = runner.read_receipt(output)
                self.assertIs(value["guardMayRemain"], False)
                self.assertEqual(value["observedFloor"], floor)
                paths = cast(dict[str, str], value["browserReceipts"])
                for name in paths.values():
                    self.assertEqual(Path(name).parent, output.parent)
                    self.assertTrue(Path(name).is_file())

    def test_receipt_parser_rejects_duplicates_nonobjects_and_oversize(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            for raw in (b'{"result":"failed","result":"passed"}', b'[]', b'x' * 131073):
                path.write_bytes(raw)
                path.chmod(0o600)
                with self.assertRaises(ValueError):
                    runner.read_receipt(path)


if __name__ == "__main__":
    unittest.main()
