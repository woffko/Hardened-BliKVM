import ast
import base64
import contextlib
import inspect
from pathlib import Path
import stat
import subprocess
import sys
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zlib

from tools import application_update_preflight as probe
from tools import capture_application_update_preflight as capture
from tools.test_retained_resource_baseline import fixture as resources
from tools.test_retained_backend_load import status


class ApplicationPreflightTests(unittest.TestCase):
    def test_bootstrap_imports_all_modules_without_device_execution(self) -> None:
        command, hashes = capture.program({})
        tree = ast.parse(command)
        encoded = next(node.value for node in ast.walk(tree) if isinstance(node, ast.Constant)
                       and isinstance(node.value, str) and len(node.value) > 1000)
        assert isinstance(encoded, str)
        source = zlib.decompress(base64.b64decode(encoded)).decode()
        body = ast.parse(source).body
        boundary = next(index for index, node in enumerate(body) if isinstance(node, ast.Assign)
                        and any(isinstance(target, ast.Name) and target.id == "value" for target in node.targets))
        bootstrap = ast.unparse(ast.Module(body=body[:boundary], type_ignores=[]))
        result = subprocess.run([sys.executable, "-B", "-"], input=bootstrap.encode(), capture_output=True, timeout=5)
        self.assertEqual(result.returncode, 0, result.stderr.decode())
        self.assertEqual(len(hashes), 15)
        self.assertLess(len(command), 100000)

    def test_preflight_accepts_only_unchanged_installed_source(self) -> None:
        for failure in ("none", "target-present", "version", "owner", "hash", "state", "status"):
            with self.subTest(failure=failure), contextlib.ExitStack() as stack:
                update = probe.update
                original_profile = update.active_profile
                stack.callback(setattr, update, "active_profile", original_profile)
                bindings: dict[str, object] = {
                    "new": {"sha256": update.u.PACKAGE_HASHES["new"], "version": update.u.VERSIONS["new"],
                            "files": {str(update.s.bootstrap.SUPERVISOR): [update.u.SUPERVISOR_HASHES["new"], 0o755]}},
                    "old": {"sha256": update.u.PACKAGE_HASHES["old"], "files": {}},
                }
                def checked(argv: list[str]) -> bytes:
                    if argv[0] == "dpkg-query":
                        return b"wrong" if failure == "version" else ("ii :" + update.u.VERSIONS["new"]).encode()
                    self.assertIn(argv[0], ("dpkg", "getcap"))
                    return b""
                def exists(path: Path) -> bool:
                    return failure == "target-present" and path.name.startswith("app-00000000000000000006-")
                source_slot = update.PROFILES["seq5-to-seq6-mjpeg"].source_slot
                def resolve(_path: Path, strict: bool = False) -> Path:
                    return update.u.STATE_ROOTS[0] / source_slot / "bin/bli-kvm-server"
                stack.enter_context(patch.object(probe.os, "geteuid", return_value=0))
                stack.enter_context(patch.object(probe.os, "uname", return_value=SimpleNamespace(machine="aarch64")))
                stack.enter_context(patch.object(Path, "glob", return_value=[]))
                stack.enter_context(patch.object(Path, "resolve", resolve))
                stack.enter_context(patch.object(Path, "lstat", return_value=SimpleNamespace(
                    st_mode=stat.S_IFREG | 0o755, st_uid=1 if failure == "owner" else 0, st_gid=0, st_nlink=1)))
                stack.enter_context(patch.object(update.infrastructure, "exists", side_effect=exists))
                stack.enter_context(patch.object(probe.resources, "snapshot", return_value=resources()[0]))
                stack.enter_context(patch.object(update.s, "snapshot", return_value={"fixture": "unchanged"}))
                stack.enter_context(patch.object(update.s, "property_value", return_value="123"))
                stack.enter_context(patch.object(update.u, "tree_fingerprint", side_effect=[{}, {"changed": True} if failure == "state" else {}]))
                stack.enter_context(patch.object(update.u, "digest", return_value="wrong" if failure == "hash" else update.u.SUPERVISOR_HASHES["new"]))
                stack.enter_context(patch.object(update, "checked", side_effect=checked))
                stack.enter_context(patch.object(update.s.bootstrap, "trusted_code_and_config"))
                stack.enter_context(patch.object(update.infrastructure, "audit_enabled"))
                wiring = stack.enter_context(patch.object(update, "validate_status_wiring"))
                stack.enter_context(patch.object(probe.status_probe, "status_read", side_effect=[status(), {} if failure == "status" else status()]))
                if failure == "none":
                    value = probe.verify(bindings, "synthetic status reader")
                    self.assertEqual(value["result"], "passed")
                    self.assertIs(value["deviceWrites"], False)
                    wiring.assert_called_once_with()
                else:
                    with self.assertRaises(ValueError):
                        probe.verify(bindings, "synthetic status reader")

    def test_verifier_has_no_device_mutation_calls(self) -> None:
        source = inspect.getsource(probe.verify)
        for forbidden in ("systemd-run", "activate_slot", "mkdir", "write_bytes", "unlink", "rename", "replace", "prepare("):
            self.assertNotIn(forbidden, source)


if __name__ == "__main__":
    unittest.main()
