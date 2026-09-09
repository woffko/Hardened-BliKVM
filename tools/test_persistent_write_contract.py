from pathlib import Path
from dataclasses import replace
import unittest

from tools import persistent_write_contract as contract


class PersistentWriteContractTests(unittest.TestCase):
    def test_commit_requires_every_exact_gate_and_explicit_retention_approval(self) -> None:
        binding = contract.EvidenceBinding("a" * 24, "b" * 64, contract.CONFIG_SHA256,
            "app-00000000000000000008-" + "c" * 64, 8, 40)
        gates = tuple(contract.GateEvidence(name, binding, "d" * 64, True) for name in sorted(contract.REQUIRED_GATES))
        for phase in (contract.Phase.WIRED, contract.Phase.COMMITTED):
            self.assertEqual(contract.require_commit(phase, gates, current=binding,
                             explicit_retention_approved=True), contract.Phase.COMMITTED)
        for index in range(len(gates)):
            with self.assertRaises(ValueError):
                contract.require_commit(contract.Phase.WIRED, gates[:index] + (replace(gates[index], passed=False),) + gates[index + 1:],
                    current=binding, explicit_retention_approved=True)
        for actual in ((), gates + (gates[0],), (gates[0],) * len(gates)):
            with self.assertRaises(ValueError):
                contract.require_commit(contract.Phase.WIRED, actual,
                    current=binding, explicit_retention_approved=True)
        with self.assertRaises(ValueError):
            contract.require_commit(contract.Phase.WIRED, gates, current=binding,
                                    explicit_retention_approved=False)
        with self.assertRaises(ValueError):
            contract.require_commit(contract.Phase.WIRED, gates, current=replace(binding, configuration_sha256="foreign"),
                                    explicit_retention_approved=True)
        for changed in (replace(binding, transaction_id="e" * 24), replace(binding, baseline_generation=41),
                        replace(binding, device_fingerprint="f" * 64)):
            with self.assertRaises(ValueError):
                contract.require_commit(contract.Phase.WIRED, gates, current=changed, explicit_retention_approved=True)

    def test_committed_policy_survives_late_deadline_or_boot_recovery(self) -> None:
        for boot in (False, True):
            for phase in (contract.Phase.COMMITTED, contract.Phase.RESTORED):
                value = contract.recovery_decision(phase, boot=boot)
                self.assertEqual(value, contract.RecoveryDecision(False, False, False, False, phase))

    def test_uncommitted_boot_recovery_never_waits_for_downstream_server(self) -> None:
        for phase in (contract.Phase.PREPARED, contract.Phase.WIRED):
            self.assertEqual(contract.recovery_decision(phase, boot=True),
                             contract.RecoveryDecision(True, False, True, False, contract.Phase.RESTORED))
            self.assertEqual(contract.recovery_decision(phase, boot=False),
                             contract.RecoveryDecision(True, True, True, True, contract.Phase.RESTORED))

    def test_socket_enablement_does_not_attach_reopen_dependencies_to_servers(self) -> None:
        data = contract.DROPIN_BYTES.decode()
        self.assertNotIn("Wants=", data)
        self.assertNotIn("Requires=", data)
        self.assertIn("After=" + " ".join(contract.SOCKETS), data)
        self.assertEqual(data.count("Environment="), 3)
        self.assertEqual(len(contract.dropins()), 2)
        self.assertEqual(set(contract.enable_links().values()), {Path("/lib/systemd/system") / name for name in contract.SOCKETS})

    def test_boot_guard_orders_before_both_sockets_and_both_servers(self) -> None:
        root = Path("/var/lib") / (contract.PREFIX + "a" * 24)
        units = contract.guard_units(root)
        boot = units[root.name + "-boot.service"].decode()
        before = next(line for line in boot.splitlines() if line.startswith("Before=")).split("=", 1)[1].split()
        self.assertTrue(set((*contract.SOCKETS, *contract.SERVERS, "sockets.target", "sysinit.target")) <= set(before))
        self.assertIn(" --boot\n", boot)
        self.assertIn(b"OnUnitInactiveSec=30", units[root.name + "-deadline.timer"])
        with self.assertRaises(ValueError):
            contract.guard_units(Path("/tmp/foreign"))

    def test_runtime_disable_stops_on_each_failure_without_killing_workers(self) -> None:
        names = ("close", "settle", "remove", "restart", "verify")
        for failure in (None, *names):
            events: list[str] = []
            def step(name: str) -> None:
                events.append(name)
                if name == failure:
                    raise ValueError("injected")
            class Actions:
                def close_admission(self) -> None: step("close")
                def await_terminal_pipeline(self) -> None: step("settle")
                def remove_owned_configuration(self) -> None: step("remove")
                def reload_and_restart_confirmed(self) -> None: step("restart")
                def verify_closed_and_preserved(self) -> None: step("verify")
            if failure is None:
                contract.disable_runtime(Actions())
                self.assertEqual(events, list(names))
            else:
                with self.assertRaises(ValueError):
                    contract.disable_runtime(Actions())
                self.assertEqual(events, list(names[:names.index(failure) + 1]))

    def test_transaction_guard_failure_blocks_both_servers_and_write_sockets(self) -> None:
        root = Path("/var/lib") / (contract.PREFIX + "a" * 24)
        values = contract.transaction_dropins(root)
        self.assertEqual(len(values), 4)
        for unit in (*contract.SERVERS, *contract.SOCKETS):
            path = Path("/etc/systemd/system") / (unit + ".d") / contract.TRANSACTION_DROPIN_NAME
            self.assertEqual(values[path], (f"[Unit]\nRequires={root.name}-boot.service\n"
                                           f"After={root.name}-boot.service\n").encode())
        self.assertIn(b"RemainAfterExit=yes", contract.guard_units(root)[root.name + "-boot.service"])

    def test_boot_disable_has_no_downstream_wait_or_restart_step(self) -> None:
        events: list[str] = []
        class Actions:
            def cancel_socket_jobs_nonblocking(self) -> None: events.append("cancel")
            def remove_owned_configuration(self) -> None: events.append("remove")
            def reload_manager(self) -> None: events.append("reload")
            def verify_owned_configuration_absent(self) -> None: events.append("verify-files")
        contract.disable_boot(Actions())
        self.assertEqual(events, ["cancel", "remove", "reload", "cancel", "verify-files"])


if __name__ == "__main__":
    unittest.main()
