import unittest

from tools import persistent_write_boot_proof as proof


class BootProofTests(unittest.TestCase):
    def value(self) -> dict[str, object]:
        slot = proof.contract.temporary.TARGET_SLOT
        return {'transaction': 'a' * 24, 'bootIdSha256': 'b' * 64, 'decision': 'revoked',
            'phase': 'restored', 'terminal': None, 'guardActiveState': 'active', 'guardSubState': 'exited',
            'guardResult': 'success', 'guardExecCode': 1, 'guardExecStatus': 0,
            'guardStartUsec': 10, 'guardExitUsec': 20, 'serverStartUsec': 30, 'deadlineStartUsec': 0,
            'liveWorkers': 0, 'queuedJobs': 0, 'ownedFenceVerified': True, 'writePolicyAbsent': True,
            'writeEnableLinksAbsent': True, 'guardDependenciesAbsent': True,
            'protectedConfigurationUnchanged': True, 'serverWritesDisabled': True,
            'activeSlotId': slot, 'confirmedSlotId': slot, 'floorSequence': 8, 'generation': 40, 'pending': None}

    def check(self, value: dict[str, object]) -> dict[str, object]:
        return proof.validate(value, prepared_boot='c' * 64, transaction='a' * 24)

    def test_valid_proof_still_requires_runtime_finalization(self) -> None:
        self.assertTrue(self.check(self.value())['runtimeFinalizationStillRequired'])

    def test_every_required_field_is_checked(self) -> None:
        original = self.value()
        for key in original:
            value = dict(original)
            del value[key]
            with self.subTest(key=key), self.assertRaises(ValueError): self.check(value)

    def test_same_boot_other_transaction_deadline_and_finalized_state_fail(self) -> None:
        changes: tuple[tuple[str, object], ...] = (('bootIdSha256', 'c' * 64), ('transaction', 'd' * 24),
            ('deadlineStartUsec', 5), ('terminal', {'slot': 'anything'}), ('decision', 'committed'),
            ('guardExecCode', 2), ('guardExecStatus', 9), ('liveWorkers', 1), ('queuedJobs', 1))
        for key, value in changes:
            with self.subTest(key=key), self.assertRaises(ValueError): self.check({**self.value(), key: value})

    def test_monotonic_order_and_strict_types(self) -> None:
        for key in ('guardStartUsec', 'guardExitUsec', 'serverStartUsec', 'deadlineStartUsec',
                    'guardExecCode', 'guardExecStatus', 'liveWorkers', 'queuedJobs', 'floorSequence', 'generation'):
            for value in (True, '10', -1):
                with self.subTest(key=key, value=value), self.assertRaises(ValueError): self.check({**self.value(), key: value})
        for value in (0, 10, 20):
            with self.assertRaises(ValueError): self.check({**self.value(), 'serverStartUsec': value})


if __name__ == '__main__':
    unittest.main()
