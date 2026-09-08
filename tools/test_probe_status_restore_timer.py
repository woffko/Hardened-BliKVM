import ast
import contextlib
import copy
import json
import subprocess
import sys
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import probe_status_restore_timer as probe
from tools import capture_status_restore_timer as capture
from tools import status_outage_contract as policy
from tools.test_status_restore_properties import fixture, OPERATION
from tools.test_retained_resource_baseline import fixture as resource_fixture


class ProbeTests(unittest.TestCase):
    def test_success_failure_and_collection_paths_never_stop_status_or_restorer(self) -> None:
        for failure in ("none", "arm", "metadata", "unowned", "restore", "identity", "collected", "collect-race"):
            with self.subTest(failure=failure):
                timer, service = fixture()
                if failure in ("metadata", "unowned"):
                    timer["Unit"]["data"] = "foreign.service"
                snapshots = [copy.deepcopy(resource_fixture()[0]) for _ in range(3)]
                if failure == "identity":
                    snapshots[1]["units"][probe.resources.UNITS[0]]["mainPid"] += 1
                present: set[str] = set()
                calls: list[list[str]] = []
                status_calls = 0
                name = policy.guard_name(OPERATION)
                def command(argv: list[str], timeout: int = 5) -> bytes:
                    calls.append(argv)
                    if argv[0] == "systemd-run":
                        if failure == "arm":
                            raise subprocess.CalledProcessError(1, argv)
                        present.update({name + ".timer", name + ".service"})
                    elif argv[:2] == ["systemctl", "start"]:
                        self.assertEqual(argv[2:], [policy.SOCKET])
                        if failure == "restore":
                            raise subprocess.CalledProcessError(1, argv)
                    else:
                        self.assertEqual(argv, ["systemctl", "stop", name + ".timer"])
                        present.clear()
                        if failure == "collect-race":
                            raise subprocess.CalledProcessError(1, argv)
                    return b""
                def status_read(_source: str) -> dict[str, object]:
                    nonlocal status_calls
                    status_calls += 1
                    if failure == "collected" and status_calls == 2:
                        present.clear()
                    return {"fixed": "synthetic-status"}
                def absent(unit: str) -> bool:
                    return unit not in present
                with contextlib.ExitStack() as stack:
                    stack.enter_context(patch.object(probe.os, "geteuid", return_value=0))
                    stack.enter_context(patch.object(probe.os, "uname", return_value=SimpleNamespace(machine="aarch64")))
                    stack.enter_context(patch.object(probe.resources, "snapshot", side_effect=snapshots))
                    stack.enter_context(patch.object(probe, "status_read", side_effect=status_read))
                    stack.enter_context(patch.object(probe, "quiet_socket"))
                    stack.enter_context(patch.object(probe, "namespace_pristine"))
                    stack.enter_context(patch.object(probe, "own_timer_identity", return_value=failure != "unowned"))
                    stack.enter_context(patch.object(probe, "command", side_effect=command))
                    stack.enter_context(patch.object(probe, "unit_absent", side_effect=absent))
                    stack.enter_context(patch.object(probe, "properties", side_effect=[timer, service]))
                    stack.enter_context(patch.object(probe.time, "monotonic_ns", return_value=100_000_000_000))
                    value = probe.probe(OPERATION, "synthetic reader")
                capture.validate(json.dumps(value).encode(), OPERATION)
                if failure in ("none", "collected", "collect-race"):
                    self.assertEqual(value["result"], "passed")
                    self.assertFalse(value["guardMayRemain"])
                    self.assertEqual(status_calls, 3)
                else:
                    self.assertEqual(value["result"], "failed")
                    self.assertEqual(value["guardMayRemain"], failure == "unowned")
                self.assertTrue(calls)
                self.assertNotIn(["systemctl", "stop", policy.SOCKET], calls)
                self.assertNotIn(["systemctl", "stop", name + ".service"], calls)

    def test_namespace_rejects_global_prefix_exact_dropins_and_dependency_links(self) -> None:
        name = policy.guard_name(OPERATION)
        for candidate in ("service.d", "timer.d", "hardened-.service.d",
                          "hardened-blikvm-status-outage-.timer.d", name + ".service.d",
                          name + ".timer.wants", name + ".service.requires", name + ".timer"):
            with self.subTest(candidate=candidate), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                with patch.object(probe, "command", return_value=json.dumps({"type": "as", "data": [directory]}).encode()):
                    probe.namespace_pristine(OPERATION)
                    (root / candidate).symlink_to(root / "missing-target")
                    with self.assertRaises(ValueError):
                        probe.namespace_pristine(OPERATION)

    def test_bundle_bootstraps_without_probe_execution(self) -> None:
        source, hashes = capture.program(OPERATION)
        tree = ast.parse(source)
        bootstrap = ast.unparse(ast.Module(body=tree.body[:-3], type_ignores=[]))
        result = subprocess.run([sys.executable, "-B", "-c", bootstrap], capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(len(hashes), 7)

    def test_receipt_rejects_wrong_identity_and_forged_success(self) -> None:
        value: dict[str, object] = {"result": "passed", "operation": OPERATION, "guardVerified": True,
                                   "guardRemoved": True, "statusUnchanged": True, "servingIdentityUnchanged": True,
                                   "statusSocketNeverStopped": True, "initialGuardMarginSeconds": 100,
                                   "guardMayRemain": False}
        for key, wrong in (("operation", "foreign"), ("guardVerified", 1), ("guardMayRemain", True),
                           ("initialGuardMarginSeconds", 59), ("statusSocketNeverStopped", False)):
            altered = dict(value)
            altered[key] = wrong
            with self.assertRaises(ValueError):
                capture.validate(json.dumps(altered).encode(), OPERATION)


if __name__ == "__main__":
    unittest.main()
