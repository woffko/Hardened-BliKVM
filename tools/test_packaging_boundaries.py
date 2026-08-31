from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent


class PackagingBoundaryTests(unittest.TestCase):
    def test_fan_policy_is_pure_host_only_and_not_linked_to_runtime(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        manifest = (ROOT / "crates/bli-kvm-fan-policy/Cargo.toml").read_text()
        source = (ROOT / "crates/bli-kvm-fan-policy/src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-fan-policy"', workspace)
        self.assertIn("thiserror.workspace = true", manifest)
        for runtime_manifest in (
            "crates/bli-kvm-platform-bli/Cargo.toml",
            "crates/bli-kvm-platform-broker/Cargo.toml",
            "crates/bli-kvm-server/Cargo.toml",
        ):
            self.assertNotIn("bli-kvm-fan-policy", (ROOT / runtime_manifest).read_text())
        self.assertIn("pub const FAILSAFE_DUTY_PERCENT: u8 = 100", source)
        for forbidden in ("/dev/", "gpio", "softPwm", "std::fs", "libc::", "tokio::"):
            self.assertNotIn(forbidden, source)

    def test_fan_supervisor_is_mockable_unlinked_and_has_no_concrete_actuator(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        manifest = (ROOT / "crates/bli-kvm-fan-supervisor/Cargo.toml").read_text()
        source = (ROOT / "crates/bli-kvm-fan-supervisor/src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-fan-supervisor"', workspace)
        self.assertIn("bli-kvm-fan-policy", manifest)
        for runtime_manifest in (
            "crates/bli-kvm-platform-bli/Cargo.toml",
            "crates/bli-kvm-platform-broker/Cargo.toml",
            "crates/bli-kvm-server/Cargo.toml",
        ):
            self.assertNotIn("bli-kvm-fan-supervisor", (ROOT / runtime_manifest).read_text())
        self.assertIn("pub trait FanActuator", source)
        self.assertIn("pub fn prepare_shutdown", source)
        for forbidden in ("/dev/", "softPwm", "std::fs", "libc::", "tokio::"):
            self.assertNotIn(forbidden, source)

    def test_serial_switch_codec_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-serial-switch-codec"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-serial-switch-codec"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-serial-switch-codec", other_manifest.read_text()
                )
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub const SELECT_FRAME_LENGTH: usize = 10", source)
        self.assertIn("pub const STATUS_FRAME_LENGTH: usize = 5", source)
        for forbidden in (
            "/dev/",
            "std::",
            "std::os::unix",
            "libc::",
            "tokio::",
            "termios::",
            "serialport",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_st7789_model_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-st7789-model"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-st7789-model"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn("bli-kvm-st7789-model", other_manifest.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub const DISPLAY_WIDTH: u16 = 240", source)
        self.assertIn("pub const MAX_PIXEL_CHUNK_PIXELS", source)
        for forbidden in (
            "/dev/",
            "std::",
            "libc::",
            "tokio::",
            "spidev",
            "SPI_IOC",
            "gpiod",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_display_button_policy_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-display-button-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-display-button-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-display-button-policy", other_manifest.read_text()
                )
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("WakeDisplayCycle", source)
        self.assertIn("HeldLatchedUntilRelease", source)
        for forbidden in (
            "/dev/",
            "std::",
            "libc::",
            "tokio::",
            "gpiod",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_activity_led_policy_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-activity-led-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-activity-led-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn("bli-kvm-activity-led-policy", other_manifest.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("FaultInactive", source)
        self.assertIn("inactive_level", source)
        for forbidden in (
            "/dev/",
            "std::",
            "libc::",
            "tokio::",
            "gpiod",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_system_action_policy_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-system-action-policy"
        journal = ROOT / "crates/bli-kvm-system-action-journal/Cargo.toml"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-system-action-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in (crate / "Cargo.toml", journal):
                self.assertNotIn("bli-kvm-system-action-policy", other_manifest.read_text())
        self.assertIn("bli-kvm-system-action-policy", journal.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub enum SystemAction", source)
        self.assertIn("pub const fn disabled()", source)
        self.assertIn("PostBootAccepted", source)
        for forbidden in (
            "/sbin/",
            "/usr/bin/",
            "std::",
            "std::process",
            "libc::",
            "tokio::",
            "systemctl",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_system_action_journal_is_pure_no_std_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-system-action-journal"
        store = ROOT / "crates/bli-kvm-system-action-store/Cargo.toml"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-system-action-journal"', workspace)
        self.assertIn("bli-kvm-system-action-policy", manifest)
        self.assertIn("sha2.workspace = true", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in (crate / "Cargo.toml", store):
                self.assertNotIn("bli-kvm-system-action-journal", other_manifest.read_text())
        self.assertIn("bli-kvm-system-action-journal", store.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("JOURNAL_COPY_BYTES", source)
        self.assertIn("pub fn select_copies", source)
        self.assertIn("pub fn plan_update", source)
        for forbidden in (
            "/dev/",
            "/var/",
            "std::",
            "std::fs",
            "libc::",
            "tokio::",
            "systemctl",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, source)

    def test_system_action_store_is_exact_root_atomic_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-system-action-store"
        maintenance = ROOT / "crates/bli-kvm-system-action-maintenance/Cargo.toml"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-system-action-store"', workspace)
        self.assertIn("bli-kvm-system-action-journal", manifest)
        self.assertIn("libc.workspace = true", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in (crate / "Cargo.toml", maintenance):
                self.assertNotIn("bli-kvm-system-action-store", other_manifest.read_text())
        self.assertIn("bli-kvm-system-action-store", maintenance.read_text())
        self.assertIn(
            '"/var/lib/hardened-blikvm-system-actions"', source
        )
        self.assertIn('c"journal-a.bin"', source)
        self.assertIn('c"journal-b.bin"', source)
        self.assertIn("libc::O_NOFOLLOW", source)
        self.assertIn("libc::O_NONBLOCK", source)
        self.assertIn("libc::flock", source)
        self.assertIn("libc::renameat", source)
        self.assertIn("system.posix_acl_access", source)
        self.assertIn("system.posix_acl_default", source)
        self.assertIn("sync_all", source)
        self.assertIn("verify_written", source)
        for forbidden in (
            "/dev/",
            "std::process",
            "Command::",
            "tokio::",
            "systemctl",
            "sh -c",
        ):
            self.assertNotIn(forbidden, source)

    def test_system_action_maintenance_is_static_confined_and_unshipped(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-system-action-maintenance"
        manifest = (crate / "Cargo.toml").read_text()
        library = (crate / "src/lib.rs").read_text()
        main = (crate / "src/main.rs").read_text()
        production_library = library.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-system-action-maintenance"', workspace)
        self.assertIn("bli-kvm-system-action-store", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-system-action-maintenance", other_manifest.read_text()
                )
        self.assertIn('"initialize-empty"', production_library)
        self.assertIn('"inspect-empty"', production_library)
        for forbidden in (
            '"repair"',
            '"reboot"',
            '"shutdown"',
            "std::process",
            "tokio::",
            "systemctl",
        ):
            self.assertNotIn(forbidden, production_library)
            self.assertNotIn(forbidden, main)

        init_unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-system-action-store-init.service"
        ).read_text()
        inspect_unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-system-action-store-inspect.service"
        ).read_text()
        for unit in (init_unit, inspect_unit):
            self.assertIn("User=root", unit)
            self.assertIn("PrivateNetwork=yes", unit)
            self.assertIn("PrivateDevices=yes", unit)
            self.assertIn("ProtectSystem=strict", unit)
            self.assertIn("RestrictAddressFamilies=AF_UNIX", unit)
            self.assertIn("CapabilityBoundingSet=\n", unit)
            self.assertIn("DevicePolicy=closed", unit)
            self.assertIn("IPAddressDeny=any", unit)
            self.assertIn(
                "ReadWritePaths=/var/lib/hardened-blikvm-system-actions", unit
            )
            self.assertNotIn("[Install]", unit)
            self.assertNotIn("systemctl", unit)
        self.assertIn(" initialize-empty", init_unit)
        self.assertIn(" inspect-empty", inspect_unit)

        tmpfiles = (
            ROOT / "packaging/tmpfiles.d/hardened-blikvm-system-actions.conf"
        ).read_text()
        self.assertEqual(
            tmpfiles,
            "# Type Path Mode User Group Age Argument\n"
            "d /var/lib/hardened-blikvm-system-actions 0700 root root - -\n",
        )
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertNotIn("bli-kvm-system-action-maintenance", builder)
        self.assertNotIn("hardened-blikvm-system-action-store-init", builder)
        self.assertNotIn("hardened-blikvm-system-actions.conf", builder)

    def test_nginx_terminates_tls_to_one_fixed_loopback_upstream(self) -> None:
        config = (ROOT / "packaging/nginx/hardened-blikvm.conf").read_text()
        self.assertIn("ssl_protocols TLSv1.2 TLSv1.3;", config)
        self.assertEqual(config.count("proxy_pass http://127.0.0.1:18080;"), 1)
        self.assertIn(
            "proxy_set_header Host $hardened_blikvm_forwarded_host;", config
        )
        self.assertIn("proxy_set_header Upgrade $http_upgrade;", config)
        self.assertNotIn("proxy_pass http://0.0.0.0", config)
        self.assertNotIn("BEGIN PRIVATE KEY", config)

    def test_server_unit_requires_secure_proxy_cookies(self) -> None:
        unit = (
            ROOT / "packaging/systemd/hardened-blikvm-server.service"
        ).read_text()
        self.assertIn("Environment=BLI_KVM_BIND=127.0.0.1:18080", unit)
        self.assertIn("Environment=BLI_KVM_SECURE_COOKIES=1", unit)
        self.assertIn(
            "Environment=BLI_KVM_VIDEO_SOCKET=/run/hardened-blikvm-video/mjpeg.sock",
            unit,
        )
        self.assertIn("IPAddressDeny=any", unit)
        self.assertIn("IPAddressAllow=localhost", unit)
        self.assertIn("SocketBindDeny=any", unit)
        self.assertIn("SocketBindAllow=tcp:18080", unit)
        self.assertNotIn("SupplementaryGroups=kvmd", unit)

    def test_video_proxy_has_a_distinct_unprivileged_unix_only_boundary(self) -> None:
        unit = (
            ROOT / "packaging/systemd/hardened-blikvm-video-proxy.service"
        ).read_text()
        self.assertIn("User=hardened-blikvm-video", unit)
        self.assertIn("SupplementaryGroups=kvmd", unit)
        self.assertIn("PrivateNetwork=yes", unit)
        self.assertIn("RestrictAddressFamilies=AF_UNIX", unit)
        self.assertIn("CapabilityBoundingSet=\n", unit)
        self.assertIn("DevicePolicy=closed", unit)
        self.assertIn("ReadWritePaths=/run/hardened-blikvm-video", unit)
        self.assertNotIn("AF_INET", unit)

    def test_control_broker_is_denied_and_has_no_backend_privilege(self) -> None:
        unit = (
            ROOT / "packaging/systemd/hardened-blikvm-control-broker.service"
        ).read_text()
        self.assertIn("User=hardened-blikvm-control", unit)
        self.assertIn("PrivateNetwork=yes", unit)
        self.assertIn("RestrictAddressFamilies=AF_UNIX", unit)
        self.assertIn("CapabilityBoundingSet=\n", unit)
        self.assertIn("DevicePolicy=closed", unit)
        self.assertNotIn("SupplementaryGroups=kvmd", unit)
        self.assertNotIn("LoadCredential", unit)
        self.assertNotIn("/run/kvmd", unit)

    def test_prototype_builder_includes_templates_but_not_tls_keys(self) -> None:
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn("packaging/nginx/hardened-blikvm.conf", builder)
        self.assertIn("bli-kvm-video-proxy", builder)
        self.assertIn("bli-kvm-control-broker", builder)
        self.assertIn("bli-kvm-update-supervisor", builder)
        self.assertIn("bli-kvm-update-health-reporter", builder)
        self.assertIn("hardened-blikvm-update-watchdog.timer", builder)
        self.assertIn("hardened-blikvm-update-confirm.socket", builder)
        self.assertIn("packaging/tmpfiles.d/hardened-blikvm-update.conf", builder)
        self.assertIn("docs/api/inventory-v2.md", builder)
        self.assertIn("docs/api/inventory-v3.md", builder)
        self.assertNotIn("tls.key", builder)
        self.assertNotIn("tls.crt", builder)

    def test_update_supervisor_is_local_health_only_and_sandboxed(self) -> None:
        watchdog = (
            ROOT / "packaging/systemd/hardened-blikvm-update-watchdog.service"
        ).read_text()
        confirmation = (
            ROOT / "packaging/systemd/hardened-blikvm-update-confirm@.service"
        ).read_text()
        socket = (
            ROOT / "packaging/systemd/hardened-blikvm-update-confirm.socket"
        ).read_text()
        timer = (
            ROOT / "packaging/systemd/hardened-blikvm-update-watchdog.timer"
        ).read_text()
        for unit in (watchdog, confirmation):
            self.assertIn("User=hardened-blikvm-update", unit)
            self.assertIn("ProtectSystem=strict", unit)
            self.assertIn("ProtectProc=invisible", unit)
            self.assertIn("IPAddressDeny=any", unit)
            self.assertIn("CapabilityBoundingSet=\n", unit)
            self.assertIn("DevicePolicy=closed", unit)
            self.assertNotIn("AF_INET", unit)
            self.assertNotIn("LoadCredential", unit)
        self.assertIn("StandardInput=socket", confirmation)
        self.assertIn("confirm-stdin", confirmation)
        self.assertIn(
            "ReadWritePaths=/var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
            confirmation,
        )
        self.assertIn(
            "ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
            watchdog,
        )
        self.assertIn("Accept=yes", socket)
        self.assertIn("SocketMode=0620", socket)
        self.assertIn("SocketGroup=hardened-blikvm", socket)
        self.assertNotIn("ListenStream=0.0.0.0", socket)
        self.assertIn("OnUnitActiveSec=2s", timer)

    def test_candidate_orchestration_is_fixed_and_credential_gated(self) -> None:
        candidate = (
            ROOT / "packaging/systemd/hardened-blikvm-server-candidate.service"
        ).read_text()
        current = (
            ROOT / "packaging/systemd/hardened-blikvm-server-current.service"
        ).read_text()
        activation = (
            ROOT / "packaging/systemd/hardened-blikvm-update-activate@.service"
        ).read_text()
        apply_candidate = (
            ROOT
            / "packaging/systemd/hardened-blikvm-update-apply-candidate.service"
        ).read_text()
        apply_confirmed = (
            ROOT
            / "packaging/systemd/hardened-blikvm-update-apply-confirmed.service"
        ).read_text()
        tmpfiles = (
            ROOT / "packaging/tmpfiles.d/hardened-blikvm-update.conf"
        ).read_text()
        self.assertIn(
            "LoadCredential=application-health-token:/run/hardened-blikvm-update/health-token",
            candidate,
        )
        self.assertIn("bli-kvm-update-health-reporter", candidate)
        self.assertIn("--health-endpoint 127.0.0.1:18080", candidate)
        self.assertIn("--confirm-socket /run/hardened-blikvm-update-confirm/confirm.sock", candidate)
        self.assertIn(
            "ExecStart=/var/lib/hardened-blikvm-slots/current/bin/bli-kvm-server",
            candidate,
        )
        self.assertIn(
            "ExecStart=/var/lib/hardened-blikvm-slots/current/bin/bli-kvm-server",
            current,
        )
        self.assertIn("activate %i", activation)
        self.assertNotIn("sh -c", activation)
        self.assertIn(
            "ReadWritePaths=/var/lib/hardened-blikvm-slots /var/lib/hardened-blikvm-update /run/hardened-blikvm-update",
            activation,
        )
        self.assertIn(
            "systemctl restart hardened-blikvm-server-candidate.service",
            apply_candidate,
        )
        self.assertIn(
            "systemctl restart hardened-blikvm-server-current.service",
            apply_confirmed,
        )
        self.assertNotIn("%i", apply_candidate)
        self.assertNotIn("%i", apply_confirmed)
        self.assertIn(
            "d /var/lib/hardened-blikvm-slots 0750 hardened-blikvm-update hardened-blikvm",
            tmpfiles,
        )
        self.assertIn(
            "d /run/hardened-blikvm-update-confirm 0750 hardened-blikvm-update hardened-blikvm",
            tmpfiles,
        )


if __name__ == "__main__":
    _ = unittest.main()
