from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import tools.build_debian_runtime_package as runtime_package
import tools.build_debian_update_runtime_package as package
import tools.stage_device_prototype as prototype


class DebianUpdateRuntimePackageTests(unittest.TestCase):
    def test_package_is_separate_inert_and_bound_to_dev2_runtime(self) -> None:
        self.assertEqual(package.PACKAGE_NAME, "hardened-blikvm-update-runtime")
        self.assertEqual(package.PACKAGE_VERSION, "0.1.0~dev1")
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
