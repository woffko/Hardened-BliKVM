from pathlib import Path
import unittest

import tools.runtime_provisioning_policy as policy
import tools.stage_device_debian_runtime_package as package_gate


class RuntimeProvisioningPolicyTests(unittest.TestCase):
    def package(self, digest: str) -> package_gate.PackageInfo:
        return package_gate.PackageInfo(
            path=Path("/package.deb"),
            sha256="a" * 64,
            size_bytes=4096,
            source_commit="b" * 40,
            source_bundle_sha256="c" * 64,
            builder_commit="d" * 40,
            installed_files={
                policy.INSTALLED_SYSUSERS_PATH: (digest, 0o644),
            },
        )

    def test_fixed_source_matches_retained_package_copy(self) -> None:
        payload = policy.SYSUSERS_SOURCE.read_bytes()
        digest, size = policy.validate_sysusers_payload(payload)
        self.assertEqual(
            digest,
            "8685ad67a137da51a1640db06ef630a4ddaf3d09663640dd081d8a90c1997620",
        )
        self.assertEqual(size, len(payload))
        info = policy.load_sysusers_source(self.package(digest))
        self.assertEqual(info.payload, payload)
        self.assertEqual(info.sha256, digest)

    def test_complete_identity_set_includes_inert_update_identity(self) -> None:
        self.assertEqual(len(policy.ACCOUNT_NAMES), 5)
        self.assertEqual(len(policy.GROUP_NAMES), 6)
        self.assertIn("hardened-blikvm-update", policy.ACCOUNT_NAMES)
        self.assertEqual(policy.KVMD_ACCOUNT, "hardened-blikvm-broker")
        self.assertEqual(policy.SHARED_GROUP, "hardened-blikvm")

    def test_altered_or_reordered_sysusers_source_is_rejected(self) -> None:
        payload = policy.SYSUSERS_SOURCE.read_bytes()
        with self.assertRaisesRegex(ValueError, "fixed identity policy"):
            policy.validate_sysusers_payload(payload.replace(b" - ", b" 992 ", 1))
        lines = payload.splitlines(keepends=True)
        with self.assertRaisesRegex(ValueError, "fixed identity policy"):
            policy.validate_sysusers_payload(b"".join((*lines[:2], lines[3], lines[2], *lines[4:])))
        with self.assertRaisesRegex(ValueError, "encoding"):
            policy.validate_sysusers_payload(payload + b"\x00")
        with self.assertRaisesRegex(ValueError, "encoding"):
            policy.validate_sysusers_payload(payload.replace(b"\n", b"\r\n", 1))
        with self.assertRaisesRegex(ValueError, "newline"):
            policy.validate_sysusers_payload(payload.rstrip(b"\n"))

    def test_package_binding_and_credential_contract_fail_closed(self) -> None:
        payload = policy.SYSUSERS_SOURCE.read_bytes()
        _ = policy.validate_sysusers_payload(payload)
        with self.assertRaisesRegex(ValueError, "does not match"):
            policy.load_sysusers_source(self.package("e" * 64))
        policy.validate_credential_username(b"hardened-blikvm-broker\n")
        with self.assertRaisesRegex(ValueError, "username"):
            policy.validate_credential_username(b"admin\n")
        self.assertEqual(
            str(policy.dedicated_payload_path(policy.USERNAME_PATH)),
            policy.USERNAME_PATH,
        )
        with self.assertRaisesRegex(ValueError, "fixed configuration root"):
            policy.dedicated_payload_path("/etc/kvmd/htpasswd")


if __name__ == "__main__":
    _ = unittest.main()
