from pathlib import Path
import subprocess
import unittest

from tools import stage_device_update_runtime_upgrade as gate
from tools import test_stage_device_update_trust_config as fixtures


class UpdateRuntimeUpgradeTests(unittest.TestCase):
    def rendered(self) -> tuple[str, str, str]:
        fixture = fixtures.StageDeviceUpdateTrustConfigTests()
        old = gate.package.PackageInfo(Path("/old.deb"), *gate.OLD[:1], 100, *gate.OLD[1:], {})
        new = gate.package.PackageInfo(Path("/new.deb"), *gate.NEW[:1], 100, *gate.NEW[1:], {})
        paths = gate.package.paths(new.sha256, "0123456789abcdef")
        accepted = gate.trust.AcceptedEnrollment("a" * 64, "b" * 64, "c" * 40)
        return gate.scripts(paths, old, new, accepted, fixture.baseline(), fixture.preflight())

    def test_all_generated_transitions_parse_as_posix_shell(self) -> None:
        for script in self.rendered():
            result = subprocess.run(["sh", "-n"], input=script.encode(), capture_output=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_commit_receipt_prevents_late_rollback_from_downgrading(self) -> None:
        rollback, apply, commit = self.rendered()
        self.assertLess(rollback.index('if [ ! -e "$stage_root/committed" ]'), rollback.index('dpkg --force-confold -i "$stage_root/old.deb"'))
        self.assertIn('test "$(cat "$stage_root/committed")" = "$marker_value"', rollback)
        self.assertIn('flock -w 30 -x 9', apply)
        self.assertIn('flock -w 30 -x 9', commit)
        self.assertLess(commit.index('sync "$stage_root/committed.part"'), commit.index('mv "$stage_root/committed.part"'))
        self.assertNotIn('dpkg --purge', rollback)

    def test_absence_checks_reject_dangling_state_symlinks(self) -> None:
        script = gate.no_state()
        self.assertIn('test ! -e "$path" && test ! -L "$path"', script)
        self.assertIn('--state=active,activating,deactivating,reloading', script)
        for transition in self.rendered():
            self.assertIn('= "$config_sha256"', transition)
            self.assertNotIn('rm -f -- "$config_target"', transition)
            self.assertNotIn('systemctl restart', transition)


if __name__ == "__main__":
    unittest.main()
