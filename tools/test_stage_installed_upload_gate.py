import copy
from pathlib import Path
import tempfile
from typing import cast
import unittest
from unittest.mock import patch

from tools import stage_installed_upload_gate as driver


def accepted() -> dict[str, object]:
    return {"guardRoot": "/var/lib/" + driver.native.PREFIX + "a" * 24,
            "result": "passed", "guardMayRemain": False, "stateAndServingPreserved": True,
            "vectors": [{"mode": mode, "result": "reply-unobserved" if mode == "lost-reply" else "passed",
                         "worker": {"naturalExit": True}, "live": {key: True for key in
                          ("dedicatedIdentity", "emptyCapabilities", "anonymousSpool", "sandboxVerified")}}
                        for mode in driver.native.MODES]}


class StageInstalledUploadGateTests(unittest.TestCase):
    def test_acceptance_requires_every_vector_live_worker_and_cleanup(self) -> None:
        driver.validate_result(accepted())
        cases: tuple[tuple[str, object], ...] = (("guardMayRemain", True), ("stateAndServingPreserved", False), ("result", "failed"), ("vectors", []))
        for key, value in cases:
            changed = accepted()
            changed[key] = value
            with self.assertRaises(ValueError):
                driver.validate_result(changed)
        for field in ("worker", "live", "mode"):
            changed = accepted()
            vectors = cast(list[dict[str, object]], changed["vectors"])
            vectors[0][field] = {} if field != "mode" else "foreign"
            with self.assertRaises(ValueError):
                driver.validate_result(changed)
        changed = copy.deepcopy(accepted())
        cast(list[object], changed["vectors"])[0] = None
        with self.assertRaises(ValueError):
            driver.validate_result(changed)

    def test_wrong_archive_rejected_before_uploading_probe_sources(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            archive = Path(temporary) / "wrong.tar"
            archive.write_bytes(b"not the accepted signed archive")
            with patch.object(driver.packages, "select_profile") as selected, \
                 patch.object(driver.packages, "upload_payloads", return_value={}):
                with self.assertRaises(ValueError):
                    driver.build_uploads("old", "new", str(archive))
                selected.assert_called_once_with("dev7-dev9")


if __name__ == "__main__":
    unittest.main()
