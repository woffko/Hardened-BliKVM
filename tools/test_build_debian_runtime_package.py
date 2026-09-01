from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import tools.build_debian_runtime_package as package
import tools.stage_device_prototype as prototype


class DebianRuntimePackageTests(unittest.TestCase):
    def test_file_map_is_read_only_runtime_only(self) -> None:
        destinations = set(package.FILE_MAP.values())
        self.assertEqual(len(destinations), len(package.FILE_MAP))
        for required in (
            "usr/libexec/hardened-blikvm/bli-kvm-server",
            "usr/libexec/hardened-blikvm/bli-kvm-platform-broker",
            "usr/libexec/hardened-blikvm/bli-kvm-video-proxy",
            "usr/libexec/hardened-blikvm/bli-kvm-control-broker",
            "lib/systemd/system/hardened-blikvm-server.service",
            "usr/lib/sysusers.d/hardened-blikvm.conf",
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
        ):
            self.assertNotIn(forbidden, rendered)

    def test_manifest_is_inert_and_binds_source_bundle(self) -> None:
        bundle = prototype.BundleInfo(
            Path("/bundle.tar.gz"),
            "a" * 64,
            1024,
            "b" * 40,
        )
        manifest = package.package_manifest(bundle)
        self.assertEqual(manifest["sourceCommit"], "b" * 40)
        self.assertEqual(manifest["sourceBundleSha256"], "a" * 64)
        self.assertEqual(manifest["scope"], "inert-read-only-runtime")
        for field in (
            "autoStart",
            "credentialsIncluded",
            "nginxConfigActive",
            "updateRuntimeIncluded",
            "diagnosticPersistenceIncluded",
            "hardwareMutationIncluded",
        ):
            self.assertIs(manifest[field], False)

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
