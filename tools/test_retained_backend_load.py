import ast
import contextlib
import copy
import io
import json
import socket
import subprocess
import sys
import time
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch
from typing import Any

from tools import observe_retained_backend_load as load
from tools import capture_retained_backend_load as capture
from tools import observe_service_resources as resources
from tools.test_retained_resource_baseline import fixture as resource_fixture


def status() -> dict[str, Any]:
    selected = {"slotId": resources.SLOT, "version": "0.1.0-dev4", "sequence": 5, "minimumSystemSequence": 1}
    return {"generation": 22, "floorSequence": 5, "systemSequence": 1,
            "selected": selected, "confirmed": dict(selected), "pending": None, "lastRollback": None,
            "previous": {"slotId": "app-00000000000000000004-" + "a" * 64,
                         "version": "0.1.0-dev3", "sequence": 4, "minimumSystemSequence": 1}}


def samples() -> list[dict[str, Any]]:
    return [{"result": "sample", "request": index + 1, "status": status(),
             "statusDurationNs": 100000, "videoBytes": (index + 1) * 4096}
            for index in range(resources.SAMPLES)]


class BackendTests(unittest.TestCase):
    def test_loaded_receipt_recomputes_both_summaries_and_requires_cleanup(self) -> None:
        observed, workload = resource_fixture(), samples()
        value: dict[str, Any] = {"result": "observed", "workload": load.WORKLOAD,
                               "samples": observed, "summary": resources.summarize(observed),
                               "loadSamples": workload, "loadSummary": load.validate_load(workload),
                               "workloadChildExited": True}
        self.assertEqual(capture.validate(json.dumps(value).encode()), value)
        invalid: tuple[tuple[str, object], ...] = (("workloadChildExited", 1), ("loadSummary", {}), ("summary", {}),
                                                  ("workload", "no-observer-generated-workload"), ("loadSamples", []))
        for key, bad in invalid:
            altered = dict(value, **{key: bad})
            with self.subTest(key=key), self.assertRaises(ValueError):
                capture.validate(json.dumps(altered).encode())

    def test_state_counters_slot_transactions_and_previous_metadata_fail_closed(self) -> None:
        for kind in ("bool", "slot", "version", "generation", "pending", "previous", "extra", "minimum"):
            value = status()
            if kind == "bool":
                value["systemSequence"] = True
            elif kind == "slot":
                value["selected"]["slotId"] = "foreign"
            elif kind == "version":
                value["selected"]["version"] = "foreign"
            elif kind == "generation":
                value["generation"] = 23
            elif kind == "pending":
                value["pending"] = {}
            elif kind == "previous":
                value["previous"] = None
            elif kind == "minimum":
                value["confirmed"]["minimumSystemSequence"] = True
            else:
                value["extra"] = 1
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                load.retained_status(value)

    def test_workload_requires_all_reads_video_progress_and_identical_previous(self) -> None:
        self.assertEqual(load.validate_load(samples())["statusReads"], 61)
        for kind in ("count", "request", "video", "duration", "previous", "extra", "bool"):
            values = samples()
            if kind == "count":
                values.pop()
            elif kind == "request":
                values[1]["request"] = 1
            elif kind == "video":
                values[1]["videoBytes"] = values[0]["videoBytes"]
            elif kind == "duration":
                values[1]["statusDurationNs"] = 5_000_000_001
            elif kind == "previous":
                values[1]["status"]["previous"]["version"] = "changed"
            elif kind == "bool":
                values[0]["request"] = True
            else:
                values[1]["extra"] = 1
            with self.subTest(kind=kind), self.assertRaises(ValueError):
                load.validate_load(values)

    def test_bundled_parent_and_child_import_without_device_files(self) -> None:
        source, hashes = capture.program()
        tree = ast.parse(source)
        final = tree.body[-1]
        self.assertIsInstance(final, ast.Expr)
        assert isinstance(final, ast.Expr) and isinstance(final.value, ast.Call)
        worker = ast.literal_eval(final.value.args[0])
        self.assertEqual(set(hashes), {"tools.observe_service_resources", "tools.retained_load_ipc",
                                      "tools.observe_retained_backend_load"})
        for program in (source, worker):
            parsed = ast.parse(program)
            bootstrap = ast.unparse(ast.Module(body=parsed.body[:-1], type_ignores=[]))
            result = subprocess.run([sys.executable, "-B", "-c", bootstrap], capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(result.stdout, b"")

    def test_video_reader_discards_payload_and_joins_on_close(self) -> None:
        client, server = socket.socketpair()
        reader = None
        try:
            with patch.object(load.ipc, "open_video", return_value=client):
                reader = load.VideoReader()
            server.sendall(b"frame-transport-only")
            self.assertEqual(reader.sample(), 20)
            server.close()
            deadline = time.monotonic() + 2
            while not reader.failed and time.monotonic() < deadline:
                time.sleep(0.01)
            with self.assertRaises(ValueError):
                reader.sample()
        finally:
            if reader is not None:
                reader.close()
                self.assertFalse(reader.thread.is_alive())
            client.close()
            server.close()

    def test_parent_drops_privileges_and_kills_only_owned_child_on_failure(self) -> None:
        for mode in ("success", "terminate", "kill"):
            failure = mode != "success"
            child = MagicMock()
            child.poll.side_effect = [None] * 61 + [0] if not failure else [None, None]
            child.wait.return_value = 0
            if mode == "kill":
                child.wait.side_effect = [subprocess.TimeoutExpired("owned workload", 3), 0]
            output = io.StringIO()
            replies = copy.deepcopy(samples())
            if failure:
                replies[0] = {"result": "failed"}
            observed = resource_fixture()
            with contextlib.ExitStack() as stack:
                stack.enter_context(patch.object(load.os, "geteuid", return_value=0))
                stack.enter_context(patch.object(load.os, "uname", return_value=SimpleNamespace(machine="aarch64")))
                stack.enter_context(patch.object(load.pwd, "getpwnam", return_value=SimpleNamespace(pw_uid=1001)))
                stack.enter_context(patch.object(load.grp, "getgrnam", return_value=SimpleNamespace(gr_gid=1002)))
                spawn = stack.enter_context(patch.object(load.subprocess, "Popen", return_value=child))
                stack.enter_context(patch.object(load, "read_reply", side_effect=replies))
                stack.enter_context(patch.object(load.resources, "snapshot", side_effect=[observed[0], *observed]))
                stack.enter_context(patch.object(load.time, "sleep"))
                stack.enter_context(contextlib.redirect_stdout(output))
                if failure:
                    with self.assertRaises(SystemExit):
                        load.main("reviewed worker")
                else:
                    load.main("reviewed worker")
            self.assertEqual(spawn.call_args.kwargs["user"], 1001)
            self.assertEqual(spawn.call_args.kwargs["group"], 1002)
            self.assertEqual(spawn.call_args.kwargs["extra_groups"], [])
            self.assertEqual(spawn.call_args.kwargs["env"], {})
            child.stdin.close.assert_called()
            child.stdout.close.assert_called_once()
            result = json.loads(output.getvalue())
            if failure:
                child.terminate.assert_called_once()
                self.assertEqual(result["result"], "failed")
                self.assertEqual(result["phase"], "workload")
                if mode == "kill":
                    child.kill.assert_called_once()
                else:
                    child.kill.assert_not_called()
            else:
                child.terminate.assert_not_called()
                self.assertTrue(capture.validate(output.getvalue().encode())["workloadChildExited"])

    def test_child_reply_is_bounded_and_rejects_eof_and_duplicate_json(self) -> None:
        for data in (b"", b"x" * (load.MAX_REPLY + 1), b'{"result":1,"result":2}\n'):
            stream = MagicMock()
            with patch.object(load.select, "select", return_value=([stream], [], [])), \
                    patch.object(load.os, "read", return_value=data), self.assertRaises(ValueError):
                load.read_reply(stream)


if __name__ == "__main__":
    unittest.main()
