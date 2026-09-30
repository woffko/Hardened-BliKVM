from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

import tools.build_debian_runtime_package as package
import tools.stage_device_prototype as prototype


class DebianRuntimePackageTests(unittest.TestCase):
    def test_development_package_revision_is_dev2(self) -> None:
        self.assertEqual(package.PACKAGE_VERSION, "0.1.0~dev2")

    def test_file_map_is_read_only_runtime_only(self) -> None:
        destinations = set(package.FILE_MAP.values())
        self.assertEqual(len(destinations), len(package.FILE_MAP))
        for required in (
            "usr/libexec/hardened-blikvm/bli-kvm-server",
            "usr/libexec/hardened-blikvm/bli-kvm-platform-broker",
            "usr/libexec/hardened-blikvm/bli-kvm-video-proxy",
            "usr/libexec/hardened-blikvm/bli-kvm-control-broker",
            "lib/systemd/system/hardened-blikvm-server.service",
            "lib/systemd/system/hardened-blikvm-nginx.service",
            "lib/systemd/system/hardened-blikvm-board-owner.service.d/90-socket-group.conf",
            "usr/share/doc/hardened-blikvm-runtime/examples/hardened-blikvm-video-tmpfiles.conf",
            "usr/share/doc/hardened-blikvm-runtime/examples/hardened-blikvm-sysusers.conf",
            "usr/share/doc/hardened-blikvm-runtime/examples/hardened-blikvm-nginx.conf",
        ):
            self.assertIn(required, destinations)
        rendered = "\n".join(sorted(package.FILE_MAP)) + "\n" + "\n".join(sorted(destinations))
        for forbidden in (
            "update-supervisor",
            "update-health-reporter",
            "update-watchdog",
            "diagnostic-collector",
            "diagnostic-persistence",
            "diagnostic-store",
            "system-action",
            "etc/nginx",
            "etc/hardened-blikvm",
            "usr/lib/sysusers.d",
        ):
            self.assertNotIn(forbidden, rendered)

    def test_manifest_is_inert_and_binds_source_bundle(self) -> None:
        bundle = prototype.BundleInfo(
            Path("/bundle.tar.gz"),
            "a" * 64,
            1024,
            "b" * 40,
        )
        manifest = package.package_manifest(bundle, "c" * 40)
        self.assertEqual(manifest["sourceCommit"], "b" * 40)
        self.assertEqual(manifest["sourceBundleSha256"], "a" * 64)
        self.assertEqual(manifest["builderCommit"], "c" * 40)
        self.assertEqual(manifest["scope"], "inert-read-only-runtime")
        for field in (
            "autoStart",
            "unitTemplatesActive",
            "accountsProvisioned",
            "credentialsIncluded",
            "nginxConfigActive",
            "updateRuntimeIncluded",
            "diagnosticPersistenceIncluded",
            "hardwareMutationIncluded",
        ):
            self.assertIs(manifest[field], False)

    def test_builder_requires_clean_git_commit(self) -> None:
        clean = subprocess.CompletedProcess(["git", "status"], 0, b"", b"")
        revision = subprocess.CompletedProcess(
            ["git", "rev-parse"], 0, b"c" * 40 + b"\n", b""
        )
        with mock.patch.object(package.subprocess, "run", side_effect=[clean, revision]):
            self.assertEqual(package.clean_builder_commit(), "c" * 40)

        dirty = subprocess.CompletedProcess(
            ["git", "status"], 0, b" M tracked-file\n", b""
        )
        with mock.patch.object(package.subprocess, "run", return_value=dirty):
            with self.assertRaises(package.BuildError):
                package.clean_builder_commit()

    def test_output_directory_must_be_new_absolute_and_empty(self) -> None:
        with self.assertRaises(ValueError):
            package.validate_output_dir("relative")
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            empty = root / "empty"
            empty.mkdir()
            self.assertEqual(package.validate_output_dir(str(empty)), empty)
            occupied = root / "occupied"
            occupied.mkdir()
            (occupied / "file").write_text("occupied")
            with self.assertRaises(ValueError):
                package.validate_output_dir(str(occupied))

    def test_legacy_name_is_allowed_only_in_non_web_documentation(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            docs = root / "usr/share/doc/hardened-blikvm-runtime"
            web = root / "usr/share/hardened-blikvm/web"
            docs.mkdir(parents=True)
            web.mkdir(parents=True)
            (docs / "history.md").write_bytes(b"Nano" + b"KVM migration history")
            (web / "index.html").write_text("Hardened BliKVM")
            package.audit_package_tree(root)
            (web / "index.html").write_bytes(b"Nano" + b"KVM")
            with self.assertRaises(package.BuildError):
                package.audit_package_tree(root)

    def test_only_the_exact_rfc1918_ten_network_literal_is_allowed(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            web = root / "usr/share/hardened-blikvm/web"
            web.mkdir(parents=True)
            (web / "index.html").write_text("Hardened BliKVM")
            binary = root / "usr/libexec/hardened-blikvm/bli-kvm-control-broker"
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"{ " + package.RFC1918_TEN_NETWORK + b" }")
            package.audit_package_tree(root)
            for leaked in (b"10." + b"0.1.2", b"10." + b"0.0.0/16", b"10." + b"0.9.1"):
                binary.write_bytes(b"{ " + package.RFC1918_TEN_NETWORK + b" } " + leaked)
                with self.assertRaises(package.BuildError):
                    package.audit_package_tree(root)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/build_debian_runtime_package.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())


if __name__ == "__main__":
    _ = unittest.main()
