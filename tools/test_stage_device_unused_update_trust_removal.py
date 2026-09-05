#!/usr/bin/env python3
"""Tests for exact unused application trust removal."""

from __future__ import annotations

import json
import os
from pathlib import Path
import tempfile
import unittest
from typing import cast

from tools import stage_device_unused_update_trust_removal as removal


class UnusedUpdateTrustRemovalTests(unittest.TestCase):
    def enrollment(self) -> removal.AcceptedEnrollment:
        return removal.AcceptedEnrollment("a" * 64, "b" * 64, "c" * 40)

    def baseline(self) -> dict[str, str]:
        return {
            "certificate_path": "/etc/kvmd/nginx/ssl/server.crt",
            "certificate_sha256": "d" * 64,
            "key_path": "/etc/kvmd/nginx/ssl/server.key",
            "key_sha256": "e" * 64,
        }

    def test_preflight_binds_exact_config_and_absent_update_authority(self) -> None:
        script = removal.build_preflight_script(self.enrollment(), self.baseline())
        self.assertIn('stat -c \'%a:%u:%g:%h\' "$config_target"', script)
        self.assertIn('= "$config_sha256"', script)
        self.assertIn("update_runtime_absent", script)
        self.assertIn("update_units_inert", script)
        self.assertIn("persistent_runtime_healthy", script)
        self.assertIn("trust_guards_absent", script)
        self.assertNotIn('rm -f -- "$config_target"', script)

    def test_removal_is_one_exact_unlink_with_no_power_or_unit_transition(self) -> None:
        preflight = {key: "yes" for key in removal.PRE_KEYS}
        script = removal.build_remove_script(
            self.enrollment(), self.baseline(), preflight
        )
        self.assertEqual(script.count('rm -f -- "$config_target"'), 1)
        self.assertIn('[ ! -e "$config_target" ] && [ ! -L "$config_target" ]', script)
        for forbidden in (
            "systemctl enable",
            "systemctl restart",
            "systemctl reboot",
            "shutdown",
            "poweroff",
            "/sys/class/gpio",
            "application-state.json\" >",
        ):
            self.assertNotIn(forbidden, script)

    def test_final_verification_requires_absence_and_unchanged_runtime(self) -> None:
        preflight = {key: "yes" for key in removal.PRE_KEYS}
        script = removal.build_final_script(
            self.enrollment(), self.baseline(), preflight
        )
        self.assertIn('[ ! -e "$config_target" ] && [ ! -L "$config_target" ]', script)
        self.assertIn("persistent_runtime_healthy", script)
        self.assertIn("update_units_inert", script)
        self.assertNotIn('rm -f -- "$config_target"', script)

    def test_classifier_distinguishes_old_absent_and_unknown(self) -> None:
        script = removal.build_classify_script(self.enrollment())
        self.assertIn("emit config_state absent", script)
        self.assertIn("emit config_state old", script)
        self.assertIn("emit config_state unknown", script)

    def test_receipt_loader_accepts_only_unused_completed_enrollment(self) -> None:
        root = Path(__file__).resolve().parent.parent / "evidence/private"
        root.mkdir(mode=0o700, parents=True, exist_ok=True)
        descriptor, path_value = tempfile.mkstemp(prefix="unused-trust-test-", dir=root)
        path = Path(path_value)
        try:
            receipt = {
                "schemaVersion": 1,
                "probe": "hardened-blikvm-update-trust-enrollment",
                "result": "passed",
                "configSha256": "a" * 64,
                "requestSha256": "b" * 64,
                "sourceCommit": "c" * 40,
                "policy": {
                    "configPublished": True,
                    "publicKeyOnly": True,
                    "privateKeyAccepted": False,
                    "stateInitialized": False,
                    "unitsEnabled": False,
                    "deviceReboot": False,
                    "targetPowerControl": False,
                },
                "finalState": {
                    "config_persistent": "yes",
                    "update_runtime_absent": "yes",
                    "update_units_inert": "yes",
                    "guards_removed": "yes",
                },
            }
            os.write(descriptor, json.dumps(receipt).encode())
            os.close(descriptor)
            descriptor = -1
            path.chmod(0o600)
            accepted = removal.load_enrollment_receipt(str(path), "a" * 64)
            self.assertEqual(accepted.request_sha256, "b" * 64)

            policy = cast(dict[str, object], receipt["policy"])
            policy["stateInitialized"] = True
            path.write_text(json.dumps(receipt))
            with self.assertRaises(ValueError):
                removal.load_enrollment_receipt(str(path), "a" * 64)
        finally:
            if descriptor >= 0:
                os.close(descriptor)
            path.unlink(missing_ok=True)


if __name__ == "__main__":
    unittest.main()
