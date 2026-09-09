import io
import copy
import hashlib
import json
import os
from pathlib import Path
from types import SimpleNamespace
import tempfile
import stat
import tarfile
from typing import cast
import unittest
from unittest.mock import patch

from tools import installed_upload_gate as gate

UNIT = "hardened-blikvm-update-upload@1-123.service"


class InstalledUploadGateTests(unittest.TestCase):
    def test_manual_restore_cleans_only_after_verification_using_saved_profile(self) -> None:
        profile = gate.probe.PROFILES["seq8-publish"]
        for valid in (True, False):
            with self.subTest(valid=valid), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                plan: dict[str, object] = {"profile": profile.name, "newSlotPublished": True}
                events: list[str] = []
                def restore(_root: Path, _plan: dict[str, object]) -> None:
                    events.append("restore")
                    if not valid:
                        raise ValueError("changed state")
                def cleanup(path: Path, selected: gate.probe.UploadProfile) -> None:
                    self.assertEqual(path, root)
                    self.assertEqual(selected, profile)
                    events.append("cleanup")
                with patch.object(gate, "load", return_value=plan), patch.object(gate, "restore", side_effect=restore), \
                     patch.object(gate, "cleanup", side_effect=cleanup):
                    if valid:
                        result = gate.restore_and_cleanup(root)
                        self.assertIs(result["guardMayRemain"], False)
                        self.assertIs(result["newSlotPublished"], True)
                    else:
                        with self.assertRaises(ValueError):
                            gate.restore_and_cleanup(root)
                self.assertEqual(events, ["restore", *(["cleanup"] if valid else [])])

    def test_restore_after_lost_reply_retains_candidate_and_records_observation(self) -> None:
        profile = gate.probe.PROFILES["seq8-publish"]
        for valid in (True, False):
            with self.subTest(valid=valid), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                candidate = root / "slots" / profile.slot
                candidate.mkdir(parents=True)
                payload = candidate / "keep"
                payload.write_bytes(b"published candidate must never be deleted")
                before: dict[str, object] = {"state": {"/old": {"inode": 1}}, "serving": {"pid": 1}}
                after: dict[str, object] = {**before, "state": {"/old": {"inode": 1}, str(candidate): {"inode": 2}}}
                plan: dict[str, object] = {"before": before, "profile": profile.name, "publicationAttempted": True,
                                           "newSlotPublished": False, "phase": "running", "spoolExisted": False}
                events: list[str] = []
                def checked(argv: list[str]) -> bytes:
                    events.append(" ".join(argv))
                    return b""
                def verify(*_args: object) -> None:
                    events.append("verify-candidate")
                    if not valid:
                        raise ValueError("candidate verification failed")
                def save(_root: Path, value: dict[str, object]) -> None:
                    events.append("save-restored")
                    self.assertIs(value["newSlotPublished"], True)
                    self.assertEqual(value["phase"], "restored")
                with patch.object(gate.u, "STATE_ROOTS", (root / "slots", root / "journal")), \
                     patch.object(gate, "UNITS", root / "units"), patch.object(gate, "SPOOL", root / "absent"), \
                     patch.object(gate, "checked", side_effect=checked), \
                     patch.object(gate, "drain", side_effect=lambda: events.append("drain")), \
                     patch.object(gate, "snapshot", return_value=after), \
                     patch.object(gate, "verify_publication", side_effect=verify), patch.object(gate, "save", side_effect=save):
                    if valid:
                        gate.restore(root, plan)
                    else:
                        with self.assertRaises(ValueError):
                            gate.restore(root, plan)
                        self.assertEqual(plan["phase"], "running")
                self.assertEqual(payload.read_bytes(), b"published candidate must never be deleted")
                self.assertEqual(events, ["systemctl stop " + gate.SOCKET, "drain", "verify-candidate",
                                         *(["save-restored", "systemctl daemon-reload"] if valid else [])])

    def test_publication_preflight_requires_absent_target_and_exact_inventory(self) -> None:
        profile = gate.probe.PROFILES["seq8-publish"]
        root = gate.u.STATE_ROOTS[0]
        state: dict[str, object] = {str(root / f"app-{sequence:020d}-{'a' * 64}"): {} for sequence in range(1, 8)}
        state[str(root / "current")] = {}
        gate.publication_preflight({"state": state}, profile)
        for name in (profile.slot, ".staging-foreign", "foreign"):
            with self.subTest(name=name), self.assertRaises(ValueError):
                gate.publication_preflight({"state": {**state, str(root / name): {}}}, profile)
        state.pop(str(root / "current"))
        with self.assertRaises(ValueError):
            gate.publication_preflight({"state": state}, profile)

    def test_publication_delta_preserves_every_original_and_pins_new_tree(self) -> None:
        profile = gate.probe.PROFILES["seq8-publish"]
        candidate = str(gate.u.STATE_ROOTS[0] / profile.slot)
        before: dict[str, object] = {"state": {"/old": {"inode": 1}}, "serving": {"pid": 1}}
        def plan() -> dict[str, object]:
            return {"before": copy.deepcopy(before), "profile": profile.name, "publicationAttempted": True}
        after: dict[str, object] = {"state": {"/old": {"inode": 1}, candidate: {"inode": 2}}, "serving": {"pid": 1}}
        with patch.object(gate, "verify_publication") as verify:
            value = plan()
            self.assertFalse(gate.preserved(value, before, Path("/archive")))
            self.assertTrue(gate.preserved(value, after, Path("/archive")))
            self.assertTrue(value["newSlotPublished"])
            verify.assert_called_once()
            self.assertTrue(gate.preserved(value, after, Path("/archive")))
            for change in (before, {**after, "serving": {"pid": 2}},
                           {**after, "state": {candidate: {"inode": 2}}},
                           {**after, "state": {"/old": {"inode": 3}, candidate: {"inode": 2}}},
                           {**after, "state": {"/old": {"inode": 1}, candidate: {"inode": 3}}},
                           {**after, "state": {"/old": {"inode": 1}, candidate + "-foreign": {}}}):
                with self.assertRaises(ValueError):
                    gate.preserved(value, change, Path("/archive"))
            value = plan()
            value["publicationAttempted"] = False
            with self.assertRaises(ValueError):
                gate.preserved(value, after, Path("/archive"))
            verify.side_effect = ValueError("invalid partial candidate")
            with self.assertRaises(ValueError):
                gate.preserved(plan(), after, Path("/archive"))

    def test_complete_publication_checks_payload_proof_metadata_and_extra_entries(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "application.tar"
            proof = {"manifest.json": b"public manifest", "manifest.sig": b"s" * 64}
            with tarfile.open(archive, "w") as output:
                for name, data in {**proof, "payload/bin/server": b"payload"}.items():
                    member = tarfile.TarInfo(name)
                    member.mode = 0o755 if name.startswith("payload/") else 0o600
                    member.size = len(data)
                    output.addfile(member, io.BytesIO(data))
            profile = gate.probe.UploadProfile("fixture", hashlib.sha256(archive.read_bytes()).hexdigest(), "a" * 64, 8, True)
            slot = root / profile.slot
            (slot / "bin").mkdir(parents=True)
            (slot / "bin/server").write_bytes(b"payload")
            (slot / "bin/server").chmod(0o555)
            (slot / ".hardened-blikvm").mkdir()
            for name, data in proof.items():
                (slot / ".hardened-blikvm" / name).write_bytes(data)
            (slot / ".hardened-blikvm/receipt.json").write_text(json.dumps({"schemaVersion": 1, "slotId": profile.slot,
                 "manifestSha256": profile.manifest_sha, "payloadBytes": 8683648}))
            added = gate.u.tree_fingerprint((slot,))
            for key, raw in added.items():
                entry = cast(dict[str, object], raw)
                path = Path(key)
                mode = 0o500 if path.name == ".hardened-blikvm" else 0o400 if path.parent.name == ".hardened-blikvm" else 0o555
                entry["mode"] = (stat.S_IFDIR if path.is_dir() else stat.S_IFREG) | mode
            with patch.object(gate.u, "STATE_ROOTS", (root, root / "journal")), \
                 patch.object(gate.probe, "BUNDLE_BYTES", archive.stat().st_size), \
                 patch.object(gate.pwd, "getpwnam", return_value=SimpleNamespace(pw_uid=os.getuid())), \
                 patch.object(gate.grp, "getgrnam", return_value=SimpleNamespace(gr_gid=os.getgid())):
                gate.verify_publication(archive, profile, added)
                changes: tuple[dict[str, object], ...] = ({**added, str(slot / "extra"): {}}, {**added, str(slot): {"mode": stat.S_IFLNK | 0o555}})
                for change in changes:
                    with self.assertRaises(ValueError):
                        gate.verify_publication(archive, profile, change)
                (slot / "bin/server").chmod(0o755)
                (slot / "bin/server").write_bytes(b"changed")
                (slot / "bin/server").chmod(0o555)
                with self.assertRaises(ValueError):
                    gate.verify_publication(archive, profile, added)

    def test_template_audit_uses_cat_and_rejects_additional_fragments(self) -> None:
        template = Path("/lib/systemd/system/hardened-blikvm-update-upload@.service")
        body = b"[Service]\nType=simple\n"
        def property_value(_unit: str, field: str) -> str:
            return str(Path("/lib/systemd/system") / gate.SOCKET) if field == "FragmentPath" else ""
        def resolve(path: Path, **_kwargs: object) -> Path:
            return path
        with patch.object(gate.s, "property_value", side_effect=property_value), patch.object(Path, "resolve", resolve), \
             patch.object(Path, "read_bytes", return_value=body), patch.object(gate, "checked", return_value=b"# " + str(template).encode() + b"\n" + body) as checked:
            gate.audit_upload_units()
            checked.assert_called_once_with(["systemctl", "cat", "--no-pager", template.name], timeout=5)
            checked.return_value += b"\n# /etc/foreign.conf\n[Service]\nUser=root\n"
            with self.assertRaises(ValueError):
                gate.audit_upload_units()

    def test_early_rejection_consumes_output_before_worker_lookup_without_wait(self) -> None:
        events: list[str] = []
        child = SimpleNamespace(stdin=io.BytesIO(), stdout=io.BytesIO(), returncode=0, poll=lambda: 0)
        def communicate(**_kwargs: object) -> tuple[bytes, None]:
            events.append("communicate")
            return json.dumps({"mode": "unauthorized", "result": "passed"}).encode(), None
        child.communicate = communicate
        def workers() -> list[str]:
            events.append("workers")
            return [UNIT] if "communicate" in events else []
        with patch.object(gate, "workers", side_effect=workers), patch.object(gate.subprocess, "Popen", return_value=child), \
             patch.object(gate, "natural_exit", return_value={"naturalExit": True}), patch.object(gate, "empty_spool"), \
             patch.object(gate, "checked"), patch.object(gate, "UploadUnitReference") as reference:
            gate.run_vector(Path("/source"), "unauthorized")
            reference.assert_not_called()
        self.assertEqual(events, ["workers", "communicate", "workers"])

    def test_live_worker_requires_real_privilege_and_anonymous_spool_evidence(self) -> None:
        for failure in ("none", "uid", "capabilities", "spool-mode", "named-spool", "journal-write"):
            value = {"MainPID": "123", "InvocationID": "a" * 32, "User": "hardened-blikvm-update",
                     "Group": "hardened-blikvm", "NoNewPrivileges": "yes", "ProtectSystem": "strict",
                     "ReadWritePaths": str(gate.u.STATE_ROOTS[0]) + " " + str(gate.SPOOL),
                     "FragmentPath": "/lib/systemd/system/hardened-blikvm-update-upload@.service",
                     "DropInPaths": "", "CollectMode": "inactive"}
            if failure == "journal-write":
                value["ReadWritePaths"] += " " + str(gate.u.STATE_ROOTS[1])
            def read_text(path: Path) -> str:
                if path.name == "stat":
                    return "123 (worker) " + " ".join(["S"] + ["0"] * 18 + ["99"])
                return ("Uid:\t" + " ".join(["0" if failure == "uid" else "984"] * 4)
                        + "\nGid:\t989 989 989 989\nCapEff:\t"
                        + ("0000000000000001" if failure == "capabilities" else "0000000000000000") + "\nNoNewPrivs:\t1\n")
            def metadata(_path: Path) -> SimpleNamespace:
                return SimpleNamespace(st_mode=stat.S_IFREG | (0o644 if failure == "spool-mode" else 0o600),
                                       st_uid=984, st_gid=989, st_nlink=0, st_dev=2, st_size=1)
            with self.subTest(failure=failure), patch.object(gate, "properties", return_value=value), \
                 patch.object(gate.s, "property_value", return_value="123"), \
                 patch.object(Path, "read_text", read_text), patch.object(Path, "resolve", return_value=gate.s.bootstrap.SUPERVISOR), \
                 patch.object(gate.u, "digest", return_value=gate.u.SUPERVISOR_HASHES["new"]), \
                 patch.object(gate.pwd, "getpwnam", return_value=SimpleNamespace(pw_uid=984)), \
                 patch.object(gate.grp, "getgrnam", return_value=SimpleNamespace(gr_gid=989)), \
                 patch.object(Path, "iterdir", return_value=iter([Path("/proc/123/fd/9")])), \
                 patch.object(gate.os, "readlink", return_value=str(gate.SPOOL) + ("/named" if failure == "named-spool" else "/#123 (deleted)")), \
                 patch.object(Path, "stat", metadata):
                if failure == "none":
                    self.assertTrue(gate.live_worker(UNIT)["anonymousSpool"])
                else:
                    with self.assertRaises(ValueError):
                        gate.live_worker(UNIT)

    def test_emergency_stop_never_waits_for_main_lock(self) -> None:
        root = Path("/var/lib") / (gate.PREFIX + "a" * 24)
        with patch.object(gate, "load"), patch.object(gate, "checked") as checked, \
             patch.object(gate, "drain") as drain, patch.object(gate.fcntl, "flock", side_effect=AssertionError("must not lock")):
            gate.emergency_stop(root)
            checked.assert_called_once_with(["systemctl", "stop", gate.SOCKET])
            drain.assert_called_once_with()
        service = gate.guard_units(root)[root.name + "-restore.service"]
        self.assertIn(b" emergency-stop --guard-root ", service)
        self.assertNotIn(b"controller.py restore", service)

    def test_restore_handles_missing_timer_after_partial_preparation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            plan: dict[str, object] = {"before": {"retained": True}, "spoolExisted": False}
            with patch.object(gate, "UNITS", root), patch.object(gate, "SPOOL", root / "absent"), \
                 patch.object(gate, "drain"), patch.object(gate, "snapshot", return_value=plan["before"]), \
                 patch.object(gate, "save"), patch.object(gate, "checked") as checked:
                gate.restore(root, plan)
                self.assertEqual(plan["phase"], "restored")
                self.assertEqual([call.args[0] for call in checked.call_args_list],
                                 [["systemctl", "stop", gate.SOCKET], ["systemctl", "daemon-reload"]])

    def test_cleanup_accepts_owned_partial_files_but_preserves_unknown_files(self) -> None:
        original = Path.lstat
        def metadata(path: Path) -> SimpleNamespace:
            value = original(path)
            return SimpleNamespace(st_mode=value.st_mode, st_uid=0, st_nlink=value.st_nlink)
        for foreign in (False, True):
            with tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary) / "guard"
                root.mkdir()
                (root / "controller.py").write_bytes(b"partial preparation")
                if foreign:
                    (root / "foreign").write_bytes(b"preserve")
                with patch.object(gate, "source_root", return_value=Path(temporary) / "absent"), patch.object(Path, "lstat", metadata):
                    if foreign:
                        with self.assertRaises(ValueError):
                            gate.cleanup(root)
                        self.assertTrue((root / "foreign").exists())
                        self.assertTrue((root / "controller.py").exists())
                    else:
                        gate.cleanup(root)
                        self.assertFalse(root.exists())

    def test_natural_exit_requires_expected_code_and_invocation(self) -> None:
        for mode, expected in (("retry", 0), ("short", 50), ("malformed", 50), ("unauthorized", 96), ("lost-reply", 50)):
            value = {"ActiveState": "inactive" if expected == 0 else "failed", "Result": "success" if expected == 0 else "exit-code",
                     "ExecMainCode": "1", "ExecMainStatus": str(expected), "InvocationID": "a" * 32, "CollectMode": "inactive",
                     "ExecMainStartTimestampMonotonic": "100", "ExecMainExitTimestampMonotonic": "200"}
            with patch.object(gate, "properties", return_value=value), patch.object(gate, "checked") as checked:
                self.assertTrue(gate.natural_exit(UNIT, mode, "a" * 32)["naturalExit"])
                with self.assertRaises(ValueError):
                    gate.natural_exit(UNIT, mode, "b" * 32)
                for key, invalid in (("ExecMainCode", "2"), ("Result", "timeout"), ("ExecMainStatus", "9"), ("ExecMainExitTimestampMonotonic", "0")):
                    saved = value[key]
                    value[key] = invalid
                    with self.assertRaises(ValueError):
                        gate.natural_exit(UNIT, mode, "a" * 32)
                    value[key] = saved
                checked.assert_not_called()

    def test_vector_records_terminal_before_reset_and_releases_reference(self) -> None:
        for fail in (False, True):
            events: list[str] = []
            child = SimpleNamespace(stdin=io.BytesIO(), stdout=io.BytesIO(), returncode=0, poll=lambda: 0)
            def communicate(**_kwargs: object) -> tuple[bytes, None]:
                events.append("release-body")
                return json.dumps({"mode": "retry", "result": "passed"}).encode(), None
            child.communicate = communicate
            def terminal(*_args: object) -> dict[str, object]:
                events.append("terminal")
                if fail:
                    raise ValueError("synthetic worker crash")
                return {"naturalExit": True}
            def reference(_unit: str) -> SimpleNamespace:
                events.append("pin")
                return SimpleNamespace(close=lambda: events.append("unpin"))
            def reset(_argv: list[str]) -> bytes:
                events.append("reset")
                return b""
            with patch.object(gate, "workers", side_effect=[[], [UNIT]]), patch.object(gate.subprocess, "Popen", return_value=child), \
                 patch.object(gate, "held_handshake"), patch.object(gate, "UploadUnitReference", side_effect=reference), \
                 patch.object(gate, "wait_live", return_value={"invocationId": "a" * 32}), \
                 patch.object(gate, "natural_exit", side_effect=terminal), patch.object(gate, "empty_spool"), \
                 patch.object(gate, "checked", side_effect=reset), \
                 patch.object(gate, "drain", side_effect=AssertionError("must not terminate worker")):
                if fail:
                    with self.assertRaises(ValueError):
                        gate.run_vector(Path("/source"), "retry")
                else:
                    gate.run_vector(Path("/source"), "retry")
            self.assertEqual(events, ["pin", "release-body", "terminal", *([] if fail else ["reset"]), "unpin"])

    def test_worker_properties_are_whitelisted_and_not_duplicated(self) -> None:
        for raw in (b"MainPID=1\nMainPID=1\n", b"MainPID=1\nEnvironment=private\n", b"x" * 16385):
            with patch.object(gate, "checked", return_value=raw), self.assertRaises(ValueError):
                gate.properties(UNIT, ("MainPID",))


if __name__ == "__main__":
    unittest.main()
