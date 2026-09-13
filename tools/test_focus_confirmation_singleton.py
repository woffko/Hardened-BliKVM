from pathlib import Path
import re
import unittest


ROOT = Path(__file__).resolve().parent.parent
HOOK = ROOT / "web/src/hooks/useFocusConfirmation.ts"


class FocusConfirmationSingletonTests(unittest.TestCase):
    def test_confirmation_is_shared_across_queued_reports(self) -> None:
        source = HOOK.read_text(encoding="utf-8")
        # The decision must live outside the hook so queued reports created in an
        # earlier render still observe the operator's confirmation.
        self.assertRegex(source, re.compile(r"^let confirmedGlobal = false;", re.MULTILINE))
        self.assertRegex(
            source,
            re.compile(r"^let pendingConfirmation: Promise<boolean> \| null = null;", re.MULTILINE),
        )
        self.assertIn("if (confirmedGlobal) {", source)
        self.assertIn("if (pendingConfirmation) {", source)
        self.assertIn("confirmedGlobal = true;", source)
        self.assertIn("pendingConfirmation = pending;", source)
        self.assertIn("pendingConfirmation = null;", source)

    def test_confirmation_does_not_capture_a_stale_value(self) -> None:
        source = HOOK.read_text(encoding="utf-8")
        # The ensureConfirmed callback must not depend on the render-scoped
        # `confirmed` value; doing so re-created it and reopened the modal.
        callback = source[source.index("const ensureConfirmed = useCallback") :]
        self.assertNotIn("[confirmed, setConfirmed]", callback)
        self.assertIn("[setConfirmed]", callback)

    def test_no_secret_or_endpoint_is_hardcoded(self) -> None:
        source = HOOK.read_text(encoding="utf-8")
        for forbidden in ("password", "10.0.", "token", "http://", "https://"):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
