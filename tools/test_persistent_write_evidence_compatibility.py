from dataclasses import replace
import unittest

from tools import persistent_write_evidence_compatibility as evidence


class CompatibilityTests(unittest.TestCase):
    def setUp(self) -> None:
        self.current = evidence.contract.EvidenceBinding('a' * 24, 'b' * 64, evidence.contract.CONFIG_SHA256,
            'app-00000000000000000009-' + 'c' * 64, 9, 43)
        self.components = {name: 'd' * 64 for names in evidence.COMPONENTS.values() for name in names}

    def source(self, gate: str) -> evidence.Source:
        historical = gate in evidence.HISTORICAL_GATES
        binding = replace(self.current, transaction_id='e' * 24) if historical else self.current
        if gate in {'native_activation', 'gui_happy_path'}:
            binding = replace(binding, baseline_slot='app-00000000000000000008-' + 'f' * 64,
                              baseline_floor=8, baseline_generation=40)
        return evidence.Source(gate, binding, self.current.baseline_slot, 9, 43, 'f' * 64, dict(self.components))

    def test_gate_partition_is_complete_and_preserves_original_binding(self) -> None:
        self.assertEqual(evidence.FRESH_GATES | evidence.HISTORICAL_GATES, evidence.contract.REQUIRED_GATES)
        for gate in evidence.COMPONENTS:
            source = self.source(gate)
            value = evidence.check(source, self.current, self.components)
            self.assertIs(value.original, source)
            self.assertEqual(value.historical, gate in evidence.HISTORICAL_GATES)
            self.assertFalse(hasattr(value, 'passed'))

    def test_every_required_component_is_checked(self) -> None:
        for gate, names in evidence.COMPONENTS.items():
            source = self.source(gate)
            for name in names:
                changed = dict(self.components)
                changed[name] = '0' * 64
                with self.subTest(gate=gate, name=name), self.assertRaises(ValueError):
                    evidence.check(source, self.current, changed)
                del changed[name]
                with self.assertRaises(ValueError): evidence.check(source, self.current, changed)

    def test_foreign_transaction_cannot_be_fresh_or_historical_relabelled(self) -> None:
        for gate in evidence.COMPONENTS:
            source = self.source(gate)
            binding = self.current if gate in evidence.HISTORICAL_GATES else replace(self.current, transaction_id='e' * 24)
            with self.subTest(gate=gate), self.assertRaises(ValueError):
                evidence.check(replace(source, binding=binding), self.current, self.components)

    def test_other_terminal_or_device_and_already_active_activation_fail(self) -> None:
        source = self.source('native_activation')
        bad = (replace(source, terminal_generation=44), replace(source, terminal_floor=True),
               replace(source, binding=replace(source.binding, device_fingerprint='0' * 64)),
               replace(source, binding=replace(self.current, transaction_id='e' * 24)))
        for value in bad:
            with self.assertRaises(ValueError): evidence.check(value, self.current, self.components)

    def test_recovery_on_previous_baseline_cannot_be_reused(self) -> None:
        for gate in ('boot_restore', 'disable_drain'):
            source = self.source(gate)
            old = replace(source.binding, baseline_slot='app-00000000000000000008-' + 'f' * 64,
                          baseline_floor=8, baseline_generation=40)
            with self.assertRaises(ValueError): evidence.check(replace(source, binding=old), self.current, self.components)

    def test_component_input_is_snapshotted_not_borrowed(self) -> None:
        source = replace(self.source('host'), components=self.components)
        original = dict(source.components)
        self.components['controller'] = '0' * 64
        self.assertEqual(dict(source.components), original)


if __name__ == '__main__':
    unittest.main()
