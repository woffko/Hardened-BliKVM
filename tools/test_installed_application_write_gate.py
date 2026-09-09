from contextlib import contextmanager
from collections.abc import Callable
from pathlib import Path
import unittest
from unittest.mock import patch

from tools import installed_application_write_gate as gate


class GateTests(unittest.TestCase):
    def test_deadline_closes_before_lock_and_rechecks_under_lock(self) -> None:
        events: list[str] = []
        plan: dict[str, object] = {"phase": "restored"}
        terminal = gate.contract.TerminalState(gate.contract.TARGET_SLOT, 8, 40)
        def revoke(_root: Path) -> None:
            events.append("revoke")
        def installed(_root: Path, _variant: str) -> None:
            events.append("installed")
        def restore(_root: Path, _plan: dict[str, object]) -> gate.contract.TerminalState:
            events.append("restore")
            return terminal
        @contextmanager
        def lock(_root: Path):
            events.append("lock")
            yield
        with patch.object(gate.guard, "load", return_value=plan), \
             patch.object(gate.guard, "mark_deadline", side_effect=revoke), \
             patch.object(gate.observer, "close_admission", side_effect=lambda: events.append("close")), \
             patch.object(gate.guard, "locked", side_effect=lock), \
             patch.object(gate.upload.u, "installed", side_effect=installed), \
             patch.object(gate, "audit_templates"), \
             patch.object(gate, "restore", side_effect=restore):
            result = gate.transition("deadline", Path("/unused"))
        self.assertEqual(events, ["revoke", "close", "lock", "installed", "restore"])
        self.assertTrue(result["runtimeWritesClosed"])

    def test_deadline_marker_failure_still_closes_admission(self) -> None:
        with patch.object(gate.guard, "load", return_value={}), \
             patch.object(gate.guard, "mark_deadline", side_effect=OSError), \
             patch.object(gate.observer, "close_admission") as close, \
             patch.object(gate.guard, "locked") as lock:
            with self.assertRaises(OSError):
                gate.transition("deadline", Path("/unused"))
            close.assert_called_once_with()
            lock.assert_not_called()

    def test_failure_to_close_does_not_begin_removing_overrides(self) -> None:
        with patch.object(gate.observer, "close_admission", side_effect=OSError), \
             patch.object(gate.guard, "save") as save, \
             patch.object(gate.overrides, "remove") as remove:
            with self.assertRaises(OSError):
                gate.contract.restore_runtime(gate.Restore(Path("/unused"), {}))
            save.assert_not_called()
            remove.assert_not_called()

    def test_retained_floor_saved_before_returning_terminal(self) -> None:
        plan: dict[str, object] = {"observedFloor": 7}
        saved: list[dict[str, object]] = []
        terminal = gate.contract.TerminalState(gate.contract.TARGET_SLOT, 8, 40)
        def settle(_read: Callable[[], gate.pipeline.Observation], remember: Callable[[int], None],
                   **_kwargs: object) -> gate.contract.TerminalState:
            remember(8)
            return terminal
        def save(_root: Path, value: dict[str, object]) -> None:
            saved.append(dict(value))
        with patch.object(gate.pipeline, "await_terminal", side_effect=settle), \
             patch.object(gate.upload, "empty_spool"), \
             patch.object(gate.guard, "save", side_effect=save):
            actual = gate.Restore(Path("/unused"), plan).await_terminal_pipeline()
        self.assertEqual(actual, terminal)
        self.assertEqual(saved[0], {"observedFloor": 8})
        self.assertEqual(saved[1]["terminal"], {"slot": terminal.slot, "floor": 8, "generation": 40})

    def test_finalization_requires_target_but_restoration_itself_allows_source(self) -> None:
        from tools.test_application_write_contract import status
        from contextlib import nullcontext
        terminal = gate.contract.TerminalState(gate.contract.SOURCE_SLOT, 7, 37)
        with patch.object(gate.guard, "load", return_value={"phase": "restored"}), \
             patch.object(gate.observer, "close_admission"), \
             patch.object(gate.guard, "locked", return_value=nullcontext()), \
             patch.object(gate.upload.u, "installed"), patch.object(gate, "audit_templates"), \
             patch.object(gate, "restore", return_value=terminal), \
             patch.object(gate.upload.application.infrastructure, "current_status", return_value=status()):
            with self.assertRaises(ValueError):
                gate.transition("finalize", Path("/unused"))


if __name__ == "__main__":
    unittest.main()
