import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_settings_gate as gate


class DeviceSettingsGateTests(unittest.TestCase):
    def test_facts_script_and_parsing_are_read_only(self) -> None:
        script = gate.facts_script()
        for forbidden in ("systemctl", "rm -", "install -m", "echo >", "tee "):
            self.assertNotIn(forbidden, script)
        self.assertIn("resolvectl mdns", script)
        self.assertIn("journalctl --since '-2min'", script)
        self.assertIn("hardened-blikvm system-log test", script)
        facts = gate.parse_facts(
            "  indented=1\nmdns_link=yes\njournal_test_lines=3\nbad key=1\n"
        )
        self.assertEqual(
            facts, {"mdns_link": "yes", "journal_test_lines": "3"}
        )
        self.assertTrue(gate.mdns_enabled({"mdns_link": "yes"}))
        self.assertFalse(gate.mdns_enabled({"mdns_link": "no"}))
        self.assertFalse(gate.mdns_enabled({}))

    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-device-settings-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def test_plan_mode_records_the_read_only_views(self) -> None:
        output, known_hosts = self._paths()
        argv = [
            "stage_device_settings_gate.py",
            "--endpoint",
            "ssh://192.0.2.10",
            "--username",
            "root",
            "--known-hosts",
            str(known_hosts),
            "--output",
            str(output),
            "--base-url",
            "https://192.0.2.10",
            "--api-username",
            "admin",
        ]
        facts = "mdns_link=no\njournal_test_lines=0\n"
        api = mock.Mock()
        api.get.side_effect = [
            {"mode": "slaac"},
            {"available": True, "ssid": "HomeNet"},
            {"enabled": True, "subject": "CN = board"},
            {"enabled": False},
            {"enabled": True, "mode": "relative"},
        ]
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(
                gate.prototype,
                "run_ssh",
                side_effect=[mock.Mock(stdout=facts.encode(), stderr=b"", returncode=0)],
            ),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertEqual(receipt["ipv6Mode"], "slaac")
        self.assertEqual(receipt["wifiSsid"], "HomeNet")
        self.assertEqual(receipt["tlsSubject"], "CN = board")
        self.assertEqual(receipt["jigglerState"], "on")


if __name__ == "__main__":
    unittest.main()
