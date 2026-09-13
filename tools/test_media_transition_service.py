from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
UNIT = "packaging/systemd/hardened-blikvm-media-transition.service"
UNIT_NAME = "hardened-blikvm-media-transition.service"
TMPFILES = "packaging/tmpfiles.d/hardened-blikvm-media.conf"


class MediaTransitionServiceTests(unittest.TestCase):
    def test_unit_runs_the_media_profile_under_the_control_sandbox(self) -> None:
        unit = (ROOT / UNIT).read_text()
        self.assertIn("Type=simple", unit)
        self.assertIn("User=hardened-blikvm-control", unit)
        self.assertIn("Group=hardened-blikvm", unit)
        self.assertIn("SupplementaryGroups=kvmd", unit)
        self.assertIn("Environment=BLI_KVM_MEDIA_PROFILE=1", unit)
        self.assertIn(
            "Environment=BLI_KVM_MEDIA_JOURNAL=/var/lib/hardened-blikvm-media/journal.json",
            unit,
        )
        self.assertIn(
            "Environment=BLI_KVM_MEDIA_CATALOG=/etc/hardened-blikvm-media/catalog.json",
            unit,
        )
        self.assertIn("Environment=BLI_KVM_MEDIA_KVMD_SOCKET=/run/kvmd/kvmd.sock", unit)
        self.assertIn("LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user", unit)
        self.assertIn(
            "LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password", unit
        )
        self.assertIn(
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-control-broker", unit
        )
        self.assertIn(
            "ReadWritePaths=/run/hardened-blikvm-media /var/lib/hardened-blikvm-media",
            unit,
        )
        for hardening in (
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "RestrictAddressFamilies=AF_UNIX",
            "CapabilityBoundingSet=",
            "SystemCallFilter=@system-service",
            "IPAddressDeny=any",
        ):
            self.assertIn(hardening, unit)
        self.assertNotIn("User=hardened-blikvm-server", unit)
        self.assertNotIn("password=", unit.lower())
        self.assertNotIn("not-a-real", unit)

    def test_state_directory_is_declared(self) -> None:
        tmpfiles = (ROOT / TMPFILES).read_text()
        self.assertIn("d /var/lib/hardened-blikvm-media 0700", tmpfiles)
        self.assertIn("hardened-blikvm-control hardened-blikvm", tmpfiles)

    def test_verifier_covers_the_unit_and_tmpfiles(self) -> None:
        verifier = (ROOT / "scripts/verify-systemd-units.sh").read_text()
        self.assertIn(UNIT_NAME, verifier)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-media.conf", verifier)

    def test_default_prototype_bundle_excludes_the_media_profile(self) -> None:
        default_bundle = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertNotIn(UNIT_NAME, default_bundle)


if __name__ == "__main__":
    unittest.main()
