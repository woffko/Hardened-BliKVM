from pathlib import Path
import hashlib
import io
import json
import tarfile
import tempfile
import unittest

from tools import stage_device_restricted_control_profile as gate


ROOT = Path(__file__).resolve().parent.parent


def write_profile_bundle(
    path: Path,
    *,
    profile: str = "restricted-control-test",
    features: list[str] | None = None,
) -> str:
    manifest: dict[str, object] = {
        "schemaVersion": 1,
        "product": "Hardened BliKVM",
        "profile": profile,
        "sourceState": "committed-development-checkpoint",
        "sourceCommit": "a" * 40,
        "cargoFeatures": features
        if features is not None
        else list(gate.EXPECTED_FEATURES),
        "releaseAccepted": False,
        "deviceAccepted": False,
        "artifacts": {
            "server": {"path": "libexec/bli-kvm-server", "sha256": "b" * 64, "size": 1},
            "controlBroker": {
                "path": "libexec/bli-kvm-control-broker",
                "sha256": "c" * 64,
                "size": 1,
            },
        },
    }
    members = {
        "MANIFEST.json": json.dumps(manifest).encode(),
        "SHA256SUMS": b"",
        "libexec/bli-kvm-server": b"server",
        "libexec/bli-kvm-control-broker": b"broker",
        "share/systemd/hardened-blikvm-control-transition.service": b"unit",
        "share/tmpfiles.d/hardened-blikvm-control-transition.conf": b"tmpfiles",
        "share/server-dropin/90-restricted-control.conf": b"dropin",
        "share/web/index.html": b"<!doctype html>",
    }
    with tarfile.open(path, mode="w:gz") as archive:
        root = tarfile.TarInfo(gate.EXPECTED_ROOT)
        root.type = tarfile.DIRTYPE
        archive.addfile(root)
        for name, payload in members.items():
            info = tarfile.TarInfo(f"{gate.EXPECTED_ROOT}/{name}")
            info.size = len(payload)
            archive.addfile(info, io.BytesIO(payload))
    return hashlib.sha256(path.read_bytes()).hexdigest()


class RestrictedControlProfileTests(unittest.TestCase):
    def test_bundle_validator_accepts_the_expected_shape(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "profile.tar.gz"
            digest = write_profile_bundle(bundle)
            validated = gate.validate_profile_bundle(str(bundle), digest)
            self.assertEqual(validated.sha256, digest)
            self.assertEqual(validated.server_sha256, "b" * 64)
            self.assertEqual(validated.broker_sha256, "c" * 64)
            self.assertEqual(validated.runtime_root, f"/tmp/hardened-blikvm-rc-{digest[:16]}")

    def test_bundle_validator_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "profile.tar.gz"
            write_profile_bundle(bundle)
            with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
                gate.validate_profile_bundle(str(bundle), "0" * 64)
            wrong_profile = Path(directory) / "wrong.tar.gz"
            write_profile_bundle(wrong_profile, profile="prototype")
            with self.assertRaisesRegex(ValueError, "expected profile"):
                gate.validate_profile_bundle(
                    str(wrong_profile), hashlib.sha256(wrong_profile.read_bytes()).hexdigest()
                )
            wrong_features = Path(directory) / "features.tar.gz"
            write_profile_bundle(wrong_features, features=["hid-transition-profile"])
            with self.assertRaisesRegex(ValueError, "expected profile"):
                gate.validate_profile_bundle(
                    str(wrong_features), hashlib.sha256(wrong_features.read_bytes()).hexdigest()
                )
            unsafe = Path(directory) / "unsafe.tar.gz"
            with tarfile.open(unsafe, mode="w:gz") as archive:
                info = tarfile.TarInfo(f"{gate.EXPECTED_ROOT}/../evil")
                info.size = 1
                archive.addfile(info, io.BytesIO(b"x"))
            with self.assertRaisesRegex(ValueError, "unsafe member"):
                gate.validate_profile_bundle(
                    str(unsafe), hashlib.sha256(unsafe.read_bytes()).hexdigest()
                )

    def test_apply_script_runs_the_combined_profile_on_a_runtime_root(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "profile.tar.gz"
            digest = write_profile_bundle(bundle)
            validated = gate.validate_profile_bundle(str(bundle), digest)
        script = gate.build_apply_script(validated)
        self.assertIn(f"root={validated.runtime_root}", script)
        self.assertIn("LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user", script)
        self.assertIn("LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password", script)
        self.assertIn("BLI_KVM_HID_PROFILE=1", script)
        self.assertIn("BLI_KVM_ATX_PROFILE=1", script)
        self.assertIn("BLI_KVM_RESTRICTED_CONTROLS=1", script)
        self.assertIn(f"BLI_KVM_WEB_ROOT={validated.runtime_root}/share/web", script)
        self.assertIn(f"BLI_KVM_CONTROL_SOCKET={validated.runtime_root}/state/control.sock", script)
        self.assertIn("web_ready", script)
        source = (ROOT / "tools/stage_device_restricted_control_profile.py").read_text()
        self.assertIn('"web_assets_missing_restricted_gui"', source)
        for key in (
            "extracted",
            "broker_started",
            "control_socket_present",
            "server_started",
            "health",
            "hid_availability",
            "power_availability",
            "https_ready",
            "web_ready",
        ):
            self.assertIn(f'emit {key} ', script)
        self.assertNotIn("not-a-real", script)
        self.assertNotIn("/home/w0w", script)

    def test_teardown_removes_only_runtime_units_and_root(self) -> None:
        script = gate.build_teardown_script("/run/hardened-blikvm-rc-deadbeefdeadbeef")
        self.assertIn("/run/systemd/system/hardened-blikvm-rc-broker.service", script)
        self.assertIn("systemctl daemon-reload", script)
        self.assertIn("runtime_absent", script)
        self.assertIn("units_removed", script)

    def test_receipt_contract(self) -> None:
        keys = gate.Receipt.__annotations__
        for required in (
            "bundleSha256",
            "stage",
            "facts",
            "endpoint",
            "cleanupConfirmed",
            "failureStage",
            "failureType",
            "physicalActionsInvoked",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_restricted_control_profile.py").read_text()
        self.assertIn('"physicalActionsInvoked": False', source)
        self.assertIn('"persistentChangeScope": "runtime-only"', source)


if __name__ == "__main__":
    unittest.main()
