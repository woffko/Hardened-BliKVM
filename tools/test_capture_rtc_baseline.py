import subprocess
import sys
import unittest

import tools.capture_rtc_baseline as rtc


def valid_facts() -> dict[str, str]:
    facts = {
        "architecture": "aarch64",
        "rootfs_mode": "rw",
        "rtc_count": "2",
    }
    values = (
        {
            "present": "yes",
            "name": "sun50i-rtc",
            "device": "rtc",
            "driver": "sun6i-rtc",
            "class_dev": "248:0",
            "hctosys": "1",
            "date": "2026-08-31",
            "time": "00:42:00",
            "since_epoch": "1788136920",
            "dev_present": "yes",
            "dev_mode": "660",
            "dev_owner": "root",
            "dev_group": "root",
            "dev_character": "yes",
        },
        {
            "present": "yes",
            "name": "rtc-pcf8563",
            "device": "0-0051",
            "driver": "rtc-pcf8563",
            "class_dev": "248:1",
            "hctosys": "0",
            "date": "2026-08-31",
            "time": "00:41:59",
            "since_epoch": "1788136919",
            "dev_present": "yes",
            "dev_mode": "660",
            "dev_owner": "root",
            "dev_group": "root",
            "dev_character": "yes",
        },
    )
    for index, rtc_values in enumerate(values):
        for field, value in rtc_values.items():
            facts[f"rtc{index}_{field}"] = value
    return facts


class CaptureRtcBaselineTests(unittest.TestCase):
    def test_parser_accepts_only_complete_bounded_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in reversed(facts.items())).encode()
        self.assertEqual(rtc.parse_facts(raw), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_or_contradictory_facts(self) -> None:
        facts = valid_facts()
        raw = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            rtc.parse_facts(raw + b"hostname=private\n")
        with self.assertRaises(ValueError):
            rtc.parse_facts(raw + b"rtc0_name=duplicate\n")
        facts["rtc1_present"] = "no"
        contradictory = "".join(f"{key}={value}\n" for key, value in facts.items()).encode()
        with self.assertRaises(ValueError):
            rtc.parse_facts(contradictory)

    def test_remote_probe_has_no_raw_bus_device_or_write_operation(self) -> None:
        probe = rtc.REMOTE_PROBE
        syntax = subprocess.run(
            ["sh", "-n"],
            input=probe.encode(),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr.decode())
        for forbidden in (
            "i2cdetect",
            "/dev/i2c",
            "new_device",
            "hwclock",
            "RTC_SET_TIME",
            "timedatectl",
            "date -s",
            "/dev/rtc$index\" <",
        ):
            self.assertNotIn(forbidden, probe)
        self.assertIn("/sys/class/rtc/rtc$index", probe)
        self.assertIn("[ -c \"$node\" ]", probe)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/capture_rtc_baseline.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
