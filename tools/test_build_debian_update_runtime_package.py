from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

import tools.build_debian_runtime_package as runtime_package
import tools.build_debian_update_runtime_package as package
import tools.stage_device_prototype as prototype
import tools.test_ve_module as module_fixture
import tools.ve_module as ve_module


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

    def test_quic_gateway_starts_at_dev35_disabled_and_confined(self) -> None:
        installed = {
            "usr/libexec/hardened-blikvm/bli-kvm-quic-gateway",
            "lib/systemd/system/hardened-blikvm-quic.service",
        }
        self.assertTrue(installed <= set(package.file_map("0.1.0~dev35").values()))
        self.assertIn("usr/libexec/hardened-blikvm/bli-kvm-quic-gateway",
                      package.EXECUTABLE_DESTINATIONS)
        first = package.SUPPORTED_PACKAGE_VERSIONS.index("0.1.0~dev35")
        for version in package.SUPPORTED_PACKAGE_VERSIONS[:first]:
            self.assertFalse(installed & set(package.file_map(version).values()), version)
        for version in package.SUPPORTED_PACKAGE_VERSIONS[first:]:
            self.assertTrue(installed <= set(package.file_map(version).values()), version)
        # The gateway is not the slot's: the application package must not carry it.
        self.assertIn("bli-kvm-quic-gateway", runtime_package.FORBIDDEN_PACKAGE_NAMES)
        self.assertIn("hardened-blikvm-quic.service", runtime_package.FORBIDDEN_PACKAGE_NAMES)
        unit = Path("packaging/systemd/hardened-blikvm-quic.service").read_text()
        for required in (
            "User=hardened-blikvm-quic",
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-quic-gateway",
            "Environment=BLI_KVM_QUIC_LINK_SOCKET=/run/hardened-blikvm-quic-link/link.sock",
            "AmbientCapabilities=CAP_NET_BIND_SERVICE",
            "CapabilityBoundingSet=CAP_NET_BIND_SERVICE",
            "SocketBindDeny=any",
            "SocketBindAllow=udp:443",
            "NoNewPrivileges=yes",
            "ProtectSystem=strict",
            "MemoryMax=64M",
            "RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6",
            "WantedBy=multi-user.target",
        ):
            self.assertIn(required, unit)
        # It writes nothing, runs no hook and is nobody's dependency: off until
        # the owner enables it.
        for forbidden in ("ReadWritePaths=", "StateDirectory=", "RuntimeDirectory=", "ExecStartPre=",
                          "ExecStartPost=", "User=root", "Group=hardened-blikvm\n"):
            self.assertNotIn(forbidden, unit)
        for other in Path("packaging/systemd").rglob("*"):
            if other.is_file() and other.name != "hardened-blikvm-quic.service":
                self.assertNotIn("hardened-blikvm-quic.service", other.read_text(), other.name)
        # The serving units name the link and the only user admitted to it.
        for server in ("hardened-blikvm-server-current.service", "hardened-blikvm-server-candidate.service",
                       "native/hardened-blikvm-server-current.service",
                       "native/hardened-blikvm-server-candidate.service"):
            text = (Path("packaging/systemd") / server).read_text()
            self.assertIn("Environment=BLI_KVM_QUIC_LINK_SOCKET=/run/hardened-blikvm-quic-link/link.sock\n", text)
            self.assertIn("Environment=BLI_KVM_QUIC_LINK_USER=hardened-blikvm-quic\n", text)
            self.assertIn(
                "RuntimeDirectory=hardened-blikvm-stream hardened-blikvm-quic-link hardened-blikvm-proxy\n",
                text,
            )
        # Its user has a fragment of its own and no shared group; the
        # runtime's pinned identities do not change.
        users = Path("packaging/sysusers.d/hardened-blikvm-quic.conf").read_text()
        self.assertIn('u hardened-blikvm-quic - "Hardened BliKVM QUIC gateway"', users)
        self.assertNotIn("\nm ", users)
        self.assertNotIn("\ng ", users)
        self.assertNotIn("quic", Path("packaging/sysusers.d/hardened-blikvm.conf").read_text())
        fragment = "usr/share/doc/hardened-blikvm-update-runtime/examples/hardened-blikvm-quic-sysusers.conf"
        self.assertIn(fragment, package.file_map("0.1.0~dev35").values())
        self.assertNotIn(fragment, package.file_map("0.1.0~dev34").values())

    def test_the_encoder_driver_starts_at_dev36_for_one_kernel_and_disabled(self) -> None:
        examples = f"usr/share/doc/{package.PACKAGE_NAME}/examples"
        source = f"usr/share/doc/{package.PACKAGE_NAME}/ve-module-source"
        installed = {
            "usr/lib/hardened-blikvm/modules/5.19.4-sunxi64/hardened-blikvm-ve-enc.ko",
            "usr/libexec/hardened-blikvm/bli-kvm-ve-load",
            "lib/systemd/system/hardened-blikvm-ve.service",
            f"{examples}/99-hardened-blikvm-ve.rules",
            f"{examples}/hardened-blikvm-ve-sysusers.conf",
            f"{examples}/hardened-blikvm-ve-modprobe.conf",
            *(f"{source}/{name}" for name in package.VE_MODULE_SOURCE_FILES),
        }
        current = package.file_map("0.1.0~dev36")
        self.assertTrue(installed <= set(current.values()))
        self.assertEqual(
            set(current.values()) - set(package.file_map("0.1.0~dev35").values()), installed
        )
        self.assertIn("usr/libexec/hardened-blikvm/bli-kvm-ve-load", package.EXECUTABLE_DESTINATIONS)
        # One module, in a directory named after the one release it loads on.
        modules = [destination for destination in current.values() if destination.endswith(".ko")]
        self.assertEqual(modules, [package.VE_MODULE_DESTINATION])
        self.assertEqual(
            package.VE_MODULE_DESTINATION,
            f"{ve_module.MODULE_ROOT}/{package.VE_KERNEL_RELEASE}/{ve_module.MODULE_FILE}",
        )
        # Nothing of it is active by being installed: no rule, group or
        # modprobe line outside the examples, and no module where the kernel
        # would find it by itself.
        for destination in current.values():
            self.assertFalse(destination.startswith(("etc/", "lib/modules/", "usr/lib/modules/",
                                                     "lib/udev/", "usr/lib/udev/", "lib/modprobe.d/",
                                                     "usr/lib/modprobe.d/")), destination)
        # The lab's instruments are in no package.
        for name in (*current, *current.values()):
            for lab in ("ve-lab", "ve_lab", "bli-kvm-ve-lab", "ve_trace", "ve-trace"):
                self.assertNotIn(lab, name)
        # The module's source goes with it, without the lab module's.
        self.assertEqual(
            set(package.VE_MODULE_SOURCE_FILES),
            {"README.md", "ve_enc.c", "ve_enc_abi.h", "ve_enc_plan.c", "ve_enc_plan.h", "ve_hw.c", "ve_hw.h"},
        )
        makefile = Path("kernel/hardened-blikvm-ve/Makefile").read_text()
        self.assertIn("hardened-blikvm-ve-enc-y := ve_enc.o ve_enc_plan.o ve_hw.o", makefile)
        readme = Path("packaging/ve/SOURCE.md").read_text()
        self.assertIn("hardened-blikvm-ve-enc-y := ve_enc.o ve_enc_plan.o ve_hw.o", readme)
        self.assertIn("GPL-2.0-only", readme)
        for name in package.VE_MODULE_SOURCE_FILES[1:]:
            text = (Path("kernel/hardened-blikvm-ve") / name).read_text()
            self.assertIn("SPDX-License-Identifier: GPL-2.0-only", text.splitlines()[0], name)
        # Neither the application package nor a slot carries any of it.
        for name in ("hardened-blikvm-ve-enc.ko", "bli-kvm-ve-load", "hardened-blikvm-ve.service"):
            self.assertIn(name, runtime_package.FORBIDDEN_PACKAGE_NAMES)
        slot_builder = Path("tools/build_production_update_bundle.py").read_text()
        for name in ("ve-enc", "bli-kvm-ve-load", "hardened-blikvm-ve"):
            self.assertNotIn(name, slot_builder)

    def test_the_driver_is_loaded_by_a_unit_with_one_capability(self) -> None:
        unit = Path("packaging/systemd/hardened-blikvm-ve.service").read_text()
        for required in (
            "ConditionPathExists=/usr/lib/hardened-blikvm/modules/%v/hardened-blikvm-ve-enc.ko",
            "Before=hardened-blikvm-video-encoder-h264.service",
            "Type=oneshot",
            "RemainAfterExit=yes",
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-ve-load",
            "CapabilityBoundingSet=CAP_SYS_MODULE\n",
            "NoNewPrivileges=yes",
            "PrivateNetwork=yes",
            "ProtectSystem=strict",
            "SystemCallFilter=@system-service @module",
            "WantedBy=multi-user.target",
        ):
            self.assertIn(required, unit)
        for forbidden in ("ExecStartPre=", "ExecStartPost=", "ReadWritePaths=", "StateDirectory=",
                          "Requires=", "Restart="):
            self.assertNotIn(forbidden, unit)
        # Nothing requires it: without the driver the encoder uses x264.
        for other in Path("packaging/systemd").rglob("*"):
            if other.is_file() and other.name != "hardened-blikvm-ve.service":
                text = other.read_text()
                for line in text.splitlines():
                    if "hardened-blikvm-ve.service" in line:
                        self.assertTrue(line.startswith("After="), f"{other.name}: {line}")
        loader = Path("packaging/ve/bli-kvm-ve-load").read_text()
        for required in (
            "file=/usr/lib/hardened-blikvm/modules/$release/hardened-blikvm-ve-enc.ko",
            "release=$(uname -r)",
            "allwinner,sun50i-h616-video-engine",
            'insmod "$file"',
            "udevadm settle",
            '"$group 660"',
        ):
            self.assertIn(required, loader)
        # It loads the one file by its path: no search of the kernel's
        # module directories, no force, no other driver unbound.
        commands = "\n".join(
            line for line in loader.splitlines() if not line.lstrip().startswith("#")
        )
        for forbidden in ("modprobe", "--force", "insmod -f", "depmod", "rmmod"):
            self.assertNotIn(forbidden, commands)
        self.assertEqual(commands.count("/unbind"), 1)
        self.assertIn('if [ "$holder" != cedrus ]; then', loader)
        rule = Path("packaging/udev/99-hardened-blikvm-ve.rules").read_text()
        self.assertIn(
            'SUBSYSTEM=="misc", KERNEL=="hardened-blikvm-ve-enc", GROUP="hardened-blikvm-ve", MODE="0660"\n',
            rule,
        )
        group = Path("packaging/sysusers.d/hardened-blikvm-ve.conf").read_text()
        self.assertIn("\ng hardened-blikvm-ve -\n", group)
        self.assertNotIn("\nu ", group)
        self.assertNotIn("\nm ", group)
        self.assertNotIn("hardened-blikvm-ve", Path("packaging/sysusers.d/hardened-blikvm.conf").read_text())
        self.assertIn("\nblacklist sunxi_cedrus\n", Path("packaging/modprobe.d/hardened-blikvm-ve.conf").read_text())
        # Only the H.264 encoder's unit is in the group and may open the device.
        for other in Path("packaging/systemd").rglob("*.service"):
            text = other.read_text()
            if other.name == "hardened-blikvm-video-encoder-h264.service":
                self.assertIn("SupplementaryGroups=hardened-blikvm hardened-blikvm-ve\n", text)
                self.assertIn("DeviceAllow=/dev/hardened-blikvm-ve-enc rw\n", text)
                self.assertIn("DevicePolicy=closed\n", text)
            else:
                self.assertNotIn("hardened-blikvm-ve\n", text, other.name)
                self.assertNotIn("hardened-blikvm-ve-enc", text.replace(
                    "hardened-blikvm-ve-enc.ko", ""), other.name)

    def test_the_capture_unit_does_not_name_a_device_number(self) -> None:
        unit = Path("packaging/systemd/hardened-blikvm-ustreamer.service").read_text()
        allowed = [line for line in unit.splitlines() if line.startswith("DeviceAllow=")]
        self.assertEqual(allowed, ["DeviceAllow=/dev/kvmd-video rw"])
        self.assertIn("DevicePolicy=closed\n", unit)
        self.assertIn("--device=/dev/kvmd-video", unit)
        self.assertIn("ConditionPathExists=/dev/kvmd-video\n", unit)
        rule = Path("packaging/udev/99-hardened-blikvm-video.rules").read_text()
        self.assertIn('SYMLINK+="kvmd-video"', rule)

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

    def test_dev40_adds_picoclaws_installer_to_dev39(self) -> None:
        # The fixes of the code audit of 2026-10-20: PicoClaw's installer has a
        # unit, a socket and an account of its own (AUD-05.2).
        dev40 = dict(package.file_map("0.1.0~dev40"))
        added = {
            "share/systemd/hardened-blikvm-picoclaw-install.socket",
            "share/systemd/hardened-blikvm-picoclaw-install@.service",
            "share/sysusers.d/hardened-blikvm-picoclaw-install.conf",
        }
        self.assertTrue(added <= set(dev40))
        for source in added:
            del dev40[source]
        self.assertEqual(dev40, package.file_map("0.1.0~dev39"))

    def test_dev41_carries_the_files_of_dev40(self) -> None:
        # dev41 changes only the native boot unit, which tries a failed
        # attempt again (code audit of 2026-10-20, gate Y).
        self.assertEqual(package.file_map("0.1.0~dev41"), package.file_map("0.1.0~dev40"))

    def test_dev42_adds_the_systems_nginx_configuration_to_dev41(self) -> None:
        # The system's own nginx configuration, outside the directory a RAW
        # update carries, and the step that puts it back before nginx starts
        # (code audit of 2026-10-20, gate Y).
        dev42 = dict(package.file_map("0.1.0~dev42"))
        added = {
            "share/nginx/hardened-blikvm.conf":
                "usr/share/hardened-blikvm/nginx/hardened-blikvm.conf",
            "share/systemd/hardened-blikvm-nginx.service.d/50-system-config.conf":
                "lib/systemd/system/hardened-blikvm-nginx.service.d/50-system-config.conf",
        }
        for source, destination in added.items():
            self.assertEqual(dev42.pop(source), destination)
        self.assertEqual(dev42, package.file_map("0.1.0~dev41"))

    def test_dev43_carries_the_files_of_dev42(self) -> None:
        # dev43 changes only the QUIC gateway, which says why a session
        # ended (2026-10-24).
        self.assertEqual(package.file_map("0.1.0~dev43"), package.file_map("0.1.0~dev42"))

    def test_dev44_carries_the_files_of_dev43(self) -> None:
        # dev44 changes only the QUIC gateway: its path MTU searched up to
        # 1360, and the path's measures said when a session ends
        # (2026-10-25).
        self.assertEqual(package.file_map("0.1.0~dev44"), package.file_map("0.1.0~dev43"))

    def test_dev45_carries_the_files_of_dev44(self) -> None:
        # dev45 has the same files, built with rustls-webpki 0.103.16
        # (RUSTSEC-2026-0382) and tokio 1.53.3 (RUSTSEC-2026-0386) for the
        # release 0.1.0-beta.10.
        self.assertEqual(package.PACKAGE_VERSION, "0.1.0~dev45")
        self.assertEqual(package.file_map("0.1.0~dev45"), package.file_map("0.1.0~dev44"))

    def test_dev37_to_dev39_carry_the_files_of_dev36(self) -> None:
        # The QUIC gateway of the transport's protocol 2, then the fixes of the
        # security review of 2026-10-18 with the download worker's account.
        fragment = (
            "usr/share/doc/hardened-blikvm-update-runtime/examples/"
            "hardened-blikvm-download-sysusers.conf"
        )
        dev39 = dict(package.file_map("0.1.0~dev39"))
        self.assertEqual(
            [source for source, target in dev39.items() if target == fragment],
            ["share/sysusers.d/hardened-blikvm-download.conf"],
        )
        del dev39["share/sysusers.d/hardened-blikvm-download.conf"]
        self.assertEqual(dev39, package.file_map("0.1.0~dev36"))
        for version in ("0.1.0~dev37", "0.1.0~dev38"):
            self.assertEqual(package.file_map(version), package.file_map("0.1.0~dev36"))

    def test_package_is_separate_inert_and_bound_to_dev2_runtime(self) -> None:
        self.assertEqual(package.PACKAGE_NAME, "hardened-blikvm-update-runtime")
        self.assertEqual(package.PACKAGE_VERSION, "0.1.0~dev45")
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
            "hardwareMutationIncluded",
        ):
            self.assertIs(manifest[field], False)
        self.assertIs(manifest["systemUpdateIncluded"], True)
        self.assertIs(manifest["kernelModuleIncluded"], True)
        self.assertIs(
            package.package_manifest(bundle, "c" * 40, version="0.1.0~dev35")["kernelModuleIncluded"],
            False,
        )

    def test_rollback_manifest_requires_an_explicit_supported_old_version(self) -> None:
        bundle = prototype.BundleInfo(Path("/bundle.tar.gz"), "a" * 64, 1024, "b" * 40)
        current = package.package_manifest(bundle, "c" * 40)
        previous = package.package_manifest(bundle, "c" * 40, version="0.1.0~dev1")
        self.assertEqual(current["version"], "0.1.0~dev45")
        self.assertEqual(previous["version"], "0.1.0~dev1")
        self.assertIs(previous["systemUpdateIncluded"], False)
        self.assertIs(previous["kernelModuleIncluded"], False)
        self.assertEqual(
            {**previous, "version": current["version"], "systemUpdateIncluded": True,
             "kernelModuleIncluded": True},
            current,
        )
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
            module = root / package.VE_MODULE_DESTINATION
            # What is in the module's place must be the encoder driver for
            # the release its directory names.
            with self.assertRaises(package.BuildError):
                package.audit_package_tree(root)
            for wrong in (
                module_fixture.module(release="6.1.0-sunxi64"),
                module_fixture.module(name="hardened_blikvm_ve_lab"),
            ):
                module.write_bytes(wrong)
                with self.assertRaises(package.BuildError):
                    package.audit_package_tree(root)
            module.write_bytes(module_fixture.module())
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
