import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from tools import switch_signed_slot_server as native
from tools.stage_device_signed_slot_server import browser_accepted


class SignedSlotServingTests(unittest.TestCase):
    def test_template_activity_checks_instances_without_querying_empty_template(self) -> None:
        def activity(unit: str) -> None:
            self.assertNotIn("@.", unit)
        expected = "dd3d529561a1e3699d1eec9d48b07921067de3242b1f87251945d519b63644d1"
        with patch.object(native, "inactive", side_effect=activity), patch.object(native, "enabled", return_value="static"), \
                patch.object(native, "digest", return_value=expected), patch.object(native, "checked", return_value=b"") as query:
            native.updater_inert()
            self.assertIn("hardened-blikvm-update-confirm@*.service", query.call_args.args[0])
            query.return_value = b"hardened-blikvm-update-confirm@1.service loaded active running\n"
            with self.assertRaises(ValueError):
                native.updater_inert()

    def test_proxy_service_and_tls_settings_survive_dependency_change(self) -> None:
        source = Path("packaging/systemd/hardened-blikvm-nginx.service").read_bytes()
        changed = native.slot_nginx_unit(source)
        self.assertEqual(source.split(b"[Service]\n", 1)[1], changed.split(b"[Service]\n", 1)[1])
        self.assertNotIn(b"Requires=hardened-blikvm-server", changed)
        self.assertIn(b"After=hardened-blikvm-server-current.service hardened-blikvm-server-candidate.service\n", changed)
        self.assertEqual([line for line in source.splitlines() if line.startswith(b"Condition")],
                         [line for line in changed.splitlines() if line.startswith(b"Condition")])

    def test_ambiguous_or_unexpected_dependency_layout_is_rejected(self) -> None:
        for source in (b"[Unit]\n", b"Description=Requires=hardened-blikvm-server.service\nAfter=hardened-blikvm-server.service\n"):
            with self.assertRaises(ValueError):
                native.slot_nginx_unit(source)
        source = Path("packaging/systemd/hardened-blikvm-nginx.service").read_bytes()
        with self.assertRaises(ValueError):
            native.slot_nginx_unit(source + b"Requires=hardened-blikvm-server.service\n")

    def test_browser_receipt_from_another_transition_cannot_commit(self) -> None:
        root = Path("evidence/private").resolve()
        root.mkdir(parents=True, exist_ok=True)
        with tempfile.NamedTemporaryFile(dir=root, suffix=".json") as stream:
            path = Path(stream.name)
            tag = "hardened-blikvm-slot-serving-" + "a" * 24
            data = {"probe": "hardened-blikvm-existing-account-browser", "result": "passed", "runTag": tag,
                    "videoDecoded": True, "sessionCleared": True, "streamRevoked": True,
                    "controlRequests": 0, "webSockets": 0}
            path.write_text(json.dumps(data))
            browser_accepted(path, tag)
            with self.assertRaises(ValueError):
                browser_accepted(path, "hardened-blikvm-slot-serving-" + "b" * 24)
            data["videoDecoded"] = False
            path.write_text(json.dumps(data))
            with self.assertRaises(ValueError):
                browser_accepted(path, tag)
