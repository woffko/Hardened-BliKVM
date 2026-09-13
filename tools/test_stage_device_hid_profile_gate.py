from pathlib import Path
import hashlib
import io
import json
import tarfile
import tempfile
import unittest

from tools import stage_device_hid_profile_gate as gate


ROOT = Path(__file__).resolve().parent.parent


def write_profile_bundle(path: Path, *, profile: str = "hid-transition-test") -> str:
    manifest: dict[str, object] = {
        "schemaVersion": 1,
        "product": "Hardened BliKVM",
        "profile": profile,
        "sourceState": "committed-development-checkpoint",
        "sourceCommit": "a" * 40,
        "releaseAccepted": False,
        "deviceAccepted": False,
        "artifacts": {},
    }
    members = {
        "MANIFEST.json": json.dumps(manifest).encode(),
        "SHA256SUMS": b"",
        "libexec/bli-kvm-control-broker": b"broker",
        "share/systemd/hardened-blikvm-hid-transition.service": b"unit",
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


class HidProfileGateTests(unittest.TestCase):
    def paths(self) -> gate.GatePaths:
        return gate.gate_paths("a" * 64, "0" * 16)

    def test_profile_bundle_validator_accepts_the_expected_shape(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            bundle = Path(directory) / "profile.tar.gz"
            digest = write_profile_bundle(bundle)
            validated = gate.validate_profile_bundle(str(bundle), digest)
            self.assertEqual(validated.sha256, digest)

    def test_profile_bundle_validator_fails_closed(self) -> None:
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
            unsafe = Path(directory) / "unsafe.tar.gz"
            with tarfile.open(unsafe, mode="w:gz") as archive:
                info = tarfile.TarInfo(f"{gate.EXPECTED_ROOT}/../evil")
                info.size = 1
                archive.addfile(info, io.BytesIO(b"x"))
            with self.assertRaisesRegex(ValueError, "unsafe member"):
                gate.validate_profile_bundle(
                    str(unsafe), hashlib.sha256(unsafe.read_bytes()).hexdigest()
                )

    def test_preflight_checks_arch_and_clean_root(self) -> None:
        script = gate.build_preflight_script(self.paths())
        self.assertIn("emit architecture", script)
        self.assertIn("test_root_absent", script)

    def test_apply_proves_credential_requirement_and_startup(self) -> None:
        script = gate.build_apply_script(self.paths())
        self.assertIn("tar -xzf", script)
        self.assertIn("env -u CREDENTIALS_DIRECTORY", script)
        self.assertIn("BLI_KVM_HID_PROFILE=1", script)
        self.assertIn("refused_without_credentials", script)
        self.assertIn("CREDENTIALS_DIRECTORY=\"$root/cred\"", script)
        self.assertIn("started_with_credentials", script)
        self.assertIn("kill \"$pid\"", script)
        self.assertNotIn("/run/kvmd/kvmd.sock", script)
        self.assertNotIn("systemd-run", script)

    def test_rollback_kills_the_process_and_removes_only_its_stage(self) -> None:
        script = gate.build_rollback_script(self.paths())
        self.assertIn("broker.pid", script)
        self.assertIn('rm -rf -- "$stage_root"', script)
        self.assertIn("process_stopped", script)

    def test_receipt_contract(self) -> None:
        keys = gate.Receipt.__annotations__
        for required in (
            "bundleSha256",
            "gateSha256",
            "facts",
            "cleanupConfirmed",
            "failureStage",
            "failureType",
            "physicalActionsInvoked",
            "persistentChangeScope",
        ):
            self.assertIn(required, keys)
        source = (ROOT / "tools/stage_device_hid_profile_gate.py").read_text()
        self.assertIn('"physicalActionsInvoked": False', source)
        self.assertIn('"persistentChangeScope": "none"', source)
        self.assertIn('"refused_without_credentials"', source)


if __name__ == "__main__":
    unittest.main()
