from pathlib import Path
from contextlib import ExitStack
from dataclasses import asdict
import stat
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import persistent_write_retained_snapshot as retained


class RetainedSnapshotTests(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path('/var/lib') / (retained.contract.PREFIX + 'a' * 24)

    def properties(self, unit: str, key: str) -> str:
        if key == 'FragmentPath': return str(retained.files.LIB_ROOT / unit)
        if key == 'DropInPaths':
            names = (retained.files.status.DROPIN_NAME, retained.contract.DROPIN_NAME) if unit in retained.contract.SERVERS else ()
            return ' '.join(sorted(str(retained.files.UNIT_ROOT / (unit + '.d') / n) for n in names))
        if key == 'Requires': return 'sysinit.target'
        if key == 'After': return self.root.name + '-boot.service'
        raise AssertionError('unexpected property')

    def test_detached_loaded_contract_has_no_guard_requirement(self) -> None:
        with patch.object(retained.native.s, 'property_value', side_effect=self.properties):
            retained.loaded_units(self.root)

    def test_foreign_fragment_override_and_guard_requirement_fail(self) -> None:
        for field in ('FragmentPath', 'DropInPaths', 'Requires'):
            def value(unit: str, key: str) -> str:
                if key == field:
                    return self.root.name + '-boot.service' if field == 'Requires' else '/foreign'
                return self.properties(unit, key)
            with self.subTest(field=field), patch.object(retained.native.s, 'property_value', side_effect=value), self.assertRaises(ValueError):
                retained.loaded_units(self.root)

    def test_retained_verification_never_accepts_missing_or_revoked_decision(self) -> None:
        terminal = retained.contract.temporary.TerminalState('app-00000000000000000009-' + 'b' * 64, 9, 43)
        for decision in (None, SimpleNamespace(outcome='revoked')):
            with patch.object(retained.native.decisions, 'read', return_value=decision), \
                 patch.object(retained, 'configuration') as config, self.assertRaises(ValueError):
                retained.capture(self.root, {}, terminal)
            config.assert_not_called()

    def test_temporary_override_cannot_be_called_retained(self) -> None:
        path = Path('/etc/systemd/system/unit.d') / retained.contract.TRANSACTION_DROPIN_NAME
        with patch.object(retained.files, 'validate_backup'), patch.object(retained.files, 'files', return_value={path: b'x'}), \
             patch.object(retained.files, 'exists', return_value=True), self.assertRaises(ValueError):
            retained.configuration(self.root, {})

    def test_active_endpoints_and_loaded_environment_are_checked(self) -> None:
        slot = 'app-00000000000000000009-' + 'b' * 64
        binding = retained.contract.EvidenceBinding('a' * 24, 'c' * 64, retained.contract.CONFIG_SHA256, slot, 9, 43)
        winner = retained.native.decisions.Decision(binding, 'committed', {name: 'e' * 64 for name in retained.contract.REQUIRED_GATES})
        terminal = retained.contract.temporary.TerminalState(slot, 9, 43)
        for mode, passes in ((0o620, True), (0o666, False)):
            with self.subTest(mode=mode), ExitStack() as stack:
                stack.enter_context(patch.object(retained.native.decisions, 'read', return_value=winner))
                stack.enter_context(patch.object(retained.native, 'load', return_value={
                    'binding': asdict(binding), 'phase': 'committed', 'cleanupStarted': False}))
                stack.enter_context(patch.object(retained.native.u, 'select_profile'))
                stack.enter_context(patch.object(retained.native.u, 'installed'))
                stack.enter_context(patch.object(retained, 'configuration'))
                stack.enter_context(patch.object(retained, 'loaded_units'))
                environment = stack.enter_context(patch.object(retained.native.snapshot.state, 'environment'))
                stack.enter_context(patch.object(retained.native.s, 'enabled', return_value='enabled'))
                stack.enter_context(patch.object(retained.native.s, 'property_value', return_value='active'))
                stack.enter_context(patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=stat.S_IFSOCK | mode, st_uid=42, st_gid=43)))
                stack.enter_context(patch.object(retained.pwd, 'getpwnam', return_value=SimpleNamespace(pw_uid=42)))
                stack.enter_context(patch.object(retained.grp, 'getgrnam', return_value=SimpleNamespace(gr_gid=43)))
                snapshot = stack.enter_context(patch.object(retained.native.snapshot.state.upload.boot.status_api, 'snapshot', return_value={}))
                stack.enter_context(patch.object(Path, 'readlink', return_value=Path(slot)))
                stack.enter_context(patch.object(retained.native.u, 'digest', return_value='d' * 64))
                if passes:
                    self.assertIn('persistentDropinHashes', retained.capture(self.root, {}, terminal))
                    snapshot.assert_called_once()
                else:
                    with self.assertRaises(ValueError): retained.capture(self.root, {}, terminal)
                    snapshot.assert_not_called()
                environment.assert_called_once_with(True)


if __name__ == '__main__':
    unittest.main()
