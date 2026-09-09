from pathlib import Path
import unittest

from tools import application_write_contract as contract


def status(floor: int = 7) -> dict[str, object]:
    slot = contract.SOURCE_SLOT if floor == 7 else contract.TARGET_SLOT
    return {"floorSequence": floor, "generation": 37 if floor == 7 else 40,
            "pending": None, "activeSlotId": slot, "confirmedSlotId": slot,
            "previousSlotId": contract.SOURCE_SLOT if floor == 8 else "old-anchor", "lastRollback": None}


class WriteContractTests(unittest.TestCase):
    def test_confirmation_not_receipt_and_observed_floor_never_regresses(self) -> None:
        self.assertEqual(contract.terminal_state(status()).floor, 7)
        self.assertEqual(contract.terminal_state(status(8), require_target=True).floor, 8)
        invalid: tuple[dict[str, object], ...] = ({"phase": "awaiting_health"}, {**status(), "pending": {}},
                      {**status(), "floorSequence": True}, {**status(), "generation": True},
                      {**status(), "generation": 36}, {**status(), "activeSlotId": contract.TARGET_SLOT})
        for value in invalid:
            with self.assertRaises(ValueError):
                contract.terminal_state(value)
        with self.assertRaises(ValueError):
            contract.terminal_state(status(), observed_floor=8)
        for value in (status(), {**status(8), "generation": 37}, {**status(8), "lastRollback": "health_timeout"},
                      {**status(8), "previousSlotId": "foreign"}):
            with self.assertRaises(ValueError):
                contract.terminal_state(value, require_target=True)

    def test_no_restart_before_workers_jobs_and_apply_receipts_settle(self) -> None:
        contract.require_pipeline_idle(activation_workers=0, queued_jobs=0, candidate_active=False,
                                       health_token_present=False, confirmed_apply_present=False)
        for workers, jobs, candidate, health, apply in ((1, 0, False, False, False), (0, 1, False, False, False),
                (0, 0, True, False, False), (0, 0, False, True, False), (0, 0, False, False, True)):
            with self.assertRaises(ValueError):
                contract.require_pipeline_idle(activation_workers=workers, queued_jobs=jobs, candidate_active=candidate,
                                                health_token_present=health, confirmed_apply_present=apply)

    def test_every_failed_restoration_step_prevents_later_mutations(self) -> None:
        names = ["close", "drain", "settle", "remove", "restart", "verify"]
        for failed in (None, *names):
            events: list[str] = []
            def event(name: str) -> None:
                events.append(name)
                if name == failed:
                    raise ValueError("injected failure")
            class Actions:
                def close_admission(self) -> None: event("close")
                def drain_naturally(self) -> None: event("drain")
                def await_terminal_pipeline(self) -> contract.TerminalState:
                    event("settle")
                    return contract.terminal_state(status(8))
                def remove_owned_overrides(self) -> None: event("remove")
                def reload_and_restart_confirmed(self) -> None: event("restart")
                def verify_closed(self, terminal: contract.TerminalState) -> None:
                    self_check = terminal.floor == 8
                    if not self_check:
                        raise AssertionError("terminal state lost")
                    event("verify")
            if failed is None:
                self.assertEqual(contract.restore_runtime(Actions()).floor, 8)
                self.assertEqual(events, names)
            else:
                with self.assertRaises(ValueError):
                    contract.restore_runtime(Actions())
                self.assertEqual(events, names[:names.index(failed) + 1])

    def test_temporary_environment_cannot_reopen_sockets_from_server_dependency(self) -> None:
        data = contract.WRITE_DROPIN_BYTES.decode()
        self.assertEqual(data.count("Environment="), 3)
        self.assertNotIn("Wants=", data)
        self.assertNotIn("Requires=", data)
        self.assertNotIn("[Install]", data)
        root = Path("/var/lib") / (contract.PREFIX + "a" * 24)
        units = contract.guard_units(root)
        self.assertIn(b" deadline --guard-root ", units[root.name + "-restore.service"])
        self.assertIn(b"OnActiveSec=600", units[root.name + "-deadline.timer"])
        self.assertIn(b"OnUnitInactiveSec=30", units[root.name + "-deadline.timer"])
        self.assertIn(b"StartLimitIntervalSec=0", units[root.name + "-restore.service"])
        with self.assertRaises(ValueError):
            contract.guard_units(Path("/tmp/foreign"))


if __name__ == "__main__":
    unittest.main()
