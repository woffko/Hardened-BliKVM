import subprocess
import unittest

from tools.stage_device_update_config_credential import probe_failure


class ProbeDiagnosticsTests(unittest.TestCase):
    def test_discarded_stderr_preserves_runner_failure(self) -> None:
        result = subprocess.CompletedProcess(["ssh"], 1, b'{"result":"failed","error":"stage exit 40"}', None)
        self.assertEqual(probe_failure(result), {"probeExitCode": 1, "error": "stage exit 40"})

    def test_missing_or_malformed_runner_output_keeps_exit_code(self) -> None:
        for raw in (b"", b"not json", b"[]", b"x" * 5000):
            self.assertEqual(probe_failure(subprocess.CompletedProcess(["ssh"], 2, raw, None)), {"probeExitCode": 2})
