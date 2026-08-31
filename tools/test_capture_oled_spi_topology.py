import subprocess
import sys
import unittest

import tools.capture_oled_spi_topology as oled


def valid_facts() -> dict[str, str]:
    return {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "spidev_class_present": "yes",
        "spidev_class_dev": "153:0",
        "spidev_device": "spi1.0",
        "spidev_driver": "spidev",
        "spidev_modalias": "spi:spidev",
        "spidev_of_compatible": "rohm,dh2228fv",
        "spi_bus_device_present": "yes",
        "spi_master_present": "yes",
        "spi_master_device": "5011000.spi",
        "spi_master_driver": "spi-sun6i",
        "node_present": "yes",
        "node_character": "yes",
        "node_mode": "660",
        "node_owner": "root",
        "node_group": "spi",
    }


class CaptureOledSpiTopologyTests(unittest.TestCase):
    def test_parser_accepts_only_complete_bounded_metadata(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in reversed(facts.items())).encode()
        self.assertEqual(oled.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_unsafe_and_contradictory_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            oled.parse_facts(raw + b"serial=forbidden\n")
        with self.assertRaises(ValueError):
            oled.parse_facts(raw + b"spidev_driver=duplicate\n")
        facts["spidev_of_compatible"] = "unsafe value"
        unsafe = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            oled.parse_facts(unsafe)
        facts = valid_facts()
        facts["node_present"] = "no"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            oled.parse_facts(contradictory)

    def test_parser_accepts_complete_absence_without_inventing_identity(self) -> None:
        facts = valid_facts()
        facts.update(
            {
                "spidev_class_present": "no",
                "spidev_class_dev": "unavailable",
                "spidev_device": "unavailable",
                "spidev_driver": "unavailable",
                "spidev_modalias": "unavailable",
                "spidev_of_compatible": "unavailable",
                "spi_bus_device_present": "no",
                "spi_master_present": "no",
                "spi_master_device": "unavailable",
                "spi_master_driver": "unavailable",
                "node_present": "no",
                "node_character": "no",
                "node_mode": "unavailable",
                "node_owner": "unavailable",
                "node_group": "unavailable",
            }
        )
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        self.assertEqual(oled.parse_facts(raw), dict(sorted(facts.items())))

        facts["node_character"] = "yes"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            oled.parse_facts(contradictory)

    def test_probe_never_opens_spi_requests_gpio_or_changes_the_screen(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=oled.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        self.assertIn("/sys/class/spidev/spidev1.0", oled.REMOTE_PROBE)
        self.assertIn("/dev/spidev1.0", oled.REMOTE_PROBE)
        for forbidden in (
            "spidev_test",
            "SPI_IOC",
            "i2cdetect",
            "gpioget",
            "gpioset",
            "gpiomon",
            "/sys/bus/spi/drivers/spidev/bind",
            "/sys/bus/spi/drivers/spidev/unbind",
            "cat /dev/spidev",
            "< /dev/spidev",
            "> /dev/spidev",
            "/proc/",
        ):
            self.assertNotIn(forbidden, oled.REMOTE_PROBE)

    def test_policy_records_metadata_only_scope(self) -> None:
        policy = oled.policy_receipt()
        self.assertIs(policy["rawSpiOpened"], False)
        self.assertIs(policy["spiIoctlsInvoked"], False)
        self.assertIs(policy["spiTransfersPerformed"], False)
        self.assertIs(policy["driverBindingChanged"], False)
        self.assertIs(policy["gpioLinesRequested"], False)
        self.assertIs(policy["gpioValuesRead"], False)
        self.assertIs(policy["screenStateChanged"], False)
        self.assertIs(policy["uniqueIdentifiersRead"], False)
        self.assertIs(policy["physicalActionsInvoked"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_oled_spi_topology.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
