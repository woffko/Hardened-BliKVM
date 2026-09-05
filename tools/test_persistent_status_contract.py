from pathlib import Path
import unittest

from tools import persistent_status_contract as contract


class PersistentStatusContractTests(unittest.TestCase):
    def test_every_uncommitted_phase_restores_but_never_starts_server_in_boot(self) -> None:
        for phase in (contract.Phase.PREPARED, contract.Phase.WIRED, contract.Phase.RESTORED):
            for boot in (False, True):
                with self.subTest(phase=phase, boot=boot):
                    result = contract.recovery_decision(phase, boot=boot)
                    self.assertTrue(result.remove_dropins)
                    self.assertEqual(result.restart_server, not boot)
                    self.assertEqual(result.stop_socket, not boot)
                    self.assertEqual(result.verify_runtime_now, not boot)
                    self.assertIs(result.phase, contract.Phase.RESTORED)

    def test_committed_state_survives_interrupted_disarm(self) -> None:
        for boot in (False, True):
            result = contract.recovery_decision(contract.Phase.COMMITTED, boot=boot)
            self.assertFalse(result.remove_dropins)
            self.assertFalse(result.restart_server)
            self.assertFalse(result.stop_socket)
            self.assertIs(result.phase, contract.Phase.COMMITTED)

    def test_commit_requires_both_acceptance_proofs(self) -> None:
        for phase in contract.Phase:
            for native in (False, True):
                for browser in (False, True):
                    if phase in (contract.Phase.WIRED, contract.Phase.COMMITTED) and native and browser:
                        self.assertIs(contract.require_commit(phase, native_verified=native, browser_verified=browser), contract.Phase.COMMITTED)
                    else:
                        with self.assertRaises(ValueError):
                            contract.require_commit(phase, native_verified=native, browser_verified=browser)

    def test_guard_has_explicit_early_boot_and_shutdown_ordering(self) -> None:
        root = Path("/var/lib") / (contract.PREFIX + "a" * 24)
        units = contract.guard_units(root)
        boot = units[root.name + "-boot.service"]
        self.assertIn("DefaultDependencies=no\nAfter=local-fs.target", boot)
        self.assertIn("Before=sysinit.target " + contract.SOCKET, boot)
        self.assertIn("Conflicts=shutdown.target", boot)
        self.assertIn("--boot", boot)
        self.assertNotIn("After=basic.target", boot)
        self.assertNotIn("systemctl", boot)
        self.assertNotIn("--boot", units[root.name + "-restore.service"])
        for invalid in (Path("/"), Path("/var/lib"), Path("/tmp") / root.name, Path("/var/lib/a;command")):
            with self.assertRaises(ValueError):
                contract.guard_units(invalid)


if __name__ == "__main__":
    unittest.main()
