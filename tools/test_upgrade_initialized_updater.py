from pathlib import Path
import json
import stat
import tempfile
from types import SimpleNamespace
import unittest
from typing import cast
from unittest.mock import patch

from tools import upgrade_initialized_updater as gate
from tools import stage_initialized_updater_upgrade as driver
from tools import status_api_wiring as status_api
from tools import persistent_status_wiring as persistent


class InitializedUpdaterTests(unittest.TestCase):
    def test_upgrade_preserves_retained_status_with_variant_bound_binaries(self) -> None:
        for variant in ("old", "new"):
            with patch.object(persistent, "validate_dropins") as validate, \
                 patch.object(status_api, "snapshot", return_value={}) as snapshot, \
                 patch.object(gate, "digest", return_value="a" * 64), patch.object(gate, "status_transport_inert") as inert:
                value = gate.snapshot(variant)
                validate.assert_called_once_with(True)
                snapshot.assert_called_once_with(True, expected_version=gate.VERSIONS[variant], expected_supervisor_sha=gate.SUPERVISOR_HASHES[variant])
                self.assertEqual(len(cast(dict[str, str], value["persistentDropinHashes"])), 2)
                inert.assert_not_called()

    def test_status_transport_must_remain_disabled_without_active_workers(self) -> None:
        with patch.object(gate.serving, "inactive"), patch.object(gate.serving, "enabled", return_value="disabled"), \
             patch.object(Path, "exists", return_value=False), patch.object(Path, "is_symlink", return_value=False), \
             patch.object(gate, "checked", return_value=b""):
            gate.status_transport_inert()
            with patch.object(gate.serving, "enabled", return_value="enabled"), self.assertRaises(ValueError):
                gate.status_transport_inert()
            with patch.object(gate, "checked", return_value=b"active worker"), self.assertRaises(ValueError):
                gate.status_transport_inert()
            with patch.object(Path, "is_symlink", return_value=True), self.assertRaises(ValueError):
                gate.status_transport_inert()

    def test_operational_helpers_are_unique_and_boot_recovery_precedes_servers(self) -> None:
        self.assertEqual(len(gate.HELPERS), len(set(gate.HELPERS)))
        self.assertIn("enable_update_services.py", gate.HELPERS)
        self.assertIn("upgrade_initialized_updater.py", gate.HELPERS)
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        unit = gate.guard_units(root)[root.name + "-boot.service"]
        self.assertIn("Before=multi-user.target " + gate.serving.CURRENT_SERVER + " " + gate.serving.CANDIDATE_SERVER, unit)

    def test_returned_guard_is_fixed_namespace_only(self) -> None:
        root = "/var/lib/" + gate.PREFIX + "b" * 24
        self.assertEqual(driver.validate_guard(root), root)
        for invalid in (None, "/var/lib", root + "/../other", root + ";command", "/tmp/" + gate.PREFIX + "b" * 24):
            with self.assertRaises(ValueError):
                driver.validate_guard(invalid)

    def test_retention_requires_a_matching_successful_rehearsal(self) -> None:
        with self.assertRaises(ValueError):
            driver.validate_rehearsal(None)
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            receipt = {"probe": "initialized-updater-upgrade", "action": "rehearse", "result": "passed",
                       "packageHashes": gate.PACKAGE_HASHES, "retainedVersion": gate.VERSIONS["old"],
                       "guardRemoved": True, "controllerStagingRemoved": True}
            path.write_text(json.dumps(receipt))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                driver.validate_rehearsal(str(path))
                receipt["guardRemoved"] = False
                path.write_text(json.dumps(receipt))
                with self.assertRaises(ValueError):
                    driver.validate_rehearsal(str(path))

    def test_incomplete_marker_recovers_only_known_transition_prefix(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            plan: dict[str, object] = {"schemaVersion": 1, "phase": "rolled_back", "before": {}}
            partial = root / "plan.json.part"
            partial.write_bytes(json.dumps({**plan, "phase": "applied"}, sort_keys=True).encode()[:20])
            metadata = SimpleNamespace(st_mode=stat.S_IFREG | 0o600, st_uid=0, st_nlink=1, st_size=20)
            with patch.object(Path, "lstat", return_value=metadata):
                gate.save(root, plan)
            self.assertEqual(json.loads((root / "plan.json").read_bytes()), plan)
            partial.write_bytes(b"unknown transition")
            with patch.object(Path, "lstat", return_value=metadata), self.assertRaises(ValueError):
                gate.save(root, plan)
            self.assertEqual(partial.read_bytes(), b"unknown transition")

    def test_tree_snapshot_detects_contents_modes_and_link_changes(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            payload = root / "payload"
            payload.write_bytes(b"first")
            link = root / "current"
            link.symlink_to("payload")
            initial = gate.tree_fingerprint((root,))
            payload.write_bytes(b"other")
            self.assertNotEqual(initial, gate.tree_fingerprint((root,)))
            payload.write_bytes(b"first")
            self.assertEqual(initial, gate.tree_fingerprint((root,)))
            payload.chmod(0o400)
            self.assertNotEqual(initial, gate.tree_fingerprint((root,)))
            link.unlink()
            link.symlink_to("different")
            self.assertEqual(cast(dict[str, object], gate.tree_fingerprint((root,))[str(link)])["target"], "different")

    def test_guard_targets_only_its_fixed_controller_and_no_power(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        rendered = gate.guard_units(root)
        self.assertEqual(len(rendered), 3)
        content = "\n".join(rendered.values())
        self.assertIn("OnActiveSec=600", content)
        self.assertIn(f"{root}/controller.py rollback --guard-root {root}", content)
        for forbidden in ("reboot", "poweroff", "reset", "rm -rf", "sh -c"):
            self.assertNotIn(forbidden, content)

    def test_late_rollback_does_not_downgrade_committed_package(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(gate, "load_plan", return_value={"phase": "committed"}), \
                 patch.object(gate, "install") as install, patch.object(gate, "verify") as verify, \
                 patch.object(gate, "save") as save, patch.object(gate, "disarm"):
                result = gate.transition("rollback", root, False)
                self.assertEqual(result["phase"], "committed")
                install.assert_not_called()
                save.assert_not_called()
                self.assertEqual(verify.call_args.args[2], "new")

    def test_pending_rollback_restores_bound_old_package_before_disarming(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            order: list[tuple[str, object]] = []

            def installed(_root: Path, variant: str) -> None:
                order.append(("install", variant))

            def verified(_root: Path, _plan: dict[str, object], variant: str, _boot: bool) -> None:
                order.append(("verify", variant))

            def saved(_root: Path, plan: dict[str, object]) -> None:
                order.append(("save", plan["phase"]))

            def disarmed(_root: Path, _boot: bool) -> None:
                order.append(("disarm", None))

            with patch.object(gate, "load_plan", return_value={"phase": "applied"}), \
                 patch.object(gate, "install", side_effect=installed), \
                 patch.object(gate, "verify", side_effect=verified), \
                 patch.object(gate, "save", side_effect=saved), \
                 patch.object(gate, "disarm", side_effect=disarmed):
                gate.transition("rollback", root, False)
            self.assertEqual(order, [("install", "old"), ("verify", "old"), ("save", "rolled_back"), ("disarm", None)])

    def test_failed_verification_cannot_commit_or_disarm(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            with patch.object(gate, "load_plan", return_value={"phase": "applied"}), \
                 patch.object(gate, "verify", side_effect=ValueError("state changed")), \
                 patch.object(gate, "save") as save, patch.object(gate, "disarm") as disarm:
                with self.assertRaises(ValueError):
                    gate.transition("commit", Path(temporary), False)
                save.assert_not_called()
                disarm.assert_not_called()


if __name__ == "__main__":
    unittest.main()
