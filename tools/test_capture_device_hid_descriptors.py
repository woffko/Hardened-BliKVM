import subprocess
import sys
import unittest
from typing import cast

import tools.capture_device_hid_descriptors as hid


def descriptor_hex() -> str:
    return "05010906a101050719e029e7150025017501950881029501750881010508"


def sha_of(hex_value: str) -> str:
    import hashlib

    return hashlib.sha256(bytes.fromhex(hex_value)).hexdigest()


def valid_lines() -> list[str]:
    first = descriptor_hex()
    return [
        "gadget_present=yes",
        "gadget=kvmd",
        f"hid:hid.usb0:1:1:8:{first}:{sha_of(first)}",
        f"hid:hid.usb1:0:0:7:{first}:{sha_of(first)}",
    ]


class CaptureHidDescriptorsTests(unittest.TestCase):
    def test_parser_accepts_the_canonical_read_only_output(self) -> None:
        facts = hid.parse_probe(("\n".join(valid_lines()) + "\n").encode())
        self.assertIs(facts["gadgetPresent"], True)
        self.assertEqual(facts["gadget"], "kvmd")
        functions = cast(list, facts["functions"])
        self.assertEqual(len(functions), 2)
        self.assertEqual(functions[0]["name"], "hid.usb0")
        self.assertEqual(functions[0]["protocol"], 1)
        self.assertEqual(functions[0]["reportLength"], 8)
        self.assertEqual(functions[0]["reportDescriptorHex"], descriptor_hex())

    def test_parser_accepts_an_absent_gadget(self) -> None:
        facts = hid.parse_probe(b"gadget_present=no\n")
        self.assertIs(facts["gadgetPresent"], False)
        self.assertEqual(facts["functions"], [])

    def test_parser_rejects_malformed_or_contradictory_output(self) -> None:
        bad_inputs = [
            b"",
            b"gadget_present=maybe\n",
            b"gadget_present=yes\n",
            b"gadget_present=yes\ngadget=BAD NAME\n",
            b"gadget_present=yes\ngadget=kvmd\nhid:hid.usb0:1:1:8:nothex:"
            + sha_of(descriptor_hex()).encode()
            + b"\n",
            b"gadget_present=no\ngadget=kvmd\n",
            b"gadget_present=yes\ngadget=kvmd\nunexpected=1\n",
        ]
        for raw in bad_inputs:
            with self.subTest(raw=raw):
                with self.assertRaises(ValueError):
                    hid.parse_probe(raw)

    def test_parser_rejects_bad_hash_duplicate_and_bounds(self) -> None:
        first = descriptor_hex()
        with self.assertRaises(ValueError):
            hid.parse_probe(
                f"gadget_present=yes\ngadget=kvmd\nhid:hid.usb0:1:1:8:{first}:{'0' * 64}\n".encode()
            )
        with self.assertRaises(ValueError):
            hid.parse_probe(
                ("\n".join(valid_lines() + [valid_lines()[2]]) + "\n").encode()
            )
        with self.assertRaises(ValueError):
            hid.parse_probe(
                f"gadget_present=yes\ngadget=kvmd\nhid:HID.USB0:1:1:8:{first}:{sha_of(first)}\n".encode()
            )
        with self.assertRaises(ValueError):
            hid.parse_probe(
                f"gadget_present=yes\ngadget=kvmd\nhid:hid.usb0:1:1:0:{first}:{sha_of(first)}\n".encode()
            )

    def test_remote_script_is_read_only(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=hid.REMOTE_SCRIPT.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        for marker in (
            "/sys/kernel/config/usb_gadget",
            "report_desc",
            "report_length",
            "protocol",
            "subclass",
            "sha256sum",
        ):
            self.assertIn(marker, hid.REMOTE_SCRIPT)
        for forbidden in (
            "systemctl",
            "rm ",
            "dd ",
            "mkfs",
            "mount ",
            "umount",
            "modprobe",
            "insmod",
            "rmmod",
            "chmod",
            "chown",
            "tee ",
            "mv ",
            "cp ",
            "/dev/hidg",
        ):
            self.assertNotIn(forbidden, hid.REMOTE_SCRIPT)

    def test_policy_records_read_only_scope(self) -> None:
        policy = hid.policy_receipt()
        self.assertIs(policy["sysfsGadgetMetadataRead"], True)
        self.assertIs(policy["reportDescriptorsRead"], True)
        self.assertIs(policy["credentialsRead"], False)
        self.assertIs(policy["mutationsPerformed"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_hid_descriptors.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
