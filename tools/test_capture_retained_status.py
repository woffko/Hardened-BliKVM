from pathlib import Path
import unittest
from unittest.mock import patch

from tools import capture_retained_status as observer


class RetainedStatusObservationTests(unittest.TestCase):
    def test_read_only_observation_binds_cleanup_wiring_and_order(self) -> None:
        root = "/var/lib/" + observer.wiring.PREFIX + "a" * 24

        def property_value(unit: str, _name: str) -> str:
            return "1000" if unit == observer.wiring.s.CURRENT_SERVER else "900"

        with patch.object(observer.wiring, "exists", return_value=False), patch.object(Path, "glob", return_value=[]), \
             patch.object(observer.wiring, "validate_dropins") as validate, \
             patch.object(observer.wiring.common, "snapshot", return_value={}) as snapshot, \
             patch.object(observer.wiring.s, "property_value", side_effect=property_value), \
             patch.object(Path, "read_text", return_value="150.5 0"), patch.object(observer.wiring, "boot_identity", return_value="b" * 64), \
             patch.object(observer.wiring.u, "digest", return_value="c" * 64), patch.object(observer.wiring, "checked") as command:
            result = observer.capture(root)
            self.assertTrue(result["guardRemoved"])
            self.assertTrue(result["persistentWiringVerified"])
            self.assertEqual(result["uptimeSeconds"], 150)
            validate.assert_called_once_with(True)
            snapshot.assert_called_once_with(True)
            command.assert_not_called()
            with patch.object(observer.wiring, "exists", return_value=True), self.assertRaises(ValueError):
                observer.capture(root)
            with patch.object(observer.wiring.s, "property_value", return_value="0"), self.assertRaises(ValueError):
                observer.capture(root)
            with self.assertRaises(ValueError):
                observer.capture("/var/lib")


if __name__ == "__main__":
    unittest.main()
