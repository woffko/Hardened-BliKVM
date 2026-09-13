import subprocess
import unittest
from typing import cast

from tools import capture_persistent_write_host_gate as gate
from tools import test_persistent_write_host_receipt as fixture


def completed(returncode: int, stdout: bytes) -> subprocess.CompletedProcess[bytes]:
    return subprocess.CompletedProcess(["python3"], returncode, stdout, b"")


class HostGateCaptureTests(unittest.TestCase):
    def test_run_suite_parses_counts_and_hashes(self) -> None:
        calls: list[list[str]] = []

        def runner(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            calls.append(argv)
            self.assertEqual(kwargs["cwd"], str(gate.ROOT))
            return completed(0, b"....\nRan 123 tests in 1.0s\n\nOK\n")

        suite = gate.run_suite(runner)
        self.assertEqual(suite["tests"], 123)
        self.assertEqual(suite["command"], gate.host.COMMAND)
        hashes = suite["sourceHashes"]
        self.assertIsInstance(hashes, dict)
        typed_hashes = cast(dict[str, str], hashes)
        self.assertEqual(set(typed_hashes), set(gate.host.source_paths()))
        self.assertEqual(calls, [gate.host.COMMAND])

    def test_run_suite_rejects_failures_and_missing_summary(self) -> None:
        def failed(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            return completed(1, b"FAILED (failures=1)\n")

        def silent(argv: list[str], **kwargs: object) -> subprocess.CompletedProcess[bytes]:
            return completed(0, b"")

        for runner in (failed, silent):
            with self.subTest(runner=runner.__name__), self.assertRaisesRegex(ValueError, "did not pass"):
                gate.run_suite(runner)

    def test_binding_requires_a_prepared_or_top_level_document(self) -> None:
        expected = gate.receipts.read_binding(fixture.BINDING)
        self.assertEqual(gate.read_binding({"prepared": {"binding": fixture.BINDING}}), expected)
        self.assertEqual(gate.read_binding({"binding": fixture.BINDING}), expected)
        with self.assertRaises(ValueError):
            gate.read_binding({})


if __name__ == "__main__":
    unittest.main()
