from pathlib import Path
from typing import cast
import unittest

from tools import build_device_media_catalog as catalog


ROOT = Path(__file__).resolve().parent.parent


class BuildDeviceMediaCatalogTests(unittest.TestCase):
    def test_listing_is_parsed_and_filtered(self) -> None:
        raw = b"kubuntu.img\t6841958400\nrescue.iso\t1024\n"
        self.assertEqual(
            catalog.parse_listing(raw),
            [("kubuntu.img", 6841958400), ("rescue.iso", 1024)],
        )
        for bad in (b"../evil.iso\t1\n", b"no-tab\n", b"name\tnotanumber\n", b""):
            with self.assertRaises(ValueError):
                catalog.parse_listing(bad)

    def test_digests_must_be_exact_hex(self) -> None:
        digest = "a" * 64
        self.assertEqual(catalog.parse_digests(f"{digest}\n".encode(), 1), [digest])
        with self.assertRaises(ValueError):
            catalog.parse_digests(b"short\n", 1)
        with self.assertRaises(ValueError):
            catalog.parse_digests(b"", 1)

    def test_catalog_uses_deterministic_opaque_ids(self) -> None:
        entries = [("a.iso", 10), ("b.img", 20)]
        digests = ["1" * 64, "2" * 64]
        document = catalog.build_catalog(entries, digests)
        self.assertEqual(document["schemaVersion"], 1)
        images = cast(list[dict[str, object]], document["entries"])
        self.assertEqual(images[0]["imageId"], 1)
        self.assertEqual(images[1]["imageId"], 2)
        self.assertEqual(images[0]["imageName"], "a.iso")
        self.assertFalse(images[0]["writableAllowed"])
        self.assertEqual(images[1]["sha256"], "2" * 64)

    def test_tool_is_read_only(self) -> None:
        source = (ROOT / "tools/build_device_media_catalog.py").read_text(encoding="utf-8")
        for forbidden in ("set_params", "set_connected", "rm -", "/msd/set"):
            self.assertNotIn(forbidden, source)
        self.assertIn("write_json_0600_atomic", source)
        self.assertIn("prototype.run_ssh", source)


if __name__ == "__main__":
    unittest.main()
