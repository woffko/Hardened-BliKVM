from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
UNIT = "packaging/systemd/hardened-blikvm-hid-transition.service"
UNIT_NAME = "hardened-blikvm-hid-transition.service"
TMPFILES = "packaging/tmpfiles.d/hardened-blikvm-hid.conf"


class HidTransitionServiceTests(unittest.TestCase):
    def test_unit_runs_the_transition_profile_under_a_dedicated_sandbox(self) -> None:
        unit = (ROOT / UNIT).read_text()
        self.assertIn("Type=simple", unit)
        self.assertIn("User=hardened-blikvm-hid", unit)
        self.assertIn("Group=hardened-blikvm", unit)
        self.assertIn("SupplementaryGroups=kvmd", unit)
        self.assertIn("Environment=BLI_KVM_HID_PROFILE=1", unit)
        self.assertIn(
            "Environment=BLI_KVM_HID_LEDGER=/var/lib/hardened-blikvm-hid/ledger.json",
            unit,
        )
        self.assertIn("Environment=BLI_KVM_HID_KVMD_SOCKET=/run/kvmd/kvmd.sock", unit)
        self.assertIn("LoadCredential=kvmd-user:/etc/hardened-blikvm/kvmd-user", unit)
        self.assertIn(
            "LoadCredential=kvmd-password:/etc/hardened-blikvm/kvmd-password", unit
        )
        self.assertIn(
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-control-broker", unit
        )
        self.assertIn(
            "ReadWritePaths=/run/hardened-blikvm-hid /var/lib/hardened-blikvm-hid",
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

    def test_ledger_directory_and_account_are_declared(self) -> None:
        tmpfiles = (ROOT / TMPFILES).read_text()
        self.assertIn("d /var/lib/hardened-blikvm-hid 0700", tmpfiles)
        self.assertIn("hardened-blikvm-hid hardened-blikvm", tmpfiles)
        sysusers = (ROOT / "packaging/sysusers.d/hardened-blikvm.conf").read_text()
        self.assertIn("u hardened-blikvm-hid -", sysusers)

    def test_verifier_covers_the_unit_and_tmpfiles(self) -> None:
        verifier = (ROOT / "scripts/verify-systemd-units.sh").read_text()
        self.assertIn(UNIT_NAME, verifier)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-hid.conf", verifier)

    def test_default_prototype_bundle_excludes_the_transition_profile(self) -> None:
        default_bundle = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertNotIn(UNIT_NAME, default_bundle)
        self.assertNotIn("BLI_KVM_CARGO_FEATURES", default_bundle)
        profile_bundle = (ROOT / "scripts/build-hid-transition-profile.sh").read_text()
        self.assertIn(UNIT_NAME, profile_bundle)


if __name__ == "__main__":
    unittest.main()
