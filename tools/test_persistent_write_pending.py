import unittest

from tools import persistent_write_pending as pending


class FakeOperations:
    def __init__(self) -> None:
        self.calls: list[str] = []
        self.failure = ''
        self.exit: tuple[int, int] = (1, 50)
        self.running = True

    def step(self, name: str) -> None:
        self.calls.append(name)
        if name == self.failure:
            raise OSError('injected failure')

    def hold_upload(self) -> pending.Worker:
        self.step('hold')
        return pending.Worker('hardened-blikvm-update-upload@1.service', 'a' * 32, 42)

    def verify_live_spool(self, worker: pending.Worker) -> None:
        worker.validate()
        self.step('live')

    def submit_restore(self) -> None: self.step('restore')
    def await_closed_owned_fence(self) -> None: self.step('fence')
    def restore_still_running(self) -> bool:
        self.step('running')
        return self.running
    def release_to_idle(self) -> None: self.step('release')
    def await_probe_idle_timeout(self) -> None: self.step('idle')
    def natural_worker_exit(self, worker: pending.Worker) -> tuple[int, int]:
        worker.validate()
        self.step('exit')
        return self.exit
    def await_restore(self) -> dict[str, object]:
        self.step('restored')
        return {'result': 'restored', 'runtimeVerified': True, 'boot': False, 'retentionApproved': False}
    def verify_preserved_terminal(self) -> None: self.step('preserved')
    def close_probe_client(self) -> None: self.step('close-client')


class PendingTests(unittest.TestCase):
    def test_closed_live_window_precedes_release_and_natural_exit(self) -> None:
        ops = FakeOperations()
        receipt: dict[str, object] = {}
        saved: list[dict[str, object]] = []
        result = pending.exercise(ops, receipt, lambda: saved.append(dict(receipt)))
        self.assertEqual(result['result'], 'passed')
        self.assertEqual(ops.calls, ['hold', 'live', 'restore', 'fence', 'live', 'running',
                                    'release', 'idle', 'exit', 'restored', 'preserved', 'close-client'])
        self.assertTrue(saved[1]['restoreSubmissionAttempted'])

    def test_ambiguous_restore_is_not_retried(self) -> None:
        ops = FakeOperations()
        ops.failure = 'restore'
        receipt: dict[str, object] = {}
        with self.assertRaises(OSError): pending.exercise(ops, receipt, lambda: None)
        self.assertEqual(ops.calls.count('restore'), 1)
        self.assertTrue(receipt['restoreSubmissionAttempted'])
        self.assertEqual(ops.calls[-1], 'close-client')
        with self.assertRaises(ValueError): pending.exercise(ops, receipt, lambda: None)
        self.assertEqual(ops.calls.count('restore'), 1)

    def test_durable_reservation_failure_prevents_submission(self) -> None:
        ops = FakeOperations()
        receipt: dict[str, object] = {}
        def save() -> None:
            if receipt.get('restoreSubmissionAttempted'): raise OSError('disk failure')
        with self.assertRaises(OSError): pending.exercise(ops, receipt, save)
        self.assertNotIn('restore', ops.calls)
        self.assertEqual(ops.calls[-1], 'close-client')

    def test_signal_success_or_early_restore_cannot_pass(self) -> None:
        for status in ((2, 9), (2, 15), (1, 0)):
            ops = FakeOperations()
            ops.exit = status
            with self.subTest(status=status), self.assertRaises(ValueError):
                pending.exercise(ops, {}, lambda: None)
            self.assertNotIn('preserved', ops.calls)
        ops = FakeOperations()
        ops.running = False
        with self.assertRaises(ValueError): pending.exercise(ops, {}, lambda: None)
        self.assertNotIn('release', ops.calls)

    def test_each_observation_failure_closes_only_client(self) -> None:
        for phase in ('hold', 'live', 'fence', 'release', 'idle', 'exit', 'restored', 'preserved', 'close-client'):
            ops = FakeOperations()
            ops.failure = phase
            with self.subTest(phase=phase), self.assertRaises(OSError):
                pending.exercise(ops, {}, lambda: None)
            self.assertLessEqual(ops.calls.count('restore'), 1)
            self.assertEqual(ops.calls[-1], 'close-client')


if __name__ == '__main__':
    unittest.main()
