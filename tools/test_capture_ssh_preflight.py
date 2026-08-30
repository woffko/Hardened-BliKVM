import unittest

from tools import capture_ssh_preflight as preflight


class CaptureSshPreflightTests(unittest.TestCase):
    def test_endpoint_and_username_validation(self) -> None:
        self.assertEqual(preflight.parse_endpoint("ssh://device.invalid:22"), ("device.invalid", 22))
        self.assertEqual(preflight.validate_username("root"), "root")
        for endpoint in (
            "https://device.invalid",
            "ssh://root:secret@device.invalid",
            "ssh://device.invalid/path",
        ):
            with self.assertRaises(ValueError):
                _ = preflight.parse_endpoint(endpoint)

    def test_parser_accepts_only_complete_allowlisted_facts(self) -> None:
        raw = "\n".join(f"{key}=available" for key in sorted(preflight.SAFE_KEYS)).encode()
        facts = preflight.parse_safe_facts(raw)
        self.assertEqual(set(facts), preflight.SAFE_KEYS)

    def test_parser_rejects_unknown_duplicate_or_unsafe_values(self) -> None:
        with self.assertRaises(ValueError):
            _ = preflight.parse_safe_facts(b"password=secret\n")
        with self.assertRaises(ValueError):
            _ = preflight.parse_safe_facts(b"kernel=one\nkernel=two\n")
        with self.assertRaises(ValueError):
            _ = preflight.parse_safe_facts(b"kernel=value\nwith-new-line\n")


if __name__ == "__main__":
    _ = unittest.main()
