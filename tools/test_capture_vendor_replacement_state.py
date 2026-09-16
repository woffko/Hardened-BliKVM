import subprocess
import sys
import unittest
from pathlib import Path

from tools import capture_vendor_replacement_state as capture


ROOT = Path(__file__).resolve().parent.parent
PROBE = ROOT / "tools/capture_vendor_replacement_state.py"


def facts() -> dict:
    fixture = {
        "gadget_present": "yes",
        "gadget_udc": "musb-hdrc.2.auto",
        "gadget_functions": "hid.usb0,hid.usb1,hid.usb2,mass_storage.usb0,",
        "msd_source": "/dev/mmcblk0p4 /var/lib/hardened-blikvm-gadget/images",
        "image_bytes": "6841958400",
        "ustreamer_socket": "yes",
        "proxy_socket": "yes",
        "health": '{"code":0,"data":{"status":"ok","platformId":"bli-h616-pikvm"}}',
        "title": "<title>Hardened BliKVM</title>",
    }
    for unit in capture.OUR_UNITS:
        fixture[f"unit_{unit}"] = "active"
    for unit in capture.RETIRED_UNITS:
        fixture[f"retired_{unit}_active"] = "inactive"
        fixture[f"retired_{unit}_enabled"] = "masked"
    return fixture


class VendorReplacementCaptureTests(unittest.TestCase):
    def test_probe_is_syntax_valid_and_secret_free(self) -> None:
        source = PROBE.read_text(encoding="utf-8")
        result = subprocess.run(
            ["python3", "-m", "py_compile", str(PROBE)],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertIn("readOnly", source)
        self.assertNotIn("x-csrf-token", source)

    def test_complete_state_passes_and_every_check_is_required(self) -> None:
        passed, checks = capture.evaluate(facts())
        self.assertTrue(passed, [name for name, ok in checks.items() if not ok])
        self.assertGreater(len(checks), 12)
        for broken in (
            "unit_hardened-blikvm-board-owner.service",
            "retired_kvmd.service_active",
            "retired_kvmd.service_enabled",
            "gadget_functions",
            "msd_source",
            "image_bytes",
            "ustreamer_socket",
            "proxy_socket",
            "health",
            "title",
        ):
            mutated = facts()
            mutated[broken] = "no"
            passed, checks = capture.evaluate(mutated)
            self.assertFalse(passed, broken)
            self.assertTrue(
                any(not ok for ok in checks.values()),
                f"{broken} did not fail any check",
            )

    def test_parse_facts_accepts_hyphenated_keys(self) -> None:
        parsed = capture.parse_facts("unit_a-b.service=active\nbad key=1\n")
        self.assertEqual(parsed, {"unit_a-b.service": "active"})


if __name__ == "__main__":
    _ = sys
    unittest.main()
