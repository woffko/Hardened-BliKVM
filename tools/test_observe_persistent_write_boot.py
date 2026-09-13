import unittest
from pathlib import Path
from unittest.mock import patch

from tools import observe_persistent_write_boot as observer


class FinalObservationTests(unittest.TestCase):
    def test_final_deadline_read_rejects_late_start(self) -> None:
        root = Path('/var/lib/fixture')
        for timestamp in ('1', '450000000', ''):
            with self.subTest(timestamp=timestamp), \
                 patch.object(observer.native, 'load', return_value={'phase': 'restored', 'terminal': None}), \
                 patch.object(observer.native.temporary.upload, 'properties', return_value={'ExecMainStartTimestampMonotonic': timestamp}), \
                 patch.object(observer.native, 'boot_identity', return_value='b' * 64), self.assertRaises(ValueError):
                observer.require_unfinalized(root, 'b' * 64)

    def test_final_read_follows_plan_read_and_checks_boot(self) -> None:
        calls: list[str] = []
        def plan(_: Path) -> dict[str, object]:
            calls.append('plan')
            return {'phase': 'restored', 'terminal': None}
        def execution(_unit: str, _fields: tuple[str, ...]) -> dict[str, str]:
            calls.append('deadline')
            return {'ExecMainStartTimestampMonotonic': '0'}
        def boot() -> str:
            calls.append('boot')
            return 'b' * 64
        with patch.object(observer.native, 'load', side_effect=plan), \
             patch.object(observer.native.temporary.upload, 'properties', side_effect=execution), \
             patch.object(observer.native, 'boot_identity', side_effect=boot):
            observer.require_unfinalized(Path('/var/lib/fixture'), 'b' * 64)
        self.assertEqual(calls, ['plan', 'deadline', 'boot'])

    def test_observation_uses_retained_plan_counters(self) -> None:
        source = Path(observer.__file__).read_text()
        self.assertIn('native.counters(plan)', source)
        self.assertNotIn('Counters(8, 40)', source)


if __name__ == '__main__':
    unittest.main()
