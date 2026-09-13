from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
UNIT = "packaging/systemd/hardened-blikvm-hid-credential.service"
SERVICE = "hardened-blikvm-hid-credential.service"
USER = "hardened-blikvm-hid"
GATE = "bli-kvm-hid-credential-gate"


class HidCredentialServiceTests(unittest.TestCase):
    def test_unit_delivers_the_credential_to_a_dedicated_sandbox(self) -> None:
        unit = (ROOT / UNIT).read_text()
        self.assertIn("Type=oneshot", unit)
        self.assertIn(f"User={USER}", unit)
        self.assertIn("Group=hardened-blikvm", unit)
        self.assertIn("LoadCredential=kvmd-user:/etc/hardened-blikvm/hid/kvmd-user", unit)
        self.assertIn(
            "LoadCredential=kvmd-password:/etc/hardened-blikvm/hid/kvmd-password",
            unit,
        )
        self.assertIn(
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-hid-credential-gate",
            unit,
        )
        for hardening in (
            "NoNewPrivileges=yes",
            "PrivateNetwork=yes",
            "ProtectSystem=strict",
            "RestrictAddressFamilies=AF_UNIX",
            "CapabilityBoundingSet=",
            "SystemCallFilter=@system-service",
        ):
            self.assertIn(hardening, unit)
        self.assertNotIn("hardened-blikvm-server", unit)
        self.assertNotIn("RuntimeDirectory=", unit)

    def test_dedicated_account_exists_and_is_not_the_network_server(self) -> None:
        sysusers = (ROOT / "packaging/sysusers.d/hardened-blikvm.conf").read_text()
        self.assertIn(f"u {USER} -", sysusers)
        self.assertIn(f"m {USER} hardened-blikvm", sysusers)
        self.assertNotIn("hardened-blikvm-server - \"Hardened BliKVM HID", sysusers)

    def test_verifier_and_bundle_install_the_unit_and_gate(self) -> None:
        verifier = (ROOT / "scripts/verify-systemd-units.sh").read_text()
        self.assertIn(
            "usr/libexec/hardened-blikvm/bli-kvm-hid-credential-gate", verifier
        )
        self.assertIn(SERVICE, verifier)
        bundle = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn("bli-kvm-hid-credential-gate", bundle)
        self.assertIn(UNIT, bundle)
        self.assertIn(f'--arg hid_credential_gate_sha', bundle)

    def test_loader_is_fixed_path_and_redacted(self) -> None:
        source = (ROOT / "crates/bli-kvm-hid-credential/src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn(
            '"/run/credentials/hardened-blikvm-hid-credential.service"', production
        )
        self.assertIn("CREDENTIALS_DIRECTORY", production)
        self.assertIn("UnexpectedDirectory", production)
        for forbidden in ("/etc/", "Command", "println!", "eprintln!"):
            self.assertNotIn(forbidden, production)
        gate = (
            ROOT / "crates/bli-kvm-hid-credential/src/bin/bli-kvm-hid-credential-gate.rs"
        ).read_text()
        self.assertIn("hid-credential-delivery=ok", gate)
        self.assertNotIn("kvmd-password", gate)


if __name__ == "__main__":
    unittest.main()
