import subprocess
import sys
import unittest

import tools.capture_device_kvm_topology as topology


def valid_facts() -> dict[str, str]:
    return {
        "architecture": "aarch64",
        "gadget_present": "yes",
        "descriptor_sha256": "a" * 64,
        "hid_functions": "3",
        "mass_storage_functions": "1",
        "udc_present": "yes",
        "kvmd_hid_otg": "yes",
        "kvmd_msd_otg": "yes",
        "kvmd_active": "yes",
        "kvmd_main_active": "yes",
        "ustreamer_active": "yes",
        "janus_active": "no",
        "nginx_active": "no",
        "blikvm_kvmd_main": "yes",
        "blikvm_enable_gadget": "yes",
        "blikvm_kvmd_msd": "yes",
        "atx_socket": "yes",
        "msd_mount_ro": "yes",
    }


def render(facts: dict[str, str]) -> bytes:
    return "".join(f"{key}={value}\n" for key, value in facts.items()).encode()


class CaptureDeviceKvmTopologyTests(unittest.TestCase):
    def test_parser_accepts_only_complete_sanitized_metadata(self) -> None:
        facts = valid_facts()
        raw = render(dict(reversed(list(facts.items()))))
        self.assertEqual(topology.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_and_private_facts(self) -> None:
        facts = valid_facts()
        raw = render(facts)
        for forbidden in (
            b"serialnumber=CAFEBABE\n",
            b"manufacturer=PiKVM\n",
            b"image=kubuntu.img\n",
            b"password=private\n",
            b"address=192.0.2.1\n",
        ):
            with self.assertRaises(ValueError):
                topology.parse_facts(raw + forbidden)
        with self.assertRaises(ValueError):
            topology.parse_facts(raw + b"hid_functions=4\n")

    def test_parser_rejects_malformed_values(self) -> None:
        for key, value in (
            ("architecture", "AARCH64"),
            ("gadget_present", "maybe"),
            ("descriptor_sha256", "nothex"),
            ("hid_functions", "9"),
            ("mass_storage_functions", "x"),
        ):
            facts = valid_facts()
            facts[key] = value
            with self.assertRaises(ValueError):
                topology.parse_facts(render(facts))

    def test_parser_rejects_contradictory_gadget_facts(self) -> None:
        facts = valid_facts()
        facts["gadget_present"] = "no"
        with self.assertRaises(ValueError):
            topology.parse_facts(render(facts))
        facts = valid_facts()
        facts["hid_functions"] = "0"
        facts["mass_storage_functions"] = "0"
        with self.assertRaises(ValueError):
            topology.parse_facts(render(facts))
        facts = valid_facts()
        facts["gadget_present"] = "no"
        facts["hid_functions"] = "0"
        facts["mass_storage_functions"] = "0"
        facts["descriptor_sha256"] = "none"
        facts["udc_present"] = "no"
        parsed = topology.parse_facts(render(facts))
        self.assertEqual(parsed["gadget_present"], "no")

    def test_probe_is_read_only_and_avoids_identity_surfaces(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=topology.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        for marker in (
            "/sys/kernel/config/usb_gadget",
            "/etc/kvmd/main.yaml",
            "/var/blikvm/atx.sock",
            "/var/lib/kvmd/msd",
            "sha256sum",
            "findmnt",
            "systemctl is-active",
        ):
            self.assertIn(marker, topology.REMOTE_PROBE)
        for forbidden in (
            "systemctl start",
            "systemctl stop",
            "systemctl restart",
            "systemctl enable",
            "systemctl disable",
            "systemctl mask",
            "umount",
            "mount -o",
            "mount /",
            "modprobe",
            "insmod",
            "rmmod",
            "tee ",
            "chmod",
            "chown",
            "mkfs",
            "fdisk",
            "parted",
            "blkdiscard",
            "/dev/hidg",
            "serialnumber",
            "manufacturer",
            "CAFEBABE",
            "ip addr",
            "ifconfig",
            "/address",
            "SSID",
            "BSSID",
        ):
            self.assertNotIn(forbidden, topology.REMOTE_PROBE)

    def test_policy_explicitly_records_sanitized_read_only_scope(self) -> None:
        policy = topology.policy_receipt()
        self.assertIs(policy["sysfsGadgetMetadataRead"], True)
        self.assertIs(policy["serviceStateCommandsInvoked"], True)
        self.assertIs(policy["kvmdConfigRead"], True)
        self.assertIs(policy["mountOptionsRead"], True)
        self.assertIs(policy["descriptorBlockHashed"], True)
        self.assertIs(policy["descriptorStringValuesRecorded"], False)
        self.assertIs(policy["uniqueIdentifiersRecorded"], False)
        self.assertIs(policy["credentialsRead"], False)
        self.assertIs(policy["imageContentsRead"], False)
        self.assertIs(policy["mutationsPerformed"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_device_kvm_topology.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
