import contextlib
import io
import json
import subprocess
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import capture_status_bus_preflight as probe


def properties() -> dict[str, object]:
    data: dict[str, object] = {"s": "fixture", "b": False, "u": 0, "t": 0, "a(stt)": []}
    return {name: {"type": signature, "data": data[signature]} for name, signature in probe.SIGNATURES.items()}


class PreflightTests(unittest.TestCase):
    def test_remote_uses_only_fixed_property_reads(self) -> None:
        output = io.StringIO()
        with patch("os.geteuid", return_value=0), \
                patch("os.uname", return_value=SimpleNamespace(machine="aarch64")), \
                patch("subprocess.run", side_effect=[SimpleNamespace(stdout=json.dumps(value).encode())
                                                      for value in properties().values()]) as run, \
                contextlib.redirect_stdout(output):
            exec(probe.remote_source(), {})
        self.assertEqual(len(run.call_args_list), len(probe.QUERIES))
        for call, query in zip(run.call_args_list, probe.QUERIES):
            self.assertEqual(call.args[0], ["busctl", "--system", "--json=short", "get-property",
                                            "org.freedesktop.systemd1", *query[1:]])
            self.assertEqual(call.kwargs["timeout"], 5)
        self.assertEqual(probe.validate(output.getvalue().encode())["result"], "observed")

    def test_partial_failure_retains_only_public_observations(self) -> None:
        output = io.StringIO()
        with patch("os.geteuid", return_value=0), \
                patch("os.uname", return_value=SimpleNamespace(machine="aarch64")), \
                patch("subprocess.run", side_effect=[SimpleNamespace(stdout=b'{"type":"s","data":"fixture"}'),
                                                      subprocess.CalledProcessError(1, "ignored", stderr=b"not-exported")]), \
                contextlib.redirect_stdout(output), self.assertRaises(SystemExit):
            exec(probe.remote_source(), {})
        value = probe.validate(output.getvalue().encode())
        self.assertEqual(value["phase"], "socketActive")
        self.assertNotIn("not-exported", output.getvalue())

    def test_receipt_rejects_foreign_fields_and_boolean_clock(self) -> None:
        good: dict[str, object] = {"result": "observed", "monotonicUsec": 1,
                                  "properties": properties()}
        invalid_values: tuple[tuple[str, object], ...] = (("monotonicUsec", True), ("properties", {}), ("foreign", 1))
        for name, replacement in invalid_values:
            invalid = dict(good)
            invalid[name] = replacement
            with self.assertRaises(ValueError):
                probe.validate(json.dumps(invalid).encode())


if __name__ == "__main__":
    unittest.main()
