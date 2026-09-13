import copy
from dataclasses import asdict
import hashlib
import json
import unittest
from typing import Any

from tools import persistent_write_activation_receipt as receipt


class ReceiptTests(unittest.TestCase):
    def setUp(self) -> None:
        self.producers = {name: 'd' * 64 for name in receipt.BROWSER_SOURCES}
        bound = receipt.contract.EvidenceBinding('b' * 24, 'c' * 64, receipt.contract.CONFIG_SHA256,
            receipt.contract.temporary.TARGET_SLOT, 8, 40)
        root = '/var/lib/' + receipt.contract.PREFIX + bound.transaction_id
        self.value: dict[str, Any] = {
            'probe': 'persistent-write-wiring-rehearsal', 'action': 'activate', 'result': 'passed',
            'runnerSha256': 'a' * 64, 'browserSourceHashes': self.producers,
            **{k: True for k in ('prepareSubmitted', 'cleanupSubmitted', 'runtimeWritesClosed', 'controllerStagingRemoved', 'browserSubmitted')},
            **{k: False for k in ('guardRetained', 'retentionApproved', 'targetPowerControl', 'rebootRequested')},
            'applicationUploads': 1, 'applicationActivations': 1, 'guardRoot': root,
            'prepared': {'binding': asdict(bound), 'guardRoot': root, 'result': 'prepared', 'phase': 'wired', 'deadlineArmed': True, 'retentionApproved': False},
            'before': {'bootIdSha256': 'e' * 64, 'runtimeWritesClosed': True,
                'status': {'activeSlotId': bound.baseline_slot, 'confirmedSlotId': bound.baseline_slot, 'floorSequence': 8, 'generation': 40, 'pending': None}},
            'after': {'bootIdSha256': 'e' * 64, 'runtimeWritesClosed': True,
                'status': {'activeSlotId': receipt.TARGET_SLOT, 'confirmedSlotId': receipt.TARGET_SLOT, 'floorSequence': 9,
                           'generation': 43, 'pending': None, 'lastRollback': None, 'previousSlotId': bound.baseline_slot}},
            'restored': {'result': 'restored', 'guardRoot': root, 'runtimeVerified': True, 'boot': False, 'retentionApproved': False},
            'cleaned': {'result': 'cleaned', 'guardRoot': root, 'archiveRoot': '/var/lib/hardened-blikvm-write-restored-' + bound.transaction_id,
                        'runtimeWritesClosed': True, 'floor': 9, 'generation': 43},
            'browser': {'probe': 'application-http-write', 'mode': 'activate', 'profile': 'seq8-to9', 'result': 'passed',
                'selectedArchiveVerified': True, 'uploadRequests': 1, 'activationRequests': 1, 'loginRequests': 1,
                'blockedMutations': 0, 'pageErrors': 0, 'ui': {'generationReviewed': '40', 'stagingDisposition': 'published',
                    'uploadedAlreadyPresent': False, 'activationStatus': 202, 'activationReceiptValidated': True}},
        }

    def parse(self, value: dict[str, Any]) -> receipt.Contents:
        return receipt.parse(json.dumps(value).encode(), expected_runner='a' * 64, expected_browser_sources=self.producers)

    def test_valid_preserves_exact_original_bytes_and_transaction(self) -> None:
        value = self.parse(self.value)
        self.assertEqual(value.original_bytes, json.dumps(self.value).encode())
        self.assertEqual(value.receipt_sha256, hashlib.sha256(value.original_bytes).hexdigest())
        self.assertEqual(value.source_binding.baseline_floor, 8)
        self.assertEqual(value.terminal_floor, 9)
        self.assertFalse(hasattr(value, 'retentionApproved'))

    def test_passed_flag_does_not_replace_nested_proofs(self) -> None:
        for key in ('prepared', 'before', 'after', 'restored', 'cleaned', 'browser'):
            value = copy.deepcopy(self.value)
            value[key] = {'result': 'passed'}
            with self.subTest(key=key), self.assertRaises(ValueError): self.parse(value)

    def test_strict_counters_and_forward_confirmation(self) -> None:
        paths: tuple[tuple[tuple[str, ...], object], ...] = (
            (('applicationActivations',), True), (('browser', 'activationRequests'), 2),
            (('browser', 'ui', 'activationStatus'), 202.0), (('after', 'status', 'generation'), 40),
            (('after', 'status', 'previousSlotId'), receipt.TARGET_SLOT),
            (('after', 'status', 'pending'), {'phase': 'awaiting_health'}),
            (('after', 'bootIdSha256'), 'f' * 64), (('cleaned', 'generation'), 42))
        for path, replacement in paths:
            value = copy.deepcopy(self.value)
            target = value
            for key in path[:-1]: target = target[key]
            target[path[-1]] = replacement
            with self.subTest(path=path), self.assertRaises(ValueError): self.parse(value)

    def test_producer_and_duplicate_fields_fail_closed(self) -> None:
        with self.assertRaises(ValueError): self.parse({**self.value, 'runnerSha256': 'f' * 64})
        with self.assertRaises(ValueError): self.parse({**self.value, 'browserSourceHashes': {}})
        for raw in (b'{"a":1,"a":2}', b'{"a":NaN}', b'', b' ' * 1048577):
            with self.assertRaises(ValueError):
                receipt.parse(raw, expected_runner='a' * 64, expected_browser_sources=self.producers)

    def seq9_receipt(self) -> dict[str, Any]:
        value = copy.deepcopy(self.value)
        bound = receipt.contract.EvidenceBinding('b' * 24, 'c' * 64, receipt.contract.CONFIG_SHA256,
            receipt.TARGET_SLOT, 9, 43)
        root = '/var/lib/' + receipt.contract.PREFIX + bound.transaction_id
        value['guardRoot'] = root
        value['prepared']['binding'] = asdict(bound)
        value['prepared']['guardRoot'] = root
        value['before']['status'] = {'activeSlotId': bound.baseline_slot, 'confirmedSlotId': bound.baseline_slot,
            'floorSequence': 9, 'generation': 43, 'pending': None}
        value['after']['status'] = {'activeSlotId': receipt.SEQ10_SLOT, 'confirmedSlotId': receipt.SEQ10_SLOT,
            'floorSequence': 10, 'generation': 46, 'pending': None, 'lastRollback': None,
            'previousSlotId': bound.baseline_slot}
        value['restored']['guardRoot'] = root
        value['cleaned'].update(guardRoot=root,
            archiveRoot='/var/lib/hardened-blikvm-write-restored-' + bound.transaction_id, floor=10, generation=46)
        value['browser']['profile'] = 'seq9-to10'
        value['browser']['ui']['generationReviewed'] = '43'
        return value

    def test_seq9_to_seq10_profile_is_accepted(self) -> None:
        parsed = self.parse(self.seq9_receipt())
        self.assertEqual(parsed.source_binding.baseline_floor, 9)
        self.assertEqual(parsed.terminal_floor, 10)
        self.assertEqual(parsed.terminal_slot, receipt.SEQ10_SLOT)

    def test_unreviewed_profile_and_terminal_are_rejected(self) -> None:
        wrong_profile = self.seq9_receipt()
        wrong_profile['browser']['profile'] = 'seq8-to9'
        with self.assertRaises(ValueError): self.parse(wrong_profile)
        wrong_terminal = self.seq9_receipt()
        wrong_terminal['after']['status']['generation'] = 47
        with self.assertRaises(ValueError): self.parse(wrong_terminal)


if __name__ == '__main__':
    unittest.main()
