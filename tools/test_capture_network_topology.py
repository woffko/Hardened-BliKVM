import subprocess
import sys
import unittest

import tools.capture_network_topology as network


def valid_facts() -> dict[str, str]:
    facts = {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "interface_count": "4",
        "nmcli_available": "yes",
        "networkmanager_state": "active",
        "systemd_networkd_state": "inactive",
        "connman_state": "unknown",
        "dhcpcd_state": "inactive",
    }
    for name in network.INTERFACES:
        present = name in {"end0", "wlan0"}
        wireless = name == "wlan0"
        facts.update(
            {
                f"{name}_present": "yes" if present else "no",
                f"{name}_type": "1" if present else "unavailable",
                f"{name}_operstate": "up" if present else "unavailable",
                f"{name}_carrier": "1" if present else "unavailable",
                f"{name}_wireless": "yes" if wireless else ("no" if present else "unavailable"),
                f"{name}_driver": "rtl8189fs" if wireless else ("dwmac-sun8i" if present else "unavailable"),
            }
        )
    return facts


class CaptureNetworkTopologyTests(unittest.TestCase):
    def test_parser_accepts_only_complete_sanitized_metadata(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in reversed(facts.items())).encode()
        self.assertEqual(network.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_private_unknown_duplicate_and_contradictory_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        for forbidden in (
            b"address=192.0.2.1\n",
            b"mac=00:11:22:33:44:55\n",
            b"ssid=private\n",
            b"route=default\n",
        ):
            with self.assertRaises(ValueError):
                network.parse_facts(raw + forbidden)
        with self.assertRaises(ValueError):
            network.parse_facts(raw + b"end0_driver=duplicate\n")
        facts["end0_present"] = "no"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            network.parse_facts(contradictory)
        facts = valid_facts()
        facts["end0_driver"] = "../../unsafe"
        unsafe = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            network.parse_facts(unsafe)
        facts = valid_facts()
        facts["interface_count"] = "1"
        impossible_count = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            network.parse_facts(impossible_count)

    def test_probe_avoids_address_identity_scan_and_mutation_surfaces(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=network.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        self.assertIn("/sys/class/net/$name", network.REMOTE_PROBE)
        for name in network.INTERFACES:
            self.assertIn(name, network.REMOTE_PROBE)
        for forbidden in (
            "/address",
            "/proc/net",
            "ip addr",
            "ip route",
            "ifconfig",
            "nmcli device",
            "nmcli connection",
            "networkctl",
            "resolv",
            "SSID",
            "BSSID",
            "gateway",
            "systemctl restart",
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "/etc/NetworkManager",
        ):
            self.assertNotIn(forbidden, network.REMOTE_PROBE)

    def test_policy_explicitly_records_sanitized_read_only_scope(self) -> None:
        policy = network.policy_receipt()
        self.assertIs(policy["sysfsInterfaceMetadataRead"], True)
        self.assertIs(policy["serviceStateCommandsInvoked"], True)
        self.assertIs(policy["nmcliInvoked"], False)
        self.assertIs(policy["addressesRead"], False)
        self.assertIs(policy["linkLayerAddressesRead"], False)
        self.assertIs(policy["ssidOrBssidRead"], False)
        self.assertIs(policy["routesRead"], False)
        self.assertIs(policy["resolversRead"], False)
        self.assertIs(policy["connectionProfilesRead"], False)
        self.assertIs(policy["wirelessScanTriggered"], False)
        self.assertIs(policy["managerMutationCommandsInvoked"], False)
        self.assertIs(policy["interfaceStateChanged"], False)
        self.assertIs(policy["servicesChanged"], False)
        self.assertIs(policy["uniqueIdentifiersRead"], False)
        self.assertIs(policy["physicalActionsInvoked"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_network_topology.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
