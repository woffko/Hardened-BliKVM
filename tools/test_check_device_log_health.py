import unittest

from tools import check_device_log_health as health


def good_facts() -> dict[str, str]:
    return {
        "architecture": "ok",
        "failed_units": "0",
        "journal_mount_source": "/dev/mmcblk0p3/hardened-blikvm-logs/journal",
        "journal_on_dedicated": "yes",
        "journald_active": "yes",
        "journald_storage": "persistent",
        "logrotate_service": "ok",
        "logrotate_valid": "yes",
        "previous_boot_journal": "no",
    }


class LogHealthTests(unittest.TestCase):
    def test_healthy_state_passes(self) -> None:
        health.validate_facts(good_facts())

    def test_failures_fail_closed(self) -> None:
        cases = {
            "logrotate_valid": "no",
            "logrotate_service": "failed",
            "journald_storage": "volatile",
            "journald_active": "no",
            "journal_on_dedicated": "no",
            "failed_units": "2",
        }
        for key, value in cases.items():
            facts = good_facts()
            facts[key] = value
            with self.assertRaises(health.GateError):
                health.validate_facts(facts)

    def test_script_reads_only_the_expected_facts(self) -> None:
        script = health.build_script()
        for key in health.FACT_KEYS:
            self.assertIn(key, script)
        self.assertIn("logrotate -d /etc/logrotate.conf", script)
        self.assertIn("journalctl -b -1", script)
        self.assertNotIn("systemctl restart", script)


if __name__ == "__main__":
    unittest.main()
