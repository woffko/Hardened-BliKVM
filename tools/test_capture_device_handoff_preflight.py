import subprocess
import sys
import unittest

import tools.capture_device_handoff_preflight as preflight


def ready_facts() -> dict[str, str]:
    return {
        "kvmd_present": "yes",
        "vendor_present": "yes",
        "gadget_present": "yes",
        "udc_present": "yes",
        "configfs_mounted": "yes",
        "kvmd_active": "yes",
        "kvmd_main_active": "yes",
        "gadget_name": "kvmd",
    }


def render(facts: dict[str, str]) -> bytes:
    return "".join(f"{key}={value}\n" for key, value in facts.items()).encode()


class CaptureHandoffPreflightTests(unittest.TestCase):
    def test_parser_accepts_a_ready_stand(self) -> None:
        facts = preflight.parse_facts(render(ready_facts()))
        self.assertTrue(preflight.handoff_ready(facts))

    def test_absent_stand_is_valid_but_not_ready(self) -> None:
        # The current endpoint is a plain host: no KVMD, vendor stack or UDC.
        facts = render(
            {
                "kvmd_present": "no",
                "vendor_present": "no",
                "gadget_present": "no",
                "udc_present": "no",
                "configfs_mounted": "yes",
                "kvmd_active": "no",
                "kvmd_main_active": "no",
                "gadget_name": "none",
            }
        )
        parsed = preflight.parse_facts(facts)
        self.assertFalse(preflight.handoff_ready(parsed))

    def test_partial_stand_is_not_ready(self) -> None:
        for missing in ("kvmd_present", "vendor_present", "gadget_present", "udc_present"):
            facts = ready_facts()
            facts[missing] = "no"
            self.assertFalse(preflight.handoff_ready(facts), missing)

    def test_parser_rejects_malformed_facts(self) -> None:
        for key, value in (
            ("kvmd_present", "maybe"),
            ("gadget_name", "BAD NAME"),
            ("gadget_name", "none"),
        ):
            facts = ready_facts()
            facts[key] = value
            with self.subTest(key=key, value=value):
                with self.assertRaises(ValueError):
                    preflight.parse_facts(render(facts))
        with self.assertRaises(ValueError):
            preflight.parse_facts(render(ready_facts()) + b"unexpected=1\n")

    def test_remote_script_is_read_only(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=preflight.REMOTE_SCRIPT.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        for marker in ("/etc/kvmd", "/usr/bin/blikvm", "/sys/class/udc", "systemctl is-active"):
            self.assertIn(marker, preflight.REMOTE_SCRIPT)
        for forbidden in (
            "systemctl start",
            "systemctl stop",
            "systemctl restart",
            "systemctl enable",
            "systemctl disable",
            "rm ",
            "dd ",
            "mkfs",
            "umount",
            "modprobe",
            "/dev/hidg",
        ):
            self.assertNotIn(forbidden, preflight.REMOTE_SCRIPT)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_handoff_preflight.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
