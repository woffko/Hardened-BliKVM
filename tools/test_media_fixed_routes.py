from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
CLIENT = ROOT / "crates/bli-kvm-kvmd-media-client/src/lib.rs"
TRANSITION = ROOT / "crates/bli-kvm-media-transition/src/lib.rs"


def strip_comments(text: str) -> str:
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )


class MediaFixedRouteTests(unittest.TestCase):
    def test_client_exposes_only_the_two_fixed_media_routes(self) -> None:
        production = strip_comments(CLIENT.read_text().split("#[cfg(test)]", 1)[0])
        routes = set(re.findall(r'"(/msd/[a-z_]+)', production))
        self.assertEqual(routes, {"/msd/set_connected", "/msd/set_params"})
        lowered = production.lower()
        for forbidden in ("upload", "download", "remove", "delete", "reset", "/api/", "/dev/"):
            self.assertNotIn(forbidden, lowered)

    def test_transition_uses_the_fixed_client_and_catalog_only(self) -> None:
        production = strip_comments(TRANSITION.read_text().split("#[cfg(test)]", 1)[0])
        self.assertIn("KvmdMediaClient", production)
        self.assertIn("MediaCatalog", production)
        for forbidden in ("/dev/", "std::process"):
            self.assertNotIn(forbidden, production)


if __name__ == "__main__":
    unittest.main()
