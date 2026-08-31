import subprocess
import sys
import unittest

import tools.capture_serial_topology as serial


def valid_facts() -> dict[str, str]:
    facts = {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "ttyusb_count": "1",
        "by_id_directory_present": "yes",
    }
    for index in serial.TTY_INDICES:
        present = index == 0
        values = {
            "present": "yes" if present else "no",
            "class_dev": "188:0" if present else "unavailable",
            "device": "ttyUSB0" if present else "unavailable",
            "driver": "ch341" if present else "unavailable",
            "usb_vendor_id": "1a86" if present else "unavailable",
            "usb_product_id": "7523" if present else "unavailable",
            "node_present": "yes" if present else "no",
            "node_character": "yes" if present else "no",
            "node_mode": "660" if present else "unavailable",
            "node_owner": "root" if present else "unavailable",
            "node_group": "dialout" if present else "unavailable",
        }
        for field, value in values.items():
            facts[f"ttyusb{index}_{field}"] = value
    return facts


class CaptureSerialTopologyTests(unittest.TestCase):
    def test_parser_accepts_only_complete_non_unique_metadata(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in reversed(facts.items())).encode()
        self.assertEqual(serial.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_and_contradictory_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            serial.parse_facts(raw + b"serial=forbidden\n")
        with self.assertRaises(ValueError):
            serial.parse_facts(raw + b"ttyusb0_driver=duplicate\n")
        facts["ttyusb0_present"] = "no"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            serial.parse_facts(contradictory)

    def test_probe_never_opens_tty_or_reads_unique_identifiers_or_frames(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=serial.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        self.assertIn("/sys/class/tty/ttyUSB", serial.REMOTE_PROBE)
        self.assertIn("idVendor", serial.REMOTE_PROBE)
        self.assertIn("idProduct", serial.REMOTE_PROBE)
        for forbidden in (
            "stty",
            "setserial",
            "udevadm",
            "ID_SERIAL",
            "/dev/serial/by-id/",
            "gpioget",
            "cat /dev/tty",
            "< /dev/tty",
            "> /dev/tty",
        ):
            self.assertNotIn(forbidden, serial.REMOTE_PROBE)

    def test_policy_records_metadata_only_scope(self) -> None:
        policy = serial.policy_receipt()
        self.assertIs(policy["ttyOpened"], False)
        self.assertIs(policy["termiosChanged"], False)
        self.assertIs(policy["framesRead"], False)
        self.assertIs(policy["framesWritten"], False)
        self.assertIs(policy["byIdIdentifiersRead"], False)
        self.assertIs(policy["usbSerialRead"], False)
        self.assertIs(policy["physicalActionsInvoked"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_serial_topology.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
