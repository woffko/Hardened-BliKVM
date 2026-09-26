from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import tools.build_debian_runtime_package as runtime_package
import tools.build_debian_update_runtime_package as package
import tools.stage_device_prototype as prototype


class DebianUpdateRuntimePackageTests(unittest.TestCase):
    def test_shared_update_profile_is_staged_for_all_server_roles_and_inert_in_deb(self) -> None:
        root = Path(__file__).resolve().parents[1]
        with tempfile.TemporaryDirectory() as directory:
            stage = Path(directory)
            result = subprocess.run(
                ["bash", str(root / "scripts/stage-application-update-profile.sh"), str(stage)],
                capture_output=True, text=True, timeout=5, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            expected = (root / "packaging/systemd/hardened-blikvm-board-server.service.d/94-application-update.conf").read_bytes()
            staged = sorted(stage.rglob("*.conf"))
            self.assertEqual(len(staged), 3)
            for path in staged:
                self.assertEqual(path.read_bytes(), expected)
                self.assertEqual(path.stat().st_mode & 0o777, 0o644)
                source = str(path.relative_to(stage))
                destination = package.file_map(package.PACKAGE_VERSION)[source]
                self.assertTrue(destination.startswith(f"usr/share/doc/{package.PACKAGE_NAME}/examples/"))
                introduced = package.SUPPORTED_PACKAGE_VERSIONS.index("0.1.0~dev10")
                for version in package.SUPPORTED_PACKAGE_VERSIONS[:introduced]:
                    self.assertNotIn(source, package.file_map(version))
                for version in package.SUPPORTED_PACKAGE_VERSIONS[introduced:]:
                    self.assertIn(source, package.file_map(version))
            text = expected.decode("utf-8")
            for surface in ("status", "upload", "activation"):
                socket = (root / f"packaging/systemd/hardened-blikvm-update-{surface}.socket").read_text()
                endpoint = next(line.removeprefix("ListenStream=") for line in socket.splitlines() if line.startswith("ListenStream="))
                self.assertIn(f"Environment=BLI_KVM_UPDATE_{surface.upper()}_SOCKET={endpoint}\n", text)
            self.assertIn("Environment=BLI_KVM_UPDATE_WRITES_ENABLED=1\n", text)

    def test_activation_units_start_at_dev9_with_fixed_success_dispatch(self) -> None:
        units = {
            "lib/systemd/system/hardened-blikvm-update-activation.socket",
            "lib/systemd/system/hardened-blikvm-update-activation@.service",
        }
        self.assertTrue(units <= set(package.file_map("0.1.0~dev9").values()))
        for version in package.SUPPORTED_PACKAGE_VERSIONS[:8]:
            self.assertFalse(units & set(package.file_map(version).values()))
        worker = Path("packaging/systemd/hardened-blikvm-update-activation@.service").read_text()
        socket = Path("packaging/systemd/hardened-blikvm-update-activation.socket").read_text()
        for required in ("Type=oneshot", "TimeoutStartSec=15s", "activation-socket --peer-user hardened-blikvm-server",
                         "OnSuccess=hardened-blikvm-update-apply-candidate.service hardened-blikvm-update-apply-confirmed.service",
                         "ReadWritePaths=/var/lib/hardened-blikvm-slots -/var/lib/hardened-blikvm-slots-v2 /var/lib/hardened-blikvm-update -/var/lib/hardened-blikvm-update-v2 /run/hardened-blikvm-update -/run/hardened-blikvm-update-v2"):
            self.assertIn(required, worker)
        for forbidden in ("SuccessExitStatus=", "OnFailure=", "ConditionPath", "ExecStart=/usr/bin/systemctl", " activate %i"):
            self.assertNotIn(forbidden, worker)
        self.assertIn("MaxConnections=1", socket)
        self.assertIn("ListenStream=/run/hardened-blikvm-update-api/activation.sock", socket)

    def test_native_helper_starts_at_dev11_and_replacements_stay_examples(self) -> None:
        installed = {
            "usr/libexec/hardened-blikvm/bli-kvm-update-exec-helper",
            "lib/systemd/system/hardened-blikvm-update-exec.socket",
            "lib/systemd/system/hardened-blikvm-update-exec@.service",
            "lib/systemd/system/hardened-blikvm-update-native-boot.service",
        }
        current = set(package.file_map(package.PACKAGE_VERSION).values())
        self.assertTrue(installed <= current)
        self.assertIn("usr/libexec/hardened-blikvm/bli-kvm-update-exec-helper",
                      package.EXECUTABLE_DESTINATIONS)
        examples = {destination for destination in current if "/examples/native/" in destination}
        self.assertEqual(len(examples), len(package.NATIVE_PROFILE_UNITS))
        first_native = package.SUPPORTED_PACKAGE_VERSIONS.index("0.1.0~dev11")
        for version in package.SUPPORTED_PACKAGE_VERSIONS[first_native:]:
            self.assertTrue(installed <= set(package.file_map(version).values()))
        for version in package.SUPPORTED_PACKAGE_VERSIONS[:first_native]:
            older = set(package.file_map(version).values())
            self.assertFalse((installed | examples) & older)
            # Older revisions keep their base server units.
            self.assertIn("lib/systemd/system/hardened-blikvm-server-current.service", older)
        # Native replacements never shadow the runtime package's base units.
        for destination in examples:
            self.assertTrue(destination.startswith(f"usr/share/doc/{package.PACKAGE_NAME}/examples/native/"))

    def test_upload_units_start_at_dev8_with_stage_only_disk_spool(self) -> None:
        units = {
            "lib/systemd/system/hardened-blikvm-update-upload.socket",
            "lib/systemd/system/hardened-blikvm-update-upload@.service",
        }
        self.assertTrue(units <= set(package.file_map("0.1.0~dev8").values()))
        for version in package.SUPPORTED_PACKAGE_VERSIONS[:7]:
            self.assertFalse(units & set(package.file_map(version).values()))
        worker = Path("packaging/systemd/hardened-blikvm-update-upload@.service").read_text()
        socket = Path("packaging/systemd/hardened-blikvm-update-upload.socket").read_text()
        for required in ("Type=simple", "RuntimeMaxSec=180s", "TimeoutStopSec=5s",
                         "StateDirectory=hardened-blikvm-upload", "StateDirectoryMode=0700",
                         "upload-socket --peer-user hardened-blikvm-server",
                         "ReadWritePaths=/var/lib/hardened-blikvm-slots -/var/lib/hardened-blikvm-slots-v2 /var/lib/hardened-blikvm-upload"):
            self.assertIn(required, worker)
        for forbidden in ("OnSuccess=", " activate ", " confirm-stdin", "RuntimeDirectory=",
                          "ReadWritePaths=/var/lib/hardened-blikvm-update"):
            self.assertNotIn(forbidden, worker)
        self.assertIn("MaxConnections=1", socket)
        self.assertIn("Accept=yes", socket)
        self.assertIn("ListenStream=/run/hardened-blikvm-update-api/upload.sock", socket)

    def test_status_units_are_available_from_dev6_and_remain_read_only(self) -> None:
        units = {
            "lib/systemd/system/hardened-blikvm-update-status.socket",
            "lib/systemd/system/hardened-blikvm-update-status@.service",
        }
        self.assertTrue(units <= set(package.file_map("0.1.0~dev6").values()))
        self.assertTrue(units <= set(package.file_map("0.1.0~dev7").values()))
        for version in package.SUPPORTED_PACKAGE_VERSIONS[:5]:
            self.assertFalse(units & set(package.file_map(version).values()))
        worker = Path("packaging/systemd/hardened-blikvm-update-status@.service").read_text()
        for forbidden in ("ReadWritePaths=", "OnSuccess=", "Requires=", "StateDirectory=", "RuntimeDirectory="):
            self.assertNotIn(forbidden, worker)
        self.assertIn("ProtectSystem=strict", worker)
        self.assertIn("status-socket --peer-user hardened-blikvm-server", worker)

    def test_legacy_package_maps_do_not_require_later_recovery_unit(self) -> None:
        recovery = "lib/systemd/system/hardened-blikvm-update-recover.service"
        self.assertIn(recovery, package.file_map("0.1.0~dev5").values())
        for version in ("0.1.0~dev1", "0.1.0~dev2", "0.1.0~dev3", "0.1.0~dev4"):
            self.assertNotIn(recovery, package.file_map(version).values())

    def test_package_is_separate_inert_and_bound_to_dev2_runtime(self) -> None:
        self.assertEqual(package.PACKAGE_NAME, "hardened-blikvm-update-runtime")
        self.assertEqual(package.PACKAGE_VERSION, "0.1.0~dev25")
        self.assertEqual(
            package.RUNTIME_DEPENDENCY,
            "hardened-blikvm-runtime (= 0.1.0~dev2)",
        )
        destinations = set(package.FILE_MAP.values())
        for required in (
            "usr/libexec/hardened-blikvm/bli-kvm-update-supervisor",
            "usr/libexec/hardened-blikvm/bli-kvm-update-health-reporter",
            "lib/systemd/system/hardened-blikvm-update-watchdog.timer",
            "lib/systemd/system/hardened-blikvm-update-confirm.socket",
            "lib/systemd/system/hardened-blikvm-update-status.socket",
            "lib/systemd/system/hardened-blikvm-update-status@.service",
            "lib/systemd/system/hardened-blikvm-server-current.service",
            "lib/systemd/system/hardened-blikvm-server-candidate.service",
        ):
            self.assertIn(required, destinations)
        for destination in destinations:
            self.assertFalse(destination.startswith("etc/"))
            self.assertFalse(destination.startswith("usr/lib/sysusers.d/"))
            self.assertFalse(destination.startswith("usr/lib/tmpfiles.d/"))

    def test_manifest_has_no_trust_activation_state_or_route(self) -> None:
        bundle = prototype.BundleInfo(
            Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40
        )
        manifest = package.package_manifest(bundle, "c" * 40)
        self.assertEqual(manifest["scope"], "inert-application-update-runtime")
        self.assertEqual(manifest["runtimeDependency"], package.RUNTIME_DEPENDENCY)
        for field in (
            "autoStart",
            "unitsEnabled",
            "trustConfigured",
            "privateKeyIncluded",
            "configurationIncluded",
            "stateInitialized",
            "slotRootCreated",
            "httpRouteIncluded",
            "guiRouteIncluded",
            "systemUpdateIncluded",
            "hardwareMutationIncluded",
        ):
            self.assertIs(manifest[field], False)

    def test_rollback_manifest_requires_an_explicit_supported_old_version(self) -> None:
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        current = package.package_manifest(bundle, "c" * 40)
        previous = package.package_manifest(bundle, "c" * 40, version="0.1.0~dev1")
        self.assertEqual(current["version"], "0.1.0~dev25")
        self.assertEqual(previous["version"], "0.1.0~dev1")
        self.assertEqual({**previous, "version": current["version"]}, current)
        with self.assertRaises(package.BuildError):
            package.package_manifest(bundle, "c" * 40, version="unreviewed")

    def test_audit_rejects_extra_and_active_configuration_files(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for relative in {
                "DEBIAN/control",
                *package.FILE_MAP.values(),
                "usr/share/doc/hardened-blikvm-update-runtime/PACKAGE-MANIFEST.json",
            }:
                path = root / relative
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text("safe", encoding="utf-8")
            package.audit_package_tree(root)
            extra = root / "etc/hardened-blikvm/update-supervisor.json"
            extra.parent.mkdir(parents=True)
            extra.write_text("{}", encoding="utf-8")
            with self.assertRaises(package.BuildError):
                package.audit_package_tree(root)

    def test_runtime_builder_public_helpers_preserve_default_behavior(self) -> None:
        self.assertEqual(
            runtime_package.normalize_tree.__defaults__,
            (runtime_package.EXECUTABLE_DESTINATIONS,),
        )

    def test_normalizer_overrides_umask_for_package_root(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary) / "package"
            root.mkdir(mode=0o700)
            binary = root / next(iter(package.EXECUTABLE_DESTINATIONS))
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"binary")
            runtime_package.normalize_tree(root, package.EXECUTABLE_DESTINATIONS)
            self.assertEqual(root.stat().st_mode & 0o777, 0o755)
            self.assertEqual(binary.stat().st_mode & 0o777, 0o755)

    def test_direct_entrypoint_loads_project_package(self) -> None:
        result = subprocess.run(
            [sys.executable, "tools/build_debian_update_runtime_package.py", "--help"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
        self.assertEqual(result.returncode, 0, result.stderr.decode())

    def test_design_keeps_trust_and_activation_separate(self) -> None:
        design = Path("docs/design/update-runtime-package-v1.md").read_text(
            encoding="utf-8"
        )
        for required in (
            "does not grant update",
            "authority or alter the running application",
            "does not install an active sysusers or tmpfiles fragment",
            "all update units disabled/inactive",
            "Production public-key enrollment",
            "ATX Power/Reset",
        ):
            self.assertIn(required, design)
        for forbidden in ("10.0.", "admin1234", "ssh_password", "gui_password"):
            self.assertNotIn(forbidden, design)


if __name__ == "__main__":
    unittest.main()
