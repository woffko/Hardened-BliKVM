import unittest

from tools import license_inventory


def metadata(packages: list[dict[str, object]]) -> dict[str, object]:
    return {"packages": packages}


class LicenseInventoryTests(unittest.TestCase):
    def test_inventory_is_deterministic_and_flags_missing_licenses(self) -> None:
        inventory = license_inventory.build_inventory(
            metadata(
                [
                    {"name": "zeta", "version": "0.2.0", "license": "MIT", "source": "registry"},
                    {"name": "alpha", "version": "0.1.0", "license": "Apache-2.0", "source": None},
                    {"name": "beta", "version": "0.3.0", "license": None, "source": "registry"},
                ]
            )
        )
        self.assertEqual(inventory["packageCount"], 3)
        self.assertEqual(
            [entry["name"] for entry in inventory["packages"]], ["alpha", "beta", "zeta"]
        )
        self.assertTrue(inventory["packages"][0]["workspace"])
        self.assertFalse(inventory["packages"][1]["workspace"])
        self.assertEqual(inventory["missingLicense"], ["beta@0.3.0"])

    def test_license_file_counts_as_declared(self) -> None:
        inventory = license_inventory.build_inventory(
            metadata(
                [
                    {
                        "name": "gamma",
                        "version": "0.1.0",
                        "license": None,
                        "license_file": "LICENSE",
                        "source": "registry",
                    }
                ]
            )
        )
        self.assertEqual(inventory["missingLicense"], [])

    def test_invalid_metadata_fails_closed(self) -> None:
        bad_inputs: list[dict[str, object]] = [
            {},
            {"packages": "x"},
            {"packages": [{"name": "x"}]},
        ]
        for bad in bad_inputs:
            with self.assertRaises(ValueError):
                _ = license_inventory.build_inventory(bad)


if __name__ == "__main__":
    unittest.main()
