import subprocess
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from tools import installed_pending_write_gate as gate


class AdapterTests(unittest.TestCase):
    def make_ops(self) -> gate.Operations:
        with patch.object(gate.native, 'load', return_value={'phase': 'wired'}):
            return gate.Operations(Path('/var/lib/fixture'), Path('/tmp/fixture'))

    def test_refuses_unwired_policy(self) -> None:
        with patch.object(gate.native, 'load', return_value={'phase': 'restored'}), self.assertRaises(ValueError):
            gate.Operations(Path('/var/lib/fixture'), Path('/tmp/fixture'))

    def test_hold_uses_idle_only_and_pins_worker(self) -> None:
        ops = self.make_ops()
        client = MagicMock(spec=subprocess.Popen)
        client.stdout = MagicMock()
        with patch.object(gate.upload, 'workers', side_effect=[[], ['held.service']]), \
             patch.object(gate.native.temporary.observer, 'live_workers', return_value=0), \
             patch.object(gate.subprocess, 'Popen', return_value=client) as spawn, \
             patch.object(gate.upload, 'held_handshake'), \
             patch.object(gate.upload, 'UploadUnitReference'), \
             patch.object(gate.upload, 'wait_live', return_value={'invocationId': 'a' * 32, 'pid': 42}):
            worker = ops.hold_upload()
        self.assertEqual(worker.pid, 42)
        argv = spawn.call_args.args[0]
        self.assertIn('idle', argv)
        self.assertIn('--hold', argv)
        self.assertNotIn('publish', argv)
        self.assertNotIn('retry', argv)

    def test_live_check_rejects_replaced_worker_or_missing_spool(self) -> None:
        ops = self.make_ops()
        worker = gate.pending.Worker('hardened-blikvm-update-upload@1.service', 'a' * 32, 42)
        good = {'invocationId': worker.invocation, 'pid': 42, 'anonymousSpool': True}
        for bad in ({**good, 'pid': 43}, {**good, 'invocationId': 'b' * 32}, {**good, 'anonymousSpool': False}):
            with patch.object(gate.upload, 'live_worker', return_value=bad), self.assertRaises(ValueError):
                ops.verify_live_spool(worker)

    def test_restore_submitted_only_once(self) -> None:
        ops = self.make_ops()
        with patch.object(gate.subprocess, 'Popen', return_value=MagicMock(spec=subprocess.Popen)) as spawn:
            ops.submit_restore()
            with self.assertRaises(ValueError): ops.submit_restore()
            self.assertEqual(spawn.call_count, 1)
            self.assertEqual(spawn.call_args.args[0][-3:], ['restore', '--guard-root', '/var/lib/fixture'])

    def test_natural_exit_bound_to_original_invocation(self) -> None:
        ops = self.make_ops()
        worker = gate.pending.Worker('hardened-blikvm-update-upload@1.service', 'a' * 32, 42)
        with patch.object(gate.upload, 'natural_exit', return_value={'naturalExit': True, 'exitCode': 50}) as observe:
            self.assertEqual(ops.natural_worker_exit(worker), (1, 50))
            observe.assert_called_once_with(worker.unit, 'idle', worker.invocation)
        with patch.object(gate.upload, 'natural_exit', return_value={'naturalExit': True, 'exitCode': 0}), self.assertRaises(ValueError):
            ops.natural_worker_exit(worker)


if __name__ == '__main__':
    unittest.main()
