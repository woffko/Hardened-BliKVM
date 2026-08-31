import subprocess
import sys
import unittest

import tools.capture_gpio_line_baseline as gpio


def valid_facts() -> dict[str, str]:
    facts = {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "chip_present": "yes",
        "chip_character": "yes",
        "chip_mode": "660",
        "chip_owner": "root",
        "chip_group": "gpio",
        "sysfs_present": "yes",
        "sysfs_label": "pinctrl@300b000",
        "sysfs_ngpio": "288",
        "sysfs_base": "0",
        "gpioinfo_available": "yes",
        "gpioinfo_version": "gpioinfo (libgpiod) v1.6.3",
        "gpioinfo_probe": "success",
    }
    for offset in gpio.PROFILE_OFFSETS:
        facts[f"line{offset}_status"] = "captured"
        facts[f"line{offset}_info"] = f"line {offset}: unnamed unused input active-high"
        facts[f"line{offset}_legacy_present"] = "no"
        facts[f"line{offset}_direction"] = "unavailable"
        facts[f"line{offset}_active_low"] = "unavailable"
        facts[f"line{offset}_edge"] = "unavailable"
    facts.update(
        {
            "line269_info": 'line 269: unnamed "blikvm-fan" output active-high [used]',
            "line269_legacy_present": "yes",
            "line269_direction": "out",
            "line269_active_low": "0",
            "line269_edge": "none",
        }
    )
    return facts


class CaptureGpioLineBaselineTests(unittest.TestCase):
    def test_parser_accepts_only_complete_sanitized_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in reversed(facts.items())).encode()
        self.assertEqual(gpio.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_and_contradictory_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            gpio.parse_facts(raw + b"hostname=private\n")
        with self.assertRaises(ValueError):
            gpio.parse_facts(raw + b"line269_status=duplicate\n")
        facts["gpioinfo_available"] = "no"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            gpio.parse_facts(contradictory)

    def test_probe_uses_only_metadata_and_never_requests_or_drives_a_line(self) -> None:
        syntax = subprocess.run(
            ["sh", "-n"],
            input=gpio.REMOTE_PROBE.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        self.assertIn("gpioinfo", gpio.REMOTE_PROBE)
        for offset in gpio.PROFILE_OFFSETS:
            self.assertIn(str(offset), gpio.REMOTE_PROBE)
            self.assertNotIn(f"gpio{offset}/value", gpio.REMOTE_PROBE)
        for forbidden in (
            "gpioget",
            "gpioset",
            "gpiomon",
            "GPIO_GET_LINEHANDLE_IOCTL",
            "GPIO_V2_GET_LINE_IOCTL",
            "/sys/class/gpio/export",
            "/sys/class/gpio/unexport",
            "softPwm",
        ):
            self.assertNotIn(forbidden, gpio.REMOTE_PROBE)

    def test_policy_explicitly_records_metadata_only_scope(self) -> None:
        self.assertEqual(gpio.SCHEMA_VERSION, 2)
        policy = gpio.policy_receipt(True)
        self.assertIs(policy["gpioInfoCommandInvoked"], True)
        self.assertIs(policy["lineRequestPerformed"], False)
        self.assertIs(policy["lineValueRead"], False)
        self.assertIs(policy["lineConfigured"], False)
        self.assertIs(policy["physicalActionsInvoked"], False)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_gpio_line_baseline.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
