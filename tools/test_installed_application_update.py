import inspect
import contextlib
from dataclasses import replace
import hashlib
import io
import json
from pathlib import Path
import tempfile
import tarfile
import stat
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from tools import installed_application_update as gate
from tools import stage_installed_application_update as driver
from tools import status_api_wiring as status_wiring


class InstalledApplicationUpdateTests(unittest.TestCase):
    def test_application_profile_pins_updater_and_restores_legacy_bindings(self) -> None:
        original = gate.active_profile
        fixture = replace(original, name="fixture-dev9", updater_profile="dev7-dev9")
        try:
            with patch.dict(gate.PROFILES, {fixture.name: fixture}):
                gate.select_profile(fixture.name)
                self.assertEqual(gate.u.selected_profile, "dev7-dev9")
                self.assertEqual(gate.u.VERSIONS["new"], "0.1.0~dev9")
                invalid = replace(fixture, name="fixture-invalid", updater_profile="unknown")
                with patch.dict(gate.PROFILES, {invalid.name: invalid}):
                    with self.assertRaises(ValueError):
                        gate.select_profile(invalid.name)
                self.assertIs(gate.active_profile, fixture)
                self.assertEqual(gate.u.selected_profile, "dev7-dev9")
                gate.select_profile("seq5-to-seq6-mjpeg")
                self.assertEqual(gate.u.VERSIONS["new"], "0.1.0~dev7")
                self.assertEqual(gate.u.PACKAGE_HASHES, gate.u.LEGACY_BINDINGS[0])
        finally:
            gate.select_profile(original.name)

    def test_driver_selects_matching_package_sources_before_upload(self) -> None:
        original = gate.active_profile
        original_package_profile = driver.packages.native.selected_profile
        fixture = replace(original, name="fixture-dev9", updater_profile="dev7-dev9")
        try:
            with tempfile.TemporaryDirectory() as temporary, patch.dict(gate.PROFILES, {fixture.name: fixture}):
                argv = ["driver", "prepare", "--profile", fixture.name, "--endpoint", "unused:22",
                        "--output", str(Path(temporary) / "receipt.json"), "--old-package", "old.deb",
                        "--new-package", "new.deb", "--bundle", "application.tar"]
                def stop_after_binding(*args: str) -> dict[str, bytes]:
                    self.assertEqual(args, ("old.deb", "new.deb"))
                    self.assertEqual(driver.packages.native.VERSIONS["new"], "0.1.0~dev9")
                    self.assertEqual(driver.packages.SOURCE_BINDINGS["old"],
                                     driver.packages.LEGACY_SOURCE_BINDINGS["new"])
                    raise RuntimeError("fixture stops before any upload")
                with patch.object(driver.sys, "argv", argv), patch.object(
                        driver.transport, "validate_output_path", return_value=Path(temporary) / "receipt.json"), patch.object(
                        driver.packages, "upload_payloads", side_effect=stop_after_binding), patch.object(
                        driver.transport, "read_secret_stdin") as secret:
                    with self.assertRaisesRegex(RuntimeError, "fixture stops"):
                        driver.main()
                    secret.assert_not_called()
        finally:
            driver.packages.select_profile(original_package_profile)
            gate.select_profile(original.name)

    def test_mjpeg_profile_binds_verified_sequence_six_and_keeps_floor(self) -> None:
        profile = gate.PROFILES["seq5-to-seq6-mjpeg"]
        self.assertEqual(profile.source_slot, gate.PROFILES["seq4-to-seq5-notification"].target_slot)
        self.assertEqual((profile.source_sequence, profile.target_sequence), (5, 6))
        self.assertEqual(profile.bundle_bytes, 8601600)
        self.assertEqual(profile.bundle_sha, "4f5a307e2d1014de360dc2cdb75cb5186a60511cfd772f3be0b98b8aff9a6036")
        self.assertEqual(profile.target_slot, "app-00000000000000000006-8eedeb5ce21ca08b90336cad27bdd7f16c6502388a1f13bc943a2026d80534ac")
        self.assertTrue(profile.persistent_status)
        self.assertIn("status_api_wiring.py", gate.HELPERS)
        with patch.object(gate, "active_profile", profile):
            self.assertEqual(gate.minimum_recovery_floor({"phase": "confirmed"}), 6)
            self.assertEqual(gate.minimum_recovery_floor({"phase": "recovered", "observedFloor": 6}), 6)
            root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
            self.assertIn("--profile seq5-to-seq6-mjpeg", gate.guard_units(root)[root.name + "-recovery.service"])

    def test_persistent_status_preservation_accepts_only_exact_rehearsal_override(self) -> None:
        for failure in ("none", "rehearsal", "foreign", "mode", "link", "content", "missing", "runtime", "runtime-extra", "runtime-content", "runtime-current", "socket"):
            with self.subTest(failure=failure), contextlib.ExitStack() as stack:
                runtime_present = failure in ("rehearsal", "runtime", "runtime-extra", "runtime-content")
                def exists(path: Path) -> bool:
                    if failure == "runtime-current":
                        return path == Path("/run/systemd/system") / (gate.s.CURRENT_SERVER + ".d")
                    return runtime_present and path == gate.DROPIN_ROOT
                def metadata(path: Path) -> SimpleNamespace:
                    if failure == "missing" and path.name.endswith(".d"):
                        raise FileNotFoundError("synthetic missing persistent override")
                    if path == status_wiring.ENDPOINT:
                        return SimpleNamespace(st_mode=stat.S_IFSOCK | 0o620, st_uid=0 if failure == "socket" else 984, st_gid=989)
                    if path.name.endswith(".d"):
                        return SimpleNamespace(st_mode=stat.S_IFDIR | 0o755, st_uid=0)
                    return SimpleNamespace(st_mode=stat.S_IFREG | (0o666 if failure == "mode" else 0o644),
                                           st_uid=0, st_nlink=2 if failure == "link" else 1)
                def entries(path: Path) -> list[Path]:
                    result = [gate.DROPIN] if path == gate.DROPIN_ROOT else [path / status_wiring.DROPIN_NAME]
                    if failure == "foreign" or (failure == "runtime-extra" and path == gate.DROPIN_ROOT):
                        result.append(path / "foreign.conf")
                    return result
                def contents(path: Path) -> bytes:
                    if path == gate.DROPIN:
                        return b"changed" if failure == "runtime-content" else gate.DROPIN_BYTES
                    return b"changed" if failure == "content" else status_wiring.DROPIN_BYTES
                def digest(path: Path) -> str:
                    return status_wiring.UNIT_HASHES[path.name]
                stack.enter_context(patch.object(gate.infrastructure, "exists", side_effect=exists))
                stack.enter_context(patch.object(Path, "lstat", metadata))
                stack.enter_context(patch.object(Path, "iterdir", entries))
                stack.enter_context(patch.object(Path, "read_bytes", contents))
                environment = stack.enter_context(patch.object(status_wiring, "runtime_environment"))
                stack.enter_context(patch.object(gate.u, "digest", side_effect=digest))
                stack.enter_context(patch.object(gate.s, "enabled", return_value="disabled"))
                stack.enter_context(patch.object(gate.s, "property_value", return_value="active"))
                stack.enter_context(patch.object(gate.pwd, "getpwnam", return_value=SimpleNamespace(pw_uid=984)))
                stack.enter_context(patch.object(gate.grp, "getgrnam", return_value=SimpleNamespace(gr_gid=989)))
                if failure in ("none", "rehearsal"):
                    gate.validate_status_wiring(allow_rehearsal=failure == "rehearsal")
                    environment.assert_called_once_with(True)
                else:
                    with self.assertRaises((ValueError, FileNotFoundError)):
                        gate.validate_status_wiring(allow_rehearsal=failure in ("runtime-extra", "runtime-content", "runtime-current"))

    def test_rehearsal_phase_is_durable_before_creating_override(self) -> None:
        source = inspect.getsource(gate.prepare)
        staged = source.index('plan["phase"] = "staged"')
        persisted = source.index("save(root, plan)", staged)
        override = source.index("DROPIN_ROOT.mkdir", staged)
        self.assertLess(staged, persisted)
        self.assertLess(persisted, override)

    def test_notification_profile_is_bound_to_verified_sequence_five(self) -> None:
        profile = gate.PROFILES["seq4-to-seq5-notification"]
        self.assertEqual(profile.source_slot, gate.PROFILES["seq2-to-seq4-status"].target_slot)
        self.assertEqual((profile.source_sequence, profile.target_sequence), (4, 5))
        self.assertEqual(profile.bundle_bytes, 8591360)
        self.assertEqual(profile.bundle_sha, "618b1068086a04aa75f44804b58bcf0dac05d62c11553af74ebfe5cfdf893093")
        with patch.object(gate, "active_profile", profile):
            self.assertEqual(gate.minimum_recovery_floor({"phase": "confirmed"}), 5)
            self.assertEqual(gate.minimum_recovery_floor({"phase": "recovered", "observedFloor": 5}), 5)

    def test_named_profile_preserves_legacy_and_binds_new_recovery(self) -> None:
        original = gate.active_profile
        try:
            gate.select_profile("seq2-to-seq4-status")
            self.assertEqual(gate.profile_slots(), {2: gate.SLOT_TWO, 4: gate.active_profile.target_slot})
            self.assertEqual(gate.active_profile.bundle_bytes, 8591360)
            root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
            for suffix in ("-boot.service", "-recovery.service"):
                self.assertIn("--profile seq2-to-seq4-status", gate.guard_units(root)[root.name + suffix])
            with self.assertRaises(ValueError):
                gate.select_profile("unreviewed")
            self.assertEqual(gate.active_profile.name, "seq2-to-seq4-status")
            gate.select_profile("seq1-to-seq2")
            self.assertEqual(gate.profile_slots(), gate.SLOTS)
        finally:
            gate.select_profile(original.name)

    def test_staged_payload_matches_new_archive_not_previous_application(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "application.tar"
            payload = b"new server bytes"
            with tarfile.open(archive, "w") as output:
                member = tarfile.TarInfo("payload/bin/bli-kvm-server")
                member.size = len(payload)
                member.mode = 0o755
                output.addfile(member, io.BytesIO(payload))
            slot = root / "slot"
            (slot / "bin").mkdir(parents=True)
            target = slot / "bin/bli-kvm-server"
            target.write_bytes(payload)
            target.chmod(0o555)
            profile = gate.UpdateProfile("fixture", hashlib.sha256(archive.read_bytes()).hexdigest(),
                                         archive.stat().st_size, 2, "source", 4, "target")
            with patch.object(gate, "active_profile", profile):
                gate.verify_staged_payload(archive, slot)
                target.chmod(0o755)
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)
                target.write_bytes(b"wrong server bytes")
                target.chmod(0o555)
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)
                target.chmod(0o755)
                target.write_bytes(payload)
                target.chmod(0o555)
                (slot / "bin/extra").write_bytes(b"extra")
                with self.assertRaises(ValueError):
                    gate.verify_staged_payload(archive, slot)

    def test_recovery_rejects_floor_regression_but_accepts_confirmation_before_marker(self) -> None:
        original = gate.active_profile
        try:
            gate.select_profile("seq2-to-seq4-status")
            root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
            for boot in (False, True):
                for phase, observed, floor, accepted in (
                    ("confirmed", 2, 2, False), ("confirmed", 2, 4, True),
                    ("rehearsed", 2, 4, True), ("recovered", 4, 2, False),
                ):
                    slot = gate.profile_slots()[floor]
                    status = {"floorSequence": floor, "pending": None, "activeSlotId": slot, "confirmedSlotId": slot}
                    plan: dict[str, object] = {"phase": phase, "observedFloor": observed}
                    with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
                         patch.object(gate.infrastructure, "current_status", return_value=status), \
                         patch.object(gate, "await_stable", return_value={"status": status}), \
                         patch.object(gate, "save") as save, patch.object(gate, "disarm") as disarm, \
                         patch.object(gate, "checked", return_value=b""):
                        if accepted:
                            gate.recover(root, plan, boot)
                            self.assertEqual(plan["observedFloor"], 4)
                        else:
                            with self.assertRaises(ValueError):
                                gate.recover(root, plan, boot)
                            save.assert_not_called()
                            disarm.assert_not_called()
        finally:
            gate.select_profile(original.name)

    def test_commit_selects_browser_phase_from_bound_checkpoint(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "receipt.json"
            base = {"probe": "installed-application-update", "result": "passed",
                    "bundleSha256": gate.active_profile.bundle_sha, "profile": gate.active_profile.name,
                    "guardRoot": "/var/lib/" + gate.PREFIX + "a" * 24}
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                for phase, expected in (("rehearsed", "rehearsal"), ("confirmed", "confirmed")):
                    path.write_text(json.dumps({**base, "device": {"phase": phase}}))
                    self.assertEqual(driver.commit_browser_phase(str(path)), expected)
                path.write_text(json.dumps({**base, "profile": "other", "device": {"phase": "confirmed"}}))
                with self.assertRaises(ValueError):
                    driver.commit_browser_phase(str(path))

    def test_device_commit_rejects_stale_browser_phase(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            with patch.object(gate, "load", return_value={"phase": "confirmed"}), \
                 patch.object(gate, "stable", return_value={"status": {"floorSequence": 2}}), \
                 patch.object(gate, "save") as save, patch.object(gate, "disarm") as disarm:
                with self.assertRaises(ValueError):
                    gate.transition("commit", root, False, "rehearsal")
                save.assert_not_called()
                disarm.assert_not_called()
                gate.transition("commit", root, False, "confirmed")
                self.assertEqual(save.call_args.args[1]["phase"], "committed-update")

    def test_browser_acceptance_is_bound_to_guard_and_transaction_phase(self) -> None:
        root = "/var/lib/" + gate.PREFIX + "b" * 24
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "browser.json"
            receipt: dict[str, object] = {"probe": "hardened-blikvm-existing-account-browser", "result": "passed",
                                          "runTag": Path(root).name + "-rehearsal", "controlRequests": 0,
                                          "webSockets": 0, "secretEmitted": False}
            receipt.update(dict.fromkeys(("videoDecoded", "streamRevoked", "secureSessionCookie", "secureCsrfCookie", "sessionCleared", "legacyBrandingAbsent"), True))
            receipt.update(dict.fromkeys(("loginStatus", "inventoryStatus", "diagnosticsStatus", "videoStatus", "logoutStatus"), 200))
            path.write_text(json.dumps(receipt))
            with patch.object(driver.transport, "validate_private_file", return_value=path):
                driver.browser_gate(str(path), root, "rehearsal")
                with self.assertRaises(ValueError):
                    driver.browser_gate(str(path), root, "confirmed")
                with self.assertRaises(ValueError):
                    driver.browser_gate(str(path), root.replace("b" * 24, "c" * 24), "rehearsal")

    def test_boot_handoff_orders_after_watchdog_without_waiting_for_gui(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        unit = gate.guard_units(root)[root.name + "-boot.service"]
        self.assertIn("After=basic.target " + gate.infrastructure.WATCHDOG, unit)
        self.assertIn("Before=" + gate.s.CURRENT_SERVER + " " + gate.s.CANDIDATE_SERVER, unit)
        self.assertIn("WantedBy=multi-user.target", unit)
        self.assertNotIn("sysinit.target", unit)
        status = {"floorSequence": 2, "pending": None, "activeSlotId": gate.SLOT_TWO, "confirmedSlotId": gate.SLOT_TWO}
        with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
             patch.object(gate.infrastructure, "current_status", return_value=status), \
             patch.object(gate, "await_stable") as wait, patch.object(gate, "save"), patch.object(gate, "disarm"):
            result = gate.recover(root, {"phase": "confirmed"}, True)
            wait.assert_not_called()
            self.assertFalse(result["journalRestoredFromBackup"])

    def test_recovery_after_confirmation_preserves_floor_two(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        verified = {"status": {"floorSequence": 2, "pending": None}}
        with patch.object(gate, "remove_override"), patch.object(gate, "remove_source"), \
             patch.object(gate.infrastructure, "current_status", return_value=verified["status"]), \
             patch.object(gate, "await_stable", return_value=verified), patch.object(gate, "save"), \
             patch.object(gate, "disarm"), patch.object(gate, "checked", return_value=b"") as checked:
            result = gate.recover(root, {"phase": "confirmed"}, False)
            self.assertEqual(result["verification"], verified)
            self.assertFalse(result["journalRestoredFromBackup"])
            self.assertEqual([call.args[0] for call in checked.call_args_list], [
                ["systemctl", "start", gate.infrastructure.WATCHDOG],
                ["systemctl", "start", gate.s.CURRENT_SERVER],
            ])

    def test_changed_health_override_is_not_removed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "override.conf"
            path.write_text("unrelated override")
            with patch.object(gate, "DROPIN", path), patch.object(gate, "checked", return_value=b""), self.assertRaises(ValueError):
                gate.remove_override()
            self.assertEqual(path.read_text(), "unrelated override")

    def test_no_journal_or_pointer_rewrite_in_recovery_controller(self) -> None:
        source = inspect.getsource(gate)
        self.assertNotIn("application-state.json", source)
        self.assertNotIn("os.symlink(", source)
        self.assertNotIn("shutil.rmtree", source)
        for forbidden in ('"reboot"', '"poweroff"', '"reset"', '"confirm-stdin"'):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
