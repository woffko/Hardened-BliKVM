from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
API = ROOT / "web/src/api/virtual-media.ts"
CAPS = ROOT / "web/src/lib/capabilities.ts"
INDEX = ROOT / "web/src/pages/desktop/menu/image/index.tsx"
HARDENED = ROOT / "web/src/pages/desktop/menu/image/hardened.tsx"


class HardenedMediaGuiTests(unittest.TestCase):
    def test_api_uses_opaque_ids_only(self) -> None:
        source = API.read_text(encoding="utf-8")
        self.assertIn("'/api/virtual-media/catalog'", source)
        self.assertIn("'/api/virtual-media'", source)
        self.assertIn("imageId", source)
        for forbidden in ("file", "path", "filename", "sha256"):
            self.assertNotIn(forbidden, source.lower())

    def test_image_menu_switches_to_the_hardened_panel(self) -> None:
        caps = CAPS.read_text(encoding="utf-8")
        self.assertIn("supportsHardenedMedia", caps)
        index = INDEX.read_text(encoding="utf-8")
        self.assertIn("supportsHardenedMedia(manifest)", index)
        self.assertIn("<HardenedImage />", index)
        self.assertIn("const LegacyImage", index)

    def test_hardened_panel_submits_typed_commands(self) -> None:
        source = HARDENED.read_text(encoding="utf-8")
        self.assertIn("submitVirtualMedia", source)
        self.assertIn("newOperationId", source)
        self.assertIn("action: 'select'", source)
        self.assertIn("action: 'set_connected'", source)
        self.assertNotIn("'/api/storage", source)
        self.assertNotIn("from '@/api/storage", source)


if __name__ == "__main__":
    unittest.main()
