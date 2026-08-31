from pathlib import Path
import subprocess
import unittest

from tools import capture_trusted_floor_topology as capture


def valid_facts() -> dict[str, str]:
    facts = {key: "no" for key in capture.FACT_KEYS}
    facts.update(
        {
            "architecture": "aarch64",
            "rootfs_mode": "rw",
            "kernel_config_available": "yes",
            "sys_class_tpm_count": "0",
            "sys_class_tee_count": "0",
            "mmc_block_count": "1",
            "rpmb_sysfs_count": "0",
            "rpmb_dev_count": "0",
            "dt_optee_node_present": "no",
            "tpm_core_module_present": "no",
            "tpm_tis_core_module_present": "no",
            "optee_module_present": "no",
        }
    )
    for key in capture.CONFIG_KEYS:
        facts[key] = "no"
    for name in capture.NODES:
        facts[f"{name}_present"] = "no"
        facts[f"{name}_type"] = "unavailable"
        facts[f"{name}_mode"] = "unavailable"
    return facts


def encode(facts: dict[str, str]) -> bytes:
    return "".join(f"{key}={facts[key]}\n" for key in sorted(facts)).encode()


class CaptureTrustedFloorTopologyTests(unittest.TestCase):
    def test_parser_accepts_only_complete_sanitized_metadata(self) -> None:
        facts = valid_facts()
        facts.update(
            {
                "config_tcg_tpm": "builtin",
                "config_mmc_block": "builtin",
                "tpm0_present": "yes",
                "tpm0_type": "char",
                "tpm0_mode": "600",
                "sys_class_tpm_count": "1",
            }
        )
        self.assertEqual(capture.parse_facts(encode(facts)), dict(sorted(facts.items())))

    def test_parser_rejects_unknown_duplicate_incomplete_and_contradictory(self) -> None:
        facts = valid_facts()
        cases = [
            encode(facts) + b"unknown=yes\n",
            encode(facts) + b"architecture=aarch64\n",
            encode(facts).replace(b"architecture=aarch64\n", b"", 1),
        ]
        contradictory = facts.copy()
        contradictory["tee0_type"] = "char"
        cases.append(encode(contradictory))
        unsafe = facts.copy()
        unsafe["tpm0_present"] = "yes"
        unsafe["tpm0_type"] = "other"
        unsafe["tpm0_mode"] = "666"
        cases.append(encode(unsafe))
        for raw in cases:
            with self.assertRaises(ValueError):
                capture.parse_facts(raw)

    def test_remote_probe_never_opens_devices_or_reads_identity(self) -> None:
        probe = capture.REMOTE_PROBE
        for forbidden in (
            "dd ",
            "blkid",
            "lsblk",
            "tpm2_",
            "tee-supplicant",
            "mmc extcsd",
            "/cid",
            "/csd",
            "/serial",
            "cat /dev/",
            "< /dev/",
            "> /dev/",
        ):
            self.assertNotIn(forbidden, probe)
        self.assertIn("stat -c %a", probe)
        self.assertIn("/dev/mmcblk0rpmb", probe)
        self.assertIn("CONFIG_TEE", probe)

    def test_policy_records_metadata_only_scope(self) -> None:
        policy = capture.policy_receipt()
        self.assertTrue(policy["fixedPathMetadataRead"])
        self.assertTrue(policy["kernelConfigRead"])
        for key in (
            "filesystemWrites",
            "deviceNodesOpened",
            "deviceContentsRead",
            "mmcIdentityRead",
            "tpmIdentityRead",
            "teeIdentityRead",
            "uniqueIdentifiersRead",
            "keysProvisioned",
            "secureObjectsCreated",
            "servicesChanged",
            "rootfsHelpersInvoked",
            "physicalActionsInvoked",
        ):
            self.assertFalse(policy[key])

    def test_direct_entrypoint_loads_project_package(self) -> None:
        script = Path(capture.__file__).resolve()
        result = subprocess.run(
            [str(script), "--help"],
            cwd="/tmp",
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
