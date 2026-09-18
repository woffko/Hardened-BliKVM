import collections.abc
import io
import json
import sys
import tempfile
import unittest
import uuid
from pathlib import Path
from unittest import mock

from tools import stage_device_video_settings_gate as gate


def engine_facts(fps: object, quality: object = 0, online: object = "yes") -> str:
    return (
        f"engine_online={online}\nengine_fps={fps}\nengine_quality={quality}\n"
    )


class VideoSettingsGateTests(unittest.TestCase):
    def test_engine_probe_is_read_only(self) -> None:
        script = gate.engine_script()
        for forbidden in ("systemctl", "rm -", "install -m", "> /", "tee "):
            self.assertNotIn(forbidden, script)
        self.assertIn("curl -s --max-time 5 --unix-socket", script)
        self.assertIn("/state", script)
        self.assertIn("engine_fps=", script)
        self.assertIn("engine_quality=", script)

    def test_facts_parse_ignores_unexpected_lines(self) -> None:
        facts = gate.parse_facts("  indented=1\nengine_fps=24\nbad key=1\n")
        self.assertEqual(facts, {"engine_fps": "24"})

    def _paths(self) -> tuple[Path, Path]:
        directory = Path(tempfile.mkdtemp())
        known_hosts = directory / "known-hosts"
        known_hosts.write_text("192.0.2.10 ssh-ed25519 AAAA\n", encoding="utf-8")
        known_hosts.chmod(0o600)
        output = (
            Path(__file__).resolve().parent.parent
            / "evidence/private"
            / f"test-video-settings-gate-{uuid.uuid4().hex}.json"
        )
        return output, known_hosts

    def _argv(self, output: Path, known_hosts: Path, extra: list[str]) -> list[str]:
        return [
            "stage_device_video_settings_gate.py",
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
        settings_sequence: list[tuple[int, int]],
        engines: collections.abc.Sequence[collections.abc.Mapping[str, object]],
        *,
        apply_result: dict[str, object] | None = None,
    ) -> dict[str, object]:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=engine_facts(**facts).encode(), stderr=b"", returncode=0)
            for facts in engines
        ]
        settings_replies = list(settings_sequence) * 12
        api = mock.Mock()
        api.settings.side_effect = settings_replies
        api.apply.return_value = apply_result or {"applied": True}
        with (
            mock.patch.object(sys, "argv", self._argv(output, known_hosts, extra)),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
            mock.patch.object(gate.time, "sleep", self._no_sleep),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        receipt["returnCode"] = code
        return receipt

    @staticmethod
    def _no_sleep(_: float) -> None:
        return None

    def test_plan_mode_observes_without_applying(self) -> None:
        receipt = self._run(
            [],
            [(24, 80)],
            [{"fps": 24, "quality": 0}],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "observed")
        self.assertEqual(receipt["mode"], "plan")
        self.assertIs(receipt["mutationsPerformed"], False)
        self.assertEqual((receipt["initialFps"], receipt["initialQuality"]), (24, 80))
        self.assertEqual(receipt["engineFps"], 24)

    def test_apply_and_restore_holds_both_read_backs(self) -> None:
        receipt = self._run(
            ["--confirm", "--apply-fps", "24", "--apply-quality", "100"],
            [(40, 80), (24, 100), (40, 80)],
            [{"fps": 40, "quality": 0}, {"fps": 24, "quality": 0}, {"fps": 40, "quality": 0}],
        )
        self.assertEqual(receipt["returnCode"], 0)
        self.assertEqual(receipt["result"], "passed")
        self.assertIs(receipt["mutationsPerformed"], True)
        self.assertEqual((receipt["apiReadBackFps"], receipt["apiReadBackQuality"]), (24, 100))
        self.assertEqual(receipt["engineFps"], 24)
        self.assertEqual((receipt["restoredFps"], receipt["restoredQuality"]), (40, 80))
        self.assertEqual(receipt["engineRestoredFps"], 40)

    def test_a_rejected_apply_fails_closed(self) -> None:
        output, known_hosts = self._paths()
        replies = [
            mock.Mock(stdout=engine_facts(40).encode(), stderr=b"", returncode=0)
        ]
        api = mock.Mock()
        api.settings.return_value = (40, 80)
        api.apply.return_value = {"applied": False, "rejection": "capability_disabled"}
        with (
            mock.patch.object(
                sys, "argv", self._argv(output, known_hosts, ["--confirm"])
            ),
            mock.patch.object(gate.prototype, "run_ssh", side_effect=replies),
            mock.patch.object(sys, "stdin", io.BytesIO(b"ssh-secret\noperator-password\n")),
            mock.patch.object(gate, "BoardApi", return_value=api),
        ):
            code = gate.main()
        receipt = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(code, 1)
        self.assertEqual(receipt["failureType"], "fps_capability_disabled")

    def test_the_engine_read_back_must_agree_with_the_api(self) -> None:
        receipt = self._run(
            ["--confirm"],
            [(24, 100)],
            [{"fps": 40, "quality": 0}] + [{"fps": 10, "quality": 0}] * 12,
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureStage"], "verify")
        self.assertEqual(receipt["failureType"], "engine_fps_mismatch")

    def test_an_offline_engine_is_refused(self) -> None:
        receipt = self._run(
            [], [(40, 80)], [{"fps": 40, "quality": 0, "online": "no"}]
        )
        self.assertEqual(receipt["returnCode"], 1)
        self.assertEqual(receipt["failureType"], "engine_offline")


if __name__ == "__main__":
    unittest.main()
