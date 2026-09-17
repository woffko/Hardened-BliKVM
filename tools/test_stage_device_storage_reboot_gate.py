import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_storage_reboot_gate as gate


def facts(**overrides: str) -> dict[str, str]:
    base = {
        "boot_id": "c4bfe713-2aa8-43f4-9292-151a1273d833",
        "fstab_entries": "1",
        "mount_source": "/dev/mmcblk0p4",
        "mount_target": gate.IMAGES_DIR,
        "mount_fstype": "ext4",
        "mount_options": "rw,nosuid,nodev,noexec,relatime",
        "tmpfiles_entries": "5",
        "gadget_dir": "750 root hardened-blikvm",
        "images_dir": "770 root hardened-blikvm",
        "image_present": "yes",
        "image_bytes": str(gate.IMAGE_BYTES),
        "failed_units": "0",
        "owner_unit": "active",
        "server_unit": "active",
    }
    base.update(overrides)
    return base


def render(values: dict[str, str]) -> str:
    return "\n".join(f"{key}={value}" for key, value in values.items()) + "\n"


class StorageRebootGateTests(unittest.TestCase):
    def test_the_probe_is_read_only(self) -> None:
        script = gate.probe_script()
        for forbidden in ("reboot", "systemctl restart", "mount ", "chmod", "install -m"):
            self.assertNotIn(forbidden, script)
        for required in ("fstab_entries=", "mount_options=", "tmpfiles_entries=", "boot_id="):
            self.assertIn(required, script)

    def test_facts_parse_ignores_indented_output(self) -> None:
        parsed = gate.parse_facts(" indented=1\nboot_id=x\nbad key=1\n")
        self.assertEqual(parsed, {"boot_id": "x"})

    def test_every_preserved_fact_is_evaluated(self) -> None:
        preserved = gate.preserved_facts(facts())
        self.assertTrue(all(preserved.values()), preserved)
        self.assertFalse(gate.preserved_facts(facts(mount_options="ro")).get("readWrite"))
        self.assertFalse(
            gate.preserved_facts(facts(mount_source="/dev/mmcblk0p1")).get("partitionSource")
        )
        self.assertFalse(gate.preserved_facts(facts(failed_units="2")).get("noFailedUnits"))
        self.assertFalse(gate.preserved_facts(facts(image_present="no")).get("image"))

    def test_the_outcome_requires_the_new_boot_and_the_owned_unit(self) -> None:
        before = facts()
        self.assertEqual(
            gate.decide_outcome(confirm_reboot=False, before=before, after={}), "plan"
        )
        same_boot = gate.decide_outcome(
            confirm_reboot=True, before=before, after=before
        )
        self.assertEqual(same_boot, "inspect")
        after = facts(boot_id="new-boot-id")
        self.assertEqual(
            gate.decide_outcome(confirm_reboot=True, before=before, after=after), "passed"
        )
        broken = facts(boot_id="new-boot-id", owner_unit="inactive")
        self.assertEqual(
            gate.decide_outcome(confirm_reboot=True, before=before, after=broken), "inspect"
        )

    def _run(self, extra: list[str], outputs: list[str], expected_code: int = 0) -> dict:
        directory = Path(tempfile.mkdtemp())
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence"
            / "private"
            / f"storage-reboot-gate-test-{uuid.uuid4().hex}.json"
        )
        self.addCleanup(lambda: output.unlink(missing_ok=True))
        argv = [
            "stage_device_storage_reboot_gate.py",
            "--endpoint",
            "ssh://127.0.0.1:22/",
            "--username",
            "root",
            "--known-hosts",
            str(directory / "known-hosts"),
            "--output",
            str(output),
        ] + extra
        results = [mock.Mock(stdout=text.encode(), stderr=b"", returncode=0) for text in outputs]
        with (
            mock.patch.object(sys, "argv", argv),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=results),
            mock.patch.object(
                gate.prototype, "read_secret_stdin", return_value=bytearray(b"ssh-secret")
            ),
            mock.patch.object(gate, "wait_for_return", return_value=True),
        ):
            code = gate.main()
        self.assertEqual(code, expected_code)
        return json.loads(output.read_text(encoding="utf-8"))

    def test_plan_mode_records_the_pre_reboot_state(self) -> None:
        receipt = self._run([], [render(facts())])
        self.assertEqual(receipt["mode"], "plan")
        self.assertEqual(receipt["result"], "plan")
        self.assertIs(receipt["policy"]["mutationsPerformed"], False)
        self.assertTrue(all(receipt["preservedBefore"].values()))

    def test_a_preserved_reboot_passes(self) -> None:
        receipt = self._run(
            ["--confirm-reboot", "--boot-wait", "0"],
            [render(facts()), render(facts()), render(facts(boot_id="new-boot-id"))],
        )
        self.assertEqual(receipt["result"], "passed")
        self.assertIs(receipt["policy"]["mutationsPerformed"], True)

    def test_a_reboot_that_loses_a_fact_is_inspect(self) -> None:
        receipt = self._run(
            ["--confirm-reboot", "--boot-wait", "0"],
            [
                render(facts()),
                render(facts()),
                render(facts(boot_id="new-boot-id", tmpfiles_entries="0")),
            ],
            expected_code=1,
        )
        self.assertEqual(receipt["result"], "inspect")


if __name__ == "__main__":
    unittest.main()
