import collections.abc
import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from typing import Any
from unittest import mock

from tools import capture_device_video_matrix as matrix


def engine_facts(
    fps: object,
    resolution: object = "1280x720",
    captured: object = 10,
    queued: object = 0,
    online: object = "yes",
) -> str:
    return (
        f"engine_online={online}\nengine_fps={fps}\nengine_quality=0\n"
        f"engine_resolution={resolution}\nengine_captured_fps={captured}\n"
        f"engine_queued_fps={queued}\n"
    )


class VideoMatrixTests(unittest.TestCase):
    @staticmethod
    def _no_sleep(_seconds: float) -> None:
        return None

    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-video-matrix-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "capture_device_video_matrix.py",
            "--endpoint",
            "ssh://192.0.2.10",
            "--username",
            "root",
            "--known-hosts",
            str(known_hosts),
            "--output",
            str(output),
            "--base-url",
            "https://192.0.2.10",
            "--api-username",
            "admin",
            "--timeout-seconds",
            "30",
        ] + extra

    def _run(
        self,
        extra: list[str],
        *,
        settings: collections.abc.Sequence[tuple[int, int]],
        engines: collections.abc.Sequence[str],
        captured: object = 10,
        resolutions: tuple[int, ...] = (720,),
        fps_values: tuple[int, ...] = (30,),
    ) -> dict[str, Any]:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=reply.encode(), stderr=b"", returncode=0)
            for reply in engines
        ]
        replies = replies + [replies[-1]] * 40
        settings_replies = list(settings) + [settings[-1]] * 40
        api = mock.Mock()
        api.settings.side_effect = settings_replies
        api.captured_fps.return_value = captured
        api.apply.return_value = {"applied": True}
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(matrix.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(matrix, "BoardApi", return_value=api),
            mock.patch.object(matrix, "REVIEWED_RESOLUTIONS", resolutions),
            mock.patch.object(matrix, "REQUESTED_FPS", fps_values),
            mock.patch.object(matrix.time, "sleep", self._no_sleep),
        ):
            code = matrix.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    def test_plan_mode_observes_without_applying(self) -> None:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.settings.return_value = (60, 100)
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, [])),
            mock.patch.object(
                matrix.prototype,
                "run_ssh",
                return_value=mock.Mock(
                    stdout=engine_facts(60).encode(), stderr=b"", returncode=0
                ),
            ),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\n")),
            mock.patch.object(matrix, "BoardApi", return_value=api),
        ):
            code = matrix.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertEqual(receipt["rows"], [])
        api.apply.assert_not_called()

    def test_the_sweep_records_a_row_and_restores(self) -> None:
        receipt = self._run(
            ["--confirm"],
            settings=[(60, 100), (30, 100), (60, 100)],
            engines=[
                engine_facts(60),
                engine_facts(30, captured=10, queued=9),
                engine_facts(60),
            ],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["probe"], "hardened-blikvm-video-matrix-gate")
        self.assertEqual(receipt["result"], "passed")
        rows = receipt["rows"]
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]["resolutionHeight"], 720)
        self.assertEqual(rows[0]["requestedFps"], 30)
        self.assertEqual(rows[0]["engineResolution"], "1280x720")
        self.assertEqual(rows[0]["engineCapturedFps"], 10)
        self.assertEqual(rows[0]["engineQueuedFps"], 9)
        self.assertIs(rows[0]["capturedMatchesServer"], True)
        self.assertEqual(receipt["restoredFps"], 60)
        self.assertEqual(receipt["restoredResolution"], "1280x720")

    def test_a_captured_mismatch_fails_closed_and_restores(self) -> None:
        receipt = self._run(
            ["--confirm"],
            settings=[(60, 100), (30, 100), (60, 100)],
            engines=[engine_facts(60)]
            + [engine_facts(30, captured=3)] * 13
            + [engine_facts(60)],
            captured=0,
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["result"], "failed")
        self.assertEqual(receipt["failureStage"], "sweep")
        self.assertEqual(receipt["failureType"], "server_captured_mismatch_720_30")
        self.assertEqual(receipt["restoredFps"], 60)
        self.assertEqual(receipt["restoredResolution"], "1280x720")

    def test_an_offline_engine_is_refused(self) -> None:
        output, known_hosts = self._paths()
        api = mock.Mock()
        api.settings.return_value = (60, 100)
        with (
            mock.patch.object(
                sys, "argv", self._argv(output, known_hosts, ["--confirm"])
            ),
            mock.patch.object(
                matrix.prototype,
                "run_ssh",
                return_value=mock.Mock(
                    stdout=engine_facts(60, online="no").encode(),
                    stderr=b"",
                    returncode=0,
                ),
            ),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(matrix, "BoardApi", return_value=api),
            mock.patch.object(matrix.time, "sleep", self._no_sleep),
        ):
            code = matrix.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 1)
        self.assertEqual(receipt["failureType"], "engine_offline")


if __name__ == "__main__":
    unittest.main()
