import copy
import json
import unittest
from typing import Any

from tools import persistent_write_recovery_receipt as recovery
from tools import test_persistent_write_activation_receipt as activation_fixtures
from tools import test_persistent_write_boot_proof as boot_fixtures


class RecoveryReceiptTests(unittest.TestCase):
    def value(self, action: str) -> dict[str, Any]:
        fixture = activation_fixtures.ReceiptTests()
        fixture.setUp()
        value = fixture.value
        for key in ('browser', 'browserSourceHashes', 'browserSubmitted'): value.pop(key, None)
        value.update(action=action, applicationActivations=0, applicationUploads=1 if action == 'pending' else 0,
                     rebootRequested=action == 'boot', sourceHashes={name: 'd' * 64 for name in recovery.PROBES[action]})
        value['before']['status'].update(lastRollback=None, previousSlotId=recovery.common.contract.temporary.SOURCE_SLOT)
        value['after'] = copy.deepcopy(value['before'])
        value['cleaned'].update(floor=8, generation=40)
        if action == 'pending':
            value['pendingSubmissionAttempted'] = True
            value['pending'] = {'result': 'passed', 'fixture': 'held-upload-idle-timeout', 'guardRoot': value['guardRoot'],
                'completeArchiveSent': False, 'restored': value['restored'], 'restoreSubmissionAttempted': True,
                'fencedWhileWorkerLive': True, 'probeSourceRemoved': True, 'terminalPreserved': True,
                'worker': {'unit': 'hardened-blikvm-update-upload@1.service', 'pid': 42, 'invocation': 'f' * 32},
                'workerNaturalExit': {'code': 1, 'status': 50}}
        else:
            value['bootSubmissionAttempted'] = True
            value['after']['bootIdSha256'] = 'f' * 64
            facts = boot_fixtures.BootProofTests().value()
            facts.update(transaction='b' * 24, bootIdSha256='f' * 64)
            value['earlyBoot'] = {'readOnly': True, 'facts': facts,
                'qualification': recovery.boot.validate(facts, prepared_boot='e' * 64, transaction='b' * 24)}
        return value

    def parse(self, value: dict[str, Any]) -> recovery.common.Contents:
        return recovery.parse(json.dumps(value).encode(), action=value['action'], expected_runner='a' * 64,
            expected_probe_sources={name: 'd' * 64 for name in recovery.PROBES[value['action']]})

    def test_recovery_formats_preserve_original_binding(self) -> None:
        for action in recovery.PROBES:
            with self.subTest(action=action):
                result = self.parse(self.value(action))
                self.assertEqual(result.source_binding.transaction_id, 'b' * 24)
                self.assertEqual((result.terminal_floor, result.terminal_generation), (8, 40))

    def test_pending_rejects_kill_missing_fence_and_foreign_worker(self) -> None:
        cases: tuple[tuple[str, object], ...] = (('workerNaturalExit', {'code': 2, 'status': 9}),
            ('fencedWhileWorkerLive', False), ('completeArchiveSent', True),
            ('worker', {'unit': 'ssh.service', 'pid': 42, 'invocation': 'f' * 32}),
            ('restoreSubmissionAttempted', False))
        for key, replacement in cases:
            value = self.value('pending')
            value['pending'][key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError): self.parse(value)

    def test_boot_rejects_late_deadline_overlap_and_changed_state(self) -> None:
        for key, replacement in (('deadlineStartUsec', 1), ('serverStartUsec', 20), ('bootIdSha256', 'e' * 64)):
            value = self.value('boot')
            value['earlyBoot']['facts'][key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError): self.parse(value)
        value = self.value('boot')
        value['after']['status']['generation'] = 41
        with self.assertRaises(ValueError): self.parse(value)

    def test_summaries_and_wrong_producers_are_not_evidence(self) -> None:
        for action in recovery.PROBES:
            for key in ('prepared', 'before', 'after', 'restored', 'cleaned', 'pending' if action == 'pending' else 'earlyBoot'):
                value = self.value(action)
                value[key] = {'result': 'passed'}
                with self.subTest(action=action, key=key), self.assertRaises(ValueError): self.parse(value)
            value = self.value(action)
            value['sourceHashes'] = {}
            with self.assertRaises(ValueError): self.parse(value)


if __name__ == '__main__':
    unittest.main()
