from pathlib import Path
import subprocess
import unittest


ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "tools/storage_acceptance_probe.py"


class StorageAcceptanceProbeTests(unittest.TestCase):
    def test_probe_is_syntax_valid_and_secret_stdin_only(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        result = subprocess.run(
            ["python3", "-m", "py_compile", str(PROBE)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("media.read_password()", source)
        self.assertNotIn("process.env", source)
        self.assertNotIn("10.0.", source)
        self.assertNotIn("/home/w0w", source)
        self.assertIn("prototype.write_json_0600_atomic", source)

    def test_probe_spaces_mutations_and_covers_the_gate(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        for required in (
            "/api/storage/image",
            "/api/storage/image/mounted",
            "/api/storage/cdrom",
            "/api/storage/image/mount",
            "/api/storage/usb/reconnect",
            "hardened-blikvm-storage-acceptance",
            "COOLDOWN_SECONDS",
            '"passed"',
        ):
            self.assertIn(required, source)
        self.assertNotIn('"apply"', source)


if __name__ == "__main__":
    unittest.main()
