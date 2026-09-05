from pathlib import Path
import inspect
import unittest

from tools import verify_update_status as gate


class UpdateStatusNativeGateTests(unittest.TestCase):
    def test_cleanup_rejects_broad_or_wrong_namespace_targets(self) -> None:
        for path in ("/", "/var/lib", "/tmp/blikvm-update-status-" + "a" * 24, "/var/lib/blikvm-update-status-invalid"):
            with self.assertRaises(ValueError):
                gate.cleanup(Path(path))

    def test_probe_uses_no_production_service_restart(self) -> None:
        source = inspect.getsource(gate)
        self.assertNotIn('"restart"', source)
        self.assertNotIn('"reboot"', source)
        self.assertIn("user=peer", source)
        self.assertIn('query(0, "status")', source)
        self.assertIn('("mutation", "oversize")', source)
        self.assertIn("range(4)", source)


if __name__ == "__main__":
    unittest.main()
