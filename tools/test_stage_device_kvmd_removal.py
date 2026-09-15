import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from tools import stage_device_kvmd_removal as removal


def facts_line(key: str, value: str) -> str:
    return f"{key}={value}\n"


def inventory_facts() -> bytes:
    values = {
        "kvmd_active": "yes",
        "kvmd_main_active": "yes",
        "kvmd_otg_active": "yes",
        "kvmd_webterm_active": "yes",
        "kvmd_unit": "/lib/systemd/system/kvmd.service",
        "kvmd_main_unit": "/lib/systemd/system/kvmd-main.service",
        "kvmd_otg_unit": "/lib/systemd/system/kvmd-otg.service",
        "kvmd_webterm_unit": "/lib/systemd/system/kvmd-webterm.service",
        "vendor_atx_present": "yes",
        "kvmd_config_present": "yes",
        "owner_socket_present": "no",
        "gadget_present": "no",
        "udc_bound": "no",
        "configfs_mounted": "yes",
    }
    return "".join(facts_line(key, value) for key, value in values.items()).encode()


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value), encoding="utf-8")


class KvmdRemovalTests(unittest.TestCase):
    def test_generated_scripts_are_valid_shell_and_have_no_secrets(self) -> None:
        scripts = (
            removal.REMOTE_INVENTORY,
            removal.build_removal_script("/var/lib/hardened-blikvm-gadget/kvmd-backup-1700000000"),
            removal.build_restore_script(),
        )
        for script in scripts:
            syntax = subprocess.run(
                ["sh", "-n"],
                input=script.encode(),
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
            for forbidden in ("password", "sshpass", "-i "):
                self.assertNotIn(forbidden, script)

    def test_backup_directory_is_pinned(self) -> None:
        with self.assertRaises(ValueError):
            removal.build_removal_script("/tmp/backup")
        with self.assertRaises(ValueError):
            removal.build_removal_script(
                "/var/lib/hardened-blikvm-gadget/kvmd-backup-now"
            )

    def test_inventory_facts_are_validated(self) -> None:
        facts = removal.parse_facts(inventory_facts(), set(removal.INVENTORY_FACTS))
        self.assertEqual(facts["kvmd_active"], "yes")
        with self.assertRaises(ValueError):
            removal.parse_facts(
                inventory_facts().replace(b"kvmd_active=yes", b"kvmd_active=maybe"),
                set(removal.INVENTORY_FACTS),
            )
        with self.assertRaises(ValueError):
            removal.parse_facts(
                inventory_facts() + b"surprise=yes\n", set(removal.INVENTORY_FACTS)
            )
        with self.assertRaises(ValueError):
            removal.parse_facts(
                inventory_facts().replace(
                    b"kvmd_unit=/lib/systemd/system/kvmd.service",
                    b"kvmd_unit=relative/path",
                ),
                set(removal.INVENTORY_FACTS),
            )
        with self.assertRaises(ValueError):
            removal.parse_facts(
                inventory_facts().replace(b"kvmd_active=yes\n", b""),
                set(removal.INVENTORY_FACTS),
            )

    def test_removal_is_refused_without_the_acceptance_receipts(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            missing = removal.evidence_gate(evidence)
            self.assertEqual(len(missing), 3)

            write_json(
                evidence / "board-handoff-gate-20260915-f.json",
                {
                    "gate": "hardened-blikvm-board-profile-handoff",
                    "acceptanceFacts": {"gadget_present": "yes", "udc_bound": "yes"},
                },
            )
            self.assertEqual(len(removal.evidence_gate(evidence)), 2)

            write_json(
                evidence / "virtual-media-acceptance-20260915-a.json",
                {
                    "probe": "hardened-blikvm-virtual-media-acceptance",
                    "result": "passed",
                },
            )
            write_json(
                evidence / "board-handoff-gate-20260915-f.json",
                {
                    "gate": "hardened-blikvm-board-profile-handoff",
                    "acceptanceFacts": {"gadget_present": "yes", "udc_bound": "yes"},
                    "atxBenchFacts": {"power_short": "ok", "reset": "ok"},
                },
            )
            self.assertEqual(removal.evidence_gate(evidence), [])

    def test_acceptance_receipt_must_show_a_bound_gadget(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            evidence = Path(directory)
            write_json(
                evidence / "board-handoff-gate-20260915-f.json",
                {
                    "gate": "hardened-blikvm-board-profile-handoff",
                    "acceptanceFacts": {"gadget_present": "yes", "udc_bound": "no"},
                    "atxBenchFacts": {"power_short": "ok", "reset": "ok"},
                },
            )
            write_json(
                evidence / "virtual-media-acceptance-20260915-a.json",
                {
                    "probe": "hardened-blikvm-virtual-media-acceptance",
                    "result": "passed",
                },
            )
            self.assertEqual(
                removal.evidence_gate(evidence),
                ["a passing composite-gadget acceptance receipt"],
            )


if __name__ == "__main__":
    unittest.main()
