import ast
import contextlib
import io
import json
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from tools import capture_subscription_preflight as capture
from tools import subscription_process, status_restore_monitor

OPERATION = "a" * 24


class PreflightTests(unittest.TestCase):
    def test_bundle_imports_without_connecting(self) -> None:
        source, hashes = capture.program(OPERATION)
        tree = ast.parse(source)
        # Only the generated module bootstrap, not the following remote probe.
        boundary = next(index for index, node in enumerate(tree.body)
                        if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name)
                           and target.id == "worker_source" for target in node.targets))
        bootstrap = ast.unparse(ast.Module(body=tree.body[:boundary], type_ignores=[]))
        result = subprocess.run([sys.executable, "-B", "-c", bootstrap], capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(len(hashes), 6)

    def test_fixed_owner_reads_and_no_timer_mutations(self) -> None:
        for failed in (False, True):
            subscription = MagicMock()
            subscription.__enter__.return_value = subscription
            subscription.process.returncode = 0
            observer = MagicMock()
            observer.__enter__.return_value = observer
            observer.process.poll.side_effect = [None, 0]
            output = io.StringIO()
            answers = [SimpleNamespace(stdout=b'{"type":"s","data":[":1.0"]}'),
                       SimpleNamespace(stdout=json.dumps({"type": "u", "data": [True if failed else 1]}).encode())]
            with patch.object(subscription_process, "SubscriptionProcess", return_value=subscription), \
                    patch.object(status_restore_monitor, "RestoreMonitor", return_value=observer), \
                    patch("os.geteuid", return_value=0), \
                    patch("os.uname", return_value=SimpleNamespace(machine="aarch64")), \
                    patch("select.select", return_value=([], [], [])), \
                    patch("subprocess.run", side_effect=answers) as run, \
                    contextlib.redirect_stdout(output), self.assertRaises(SystemExit) as exited:
                exec(capture.REMOTE, {"worker_source": "synthetic", "operation": OPERATION})
            self.assertEqual(exited.exception.code, 1 if failed else 0)
            if not failed:
                capture.validate(output.getvalue().encode(), OPERATION)
            self.assertEqual(len(run.call_args_list), 2)
            for call in run.call_args_list:
                self.assertEqual(call.args[0][0], "/usr/bin/busctl")
                self.assertIn("call", call.args[0])
                self.assertIn("org.freedesktop.DBus", call.args[0])
                self.assertEqual(call.kwargs["timeout"], 6)
            self.assertEqual(run.call_args_list[0].args[0][-3:], ["GetNameOwner", "s", "org.freedesktop.systemd1"])
            self.assertEqual(run.call_args_list[1].args[0][-3:], ["GetConnectionUnixProcessID", "s", ":1.0"])


if __name__ == "__main__":
    unittest.main()
