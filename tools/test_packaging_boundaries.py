from pathlib import Path
import unittest


ROOT = Path(__file__).resolve().parent.parent
DIAGNOSTIC_SOCKET_LITERAL = "/run/hardened-blikvm-diagnostics/collector.sock"


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

    def test_diagnostic_export_is_typed_pure_no_std_and_runtime_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-export"
        collector = ROOT / "crates/bli-kvm-diagnostic-collector-policy"
        runtime_collector = ROOT / "crates/bli-kvm-diagnostic-collector"
        protocol = ROOT / "crates/bli-kvm-diagnostic-protocol"
        broker_protocol = ROOT / "crates/bli-kvm-broker-protocol"
        broker_client = ROOT / "crates/bli-kvm-broker-client"
        platform_broker = ROOT / "crates/bli-kvm-platform-broker"
        export_protocol = ROOT / "crates/bli-kvm-diagnostic-export-protocol"
        export_client = ROOT / "crates/bli-kvm-diagnostic-export-client"
        diagnostic_journal = ROOT / "crates/bli-kvm-diagnostic-journal"
        diagnostic_store = ROOT / "crates/bli-kvm-diagnostic-store"
        diagnostic_coordinator = ROOT / "crates/bli-kvm-diagnostic-coordinator"
        export_listener = ROOT / "crates/bli-kvm-diagnostic-export-listener"
        window = ROOT / "crates/bli-kvm-diagnostic-window-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-export"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                collector / "Cargo.toml",
                runtime_collector / "Cargo.toml",
                protocol / "Cargo.toml",
                broker_protocol / "Cargo.toml",
                broker_client / "Cargo.toml",
                platform_broker / "Cargo.toml",
                export_protocol / "Cargo.toml",
                export_client / "Cargo.toml",
                diagnostic_journal / "Cargo.toml",
                diagnostic_store / "Cargo.toml",
                diagnostic_coordinator / "Cargo.toml",
                export_listener / "Cargo.toml",
                window / "Cargo.toml",
            }:
                self.assertNotIn("bli-kvm-diagnostic-export", other_manifest.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub enum DiagnosticEvent", source)
        self.assertIn("MAX_RECORDS", source)
        self.assertIn("pub fn decode(bytes: &[u8])", source)
        self.assertIn("InvalidBundle", source)
        self.assertIn("fn parse_record", source)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "/dev/",
            "/var/",
            "tokio::",
            "libc::",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_export_protocol_is_fixed_instance_bound_and_unlinked(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-export-protocol"
        export_client = ROOT / "crates/bli-kvm-diagnostic-export-client"
        export_listener = ROOT / "crates/bli-kvm-diagnostic-export-listener"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-export-protocol"', workspace)
        self.assertIn(
            'bli-kvm-diagnostic-export = { path = "../bli-kvm-diagnostic-export" }',
            manifest,
        )
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                export_client / "Cargo.toml",
                export_listener / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-export-protocol", other_manifest.read_text()
                )
        for required in (
            "#![no_std]",
            "#![forbid(unsafe_code)]",
            "DIAGNOSTIC_EXPORT_REQUEST_BYTES: usize = 16",
            "DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES: usize = 24",
            "MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES",
            "instance_id: u64",
            "window_id: u32",
            "NonCanonicalReserved",
            "bundle: &'a DiagnosticBundle",
            "pub fn decode_request",
            "pub fn decode_response",
        ):
            self.assertIn(required, production)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "tokio::",
            "serde",
            "libc::",
            "unsafe {",
            "/dev/",
            "/var/",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_export_client_is_fixed_bounded_and_default_off_broker_only(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-export-client"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        platform_broker = ROOT / "crates/bli-kvm-platform-broker"
        platform_manifest = (platform_broker / "Cargo.toml").read_text()
        platform_source = (platform_broker / "src/server.rs").read_text()
        broker_protocol_manifest = (
            ROOT / "crates/bli-kvm-broker-protocol/Cargo.toml"
        ).read_text()
        broker_client_manifest = (
            ROOT / "crates/bli-kvm-broker-client/Cargo.toml"
        ).read_text()
        server_source = (ROOT / "crates/bli-kvm-server/src/lib.rs").read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-export-client"', workspace)
        self.assertIn("bli-kvm-diagnostic-export-protocol", manifest)
        self.assertIn("bli-kvm-diagnostic-export =", manifest)
        self.assertIn("tokio.workspace = true", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                platform_broker / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-export-client", other_manifest.read_text()
                )
        self.assertIn('default = []', platform_manifest)
        self.assertIn('diagnostic-export = [', platform_manifest)
        self.assertIn('"dep:bli-kvm-diagnostic-export-client"', platform_manifest)
        self.assertIn('"dep:bli-kvm-diagnostic-peer-policy"', platform_manifest)
        self.assertIn(
            'bli-kvm-diagnostic-export-client = { path = "../bli-kvm-diagnostic-export-client", optional = true }',
            platform_manifest,
        )
        self.assertIn('default = []', broker_protocol_manifest)
        self.assertIn('default = []', broker_client_manifest)
        for required in (
            "#![forbid(unsafe_code)]",
            '"/run/hardened-blikvm-diagnostics/export.sock"',
            "DIAGNOSTIC_EXPORT_CONNECT_TIMEOUT",
            "DIAGNOSTIC_EXPORT_WRITE_TIMEOUT",
            "DIAGNOSTIC_EXPORT_READ_TIMEOUT",
            "MAX_DIAGNOSTIC_EXPORT_PAYLOAD_BYTES",
            "pub enum DiagnosticExportReadError",
            "pub enum DiagnosticExportAcknowledgementError",
            "pub struct DiagnosticExportBinding",
            "pub struct DiagnosticSealedSnapshot",
            "pub struct DiagnosticExportClient",
            "pub async fn read_sealed",
            "pub async fn acknowledge",
            "UnixStream::connect",
            "timeout(limits.write_timeout",
            "timeout(limits.response_timeout",
            "decode_response(frame.as_bytes())",
            "DiagnosticBundle::decode(payload)",
            "DeliveryAmbiguous",
        ):
            self.assertIn(required, production)
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticExportClient",
        )
        self.assertNotIn("bli-kvm-diagnostic-export-client", builder)
        self.assertNotIn("diagnostic-export", builder)
        self.assertNotIn(".acknowledge(", platform_source)
        self.assertNotIn(".diagnostics()", server_source)
        for forbidden in (
            "pub fn with_parts",
            "pub fn with_timeout",
            "pub fn with_socket",
            "std::env",
            "Command",
            "std::fs",
            "Tcp",
            "serde",
            "tracing",
            "log::",
            "println!",
            "eprintln!",
            "Io(",
            "retry(",
            "sleep(",
            "/dev/",
            "/var/",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_export_listener_is_exact_bounded_and_collector_only(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-export-listener"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        collector = ROOT / "crates/bli-kvm-diagnostic-collector/Cargo.toml"
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-export-listener"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-export =",
            "bli-kvm-diagnostic-export-protocol =",
            "bli-kvm-diagnostic-peer-adapter =",
            "bli-kvm-diagnostic-window-policy =",
            "tokio.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {crate / "Cargo.toml", collector}:
                self.assertNotIn(
                    "bli-kvm-diagnostic-export-listener", other_manifest.read_text()
                )
        self.assertIn("bli-kvm-diagnostic-export-listener", collector.read_text())
        for required in (
            "DIAGNOSTIC_EXPORT_REQUEST_TIMEOUT",
            "DIAGNOSTIC_EXPORT_RESPONSE_TIMEOUT",
            "DIAGNOSTIC_EXPORT_MAX_CONNECTIONS: usize = 2",
            "DiagnosticExportConsumerAuthenticator::from_current_process()",
            ".authenticate(stream)",
            ".try_acquire_owned()",
            "DIAGNOSTIC_EXPORT_REQUEST_BYTES",
            "decode_request(&bytes)",
            "state.sealed_window().await",
            "state.acknowledge(instance_id, window_id).await",
            "stream.write_all(frame.as_bytes())",
            "JoinSet",
            ".compare_exchange",
        ):
            self.assertIn(required, production)
        self.assertLess(
            production.index("response_for_request(state.as_ref(), request).await"),
            production.index("stream.write_all(frame.as_bytes())"),
        )
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticExportListener",
        )
        self.assertNotIn("bli-kvm-diagnostic-export-listener", builder)
        for forbidden in (
            "std::env",
            "Command",
            "Path",
            "String",
            "Vec<",
            "Tcp",
            "serde",
            "tracing",
            "log::",
            "fs::",
            "UnixListener::bind",
            "UnixStream::connect",
            "remove_file",
            "println!",
            "eprintln!",
            "/dev/",
            "/run/",
            "/var/",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_journal_is_encrypted_bounded_pure_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-journal"
        store = ROOT / "crates/bli-kvm-diagnostic-store"
        key_loader = ROOT / "crates/bli-kvm-diagnostic-key-loader"
        coordinator = ROOT / "crates/bli-kvm-diagnostic-coordinator"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        design = (ROOT / "docs/design/diagnostic-journal-v1.md").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-journal"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-export =",
            "chacha20poly1305.workspace = true",
            "zeroize.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                store / "Cargo.toml",
                key_loader / "Cargo.toml",
                coordinator / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-journal", other_manifest.read_text()
                )
        for required in (
            "#![no_std]",
            "#![forbid(unsafe_code)]",
            "XChaCha20Poly1305",
            "AeadInOut",
            "DIAGNOSTIC_JOURNAL_KEY_BYTES: usize = 32",
            "DIAGNOSTIC_JOURNAL_NONCE_BYTES: usize = 24",
            "DIAGNOSTIC_JOURNAL_TAG_BYTES: usize = 16",
            "MAX_DIAGNOSTIC_JOURNAL_ENTRIES: usize = 2",
            'DIAGNOSTIC_JOURNAL_COPY_MAGIC: [u8; 8] = *b"BLKDJE01"',
            "DIAGNOSTIC_JOURNAL_COPY_BYTES",
            "pub struct DiagnosticJournalKey",
            "self.bytes.zeroize()",
            "pub struct DiagnosticJournalKeyRing",
            "pub struct SelectedDiagnosticJournal",
            "pub fn select_copies",
            "pub fn plan_initialize",
            "pub fn plan_update",
            "pub fn plan_reconcile",
            "pub fn plan_acknowledgement_result",
            "DiagnosticBundle::decode",
            "AcknowledgementAmbiguous",
            "ReconciliationAction::Acknowledge",
            "DiagnosticJournalEntryState::Orphaned",
        ):
            self.assertIn(required, production)
        for secret_type in ("DiagnosticJournalKey", "DiagnosticJournalKeyRing"):
            self.assertNotRegex(
                production,
                rf"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct {secret_type}",
            )
        self.assertNotIn("bli-kvm-diagnostic-journal", builder)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "std::fs",
            "tokio::",
            "serde",
            "getrandom",
            "OsRng",
            "SystemTime",
            "println!",
            "eprintln!",
            "/dev/",
            "/run/",
            "/var/",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)
        self.assertIn("XChaCha20Poly1305", design)
        self.assertIn("does not protect", design)
        self.assertIn("write-before-ack", design)

    def test_diagnostic_coordinator_is_floor_gated_linear_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-coordinator"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production, tests = source.split("#[cfg(test)]", 1)
        design = (
            ROOT / "docs/design/diagnostic-coordinator-v1.md"
        ).read_text()
        baseline = (
            ROOT / "docs/baselines/diagnostic-coordinator.md"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-coordinator"', workspace)
        self.assertIn("bli-kvm-diagnostic-journal =", manifest)
        self.assertIn("[dev-dependencies]", manifest)
        self.assertIn("bli-kvm-diagnostic-export =", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-diagnostic-coordinator", other_manifest.read_text()
                )
        for required in (
            "#![no_std]",
            "#![forbid(unsafe_code)]",
            "pub trait DiagnosticJournalRepository",
            "pub struct DiagnosticGenerationFloor(NonZeroU64)",
            "pub struct DiagnosticCoordinator",
            "pub fn observe",
            "pub fn prepare_acknowledgement",
            "pub fn recover_cancelled_acknowledgement",
            "pub struct DiagnosticAcknowledgementPermit",
            "impl<R: DiagnosticJournalRepository> Drop",
            "AcknowledgementRecoveryRequired",
            "AcknowledgementNotReady",
            "DiagnosticAcknowledgementCompletion::NotSent",
            "AcknowledgementResult::DeliveryAmbiguous",
            "selected.generation() > floor.get()",
            "selected.degraded()",
            "selected.journal() != mutation.journal()",
            "self.persist_verified(floor, mutation)",
        ):
            self.assertIn(required, production)
        for required_test in (
            "floor_must_catch_up_before_acknowledgement_permit",
            "dropped_permit_blocks_observation_until_ambiguity_is_persisted",
            "failure_after_first_copy_requires_explicit_repair",
            "failure_after_both_copies_is_discovered_without_duplicate_write",
            "failed_ack_persistence_arms_recovery_and_terminal_write_is_discovered",
            "bounded_repository_operations_have_no_internal_retry",
        ):
            self.assertIn(required_test, tests)
        self.assertNotIn("bli-kvm-diagnostic-coordinator", builder)
        for forbidden in (
            "std::",
            "tokio::",
            "async fn",
            "DiagnosticJournalStore",
            "DiagnosticExportClient",
            "DiagnosticJournalKey",
            "LoadCredential",
            "String",
            "Vec<",
            "Path",
            "Command",
            "serde",
            "tracing",
            "log::",
            "println!",
            "eprintln!",
            "/run/",
            "/var/",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)
        for required_design in (
            "External rollback floor",
            "linear acknowledgement permit",
            "never loops or retries",
            "full process crash",
            "dedicated Diagnostics tab",
        ):
            self.assertIn(required_design, design)
        for required_baseline in (
            "945eb19098639c733f3f630eb41750b7c1f0d936",
            "Twenty-six focused Rust tests",
            "document limit exceeded: 100/100",
            "7ef8812480c939dab37ac3efa479f651837cf86a7c9a3eb07185f86637b3362b",
            "6355407a9dfddb2b945e9cd4be064ded39f04fed886e88c0027e4f846ab87e5f",
            "737554ec4fdd4acd99836e8326a245e8",
            "deviceAccepted=false",
        ):
            self.assertIn(required_baseline, baseline)

    def test_diagnostic_store_is_fixed_atomic_keyless_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-store"
        maintenance = ROOT / "crates/bli-kvm-diagnostic-maintenance"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source
        tests = (crate / "src/tests.rs").read_text()
        design = (ROOT / "docs/design/diagnostic-store-v1.md").read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-store"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-journal =",
            "libc.workspace = true",
            "nix.workspace = true",
            "rand_core.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                maintenance / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-store", other_manifest.read_text()
                )
        for required in (
            '"/var/lib/hardened-blikvm-diagnostics"',
            '"journal-a.bin"',
            '"journal-b.bin"',
            'c".journal-a.tmp"',
            'c".journal-b.tmp"',
            '"hardened-blikvm-broker"',
            '"hardened-blikvm"',
            "User::from_name",
            "Group::from_name",
            "geteuid()",
            "getegid()",
            "libc::O_NOFOLLOW",
            "libc::O_NONBLOCK",
            "libc::openat",
            "libc::flock",
            "libc::renameat",
            "libc::unlinkat",
            "libc::fgetxattr",
            "sync_all()",
            "plan_initialize",
            "plan_update",
            "plan_repair",
            "select_copies",
            "verify_written",
            "pub fn load_read_only",
            "RecoveryRequired",
            "OsRng",
            "try_fill_bytes",
            "pub struct DiagnosticJournalStore",
        ):
            self.assertIn(required, production)
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticJournalStore",
        )
        self.assertNotIn("bli-kvm-diagnostic-store", builder)
        for forbidden in (
            "pub fn open_at",
            "pub fn with_root",
            "pub fn with_path",
            "std::env",
            "Command",
            "tokio::",
            "serde",
            "DiagnosticExportClient",
            ".acknowledge(",
            "BrokerReadClient",
            "Http",
            "Tcp",
            "println!",
            "eprintln!",
            "log::",
            "tracing",
            "kvmd-password",
            "LoadCredential",
        ):
            self.assertNotIn(forbidden, production)
        for required_test in (
            "every_durability_fault_point_on_both_copy_writes_is_recoverable",
            "active_previous_key_rotation_rewrites_both_copies_under_active_key",
            "unsafe_copy_types_modes_links_sizes_and_acls_are_rejected",
            "nonce_failure_reuse_wrong_key_floor_and_root_replacement_fail_closed",
            "read_only_load_never_cleans_a_temporary_or_repairs_a_degraded_copy",
        ):
            self.assertIn(required_test, tests)
        self.assertIn("/var/lib/hardened-blikvm-diagnostics", design)
        self.assertIn("write-readback-sync-ack", design)
        self.assertIn("does not retain", design)

    def test_diagnostic_key_loader_is_fixed_systemd_only_secret_safe_and_unlinked(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-key-loader"
        maintenance = ROOT / "crates/bli-kvm-diagnostic-maintenance"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        tests = (crate / "src/tests.rs").read_text()
        design = (
            ROOT / "docs/design/diagnostic-key-credential-v1.md"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-key-loader"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-journal =",
            "libc.workspace = true",
            "nix.workspace = true",
            "subtle.workspace = true",
            "zeroize.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                maintenance / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-key-loader", other_manifest.read_text()
                )
        for required in (
            '"diagnostic-journal-keyring"',
            '"CREDENTIALS_DIRECTORY"',
            '"/run/credentials/hardened-blikvm-diagnostic-store-init.service"',
            '"/run/credentials/hardened-blikvm-diagnostic-store-inspect.service"',
            'b"BLIDKEY\\0"',
            "DIAGNOSTIC_KEY_CREDENTIAL_BYTES: usize = 96",
            '"hardened-blikvm-broker"',
            '"hardened-blikvm"',
            "User::from_name",
            "Group::from_name",
            "geteuid()",
            "getegid()",
            "env::var_os",
            "libc::O_DIRECTORY",
            "libc::O_NOFOLLOW",
            "libc::O_NONBLOCK",
            "libc::openat",
            "libc::fgetxattr",
            "POSIX_ACL_VERSION",
            "ACL_USER_OBJ",
            "ACL_USER",
            "ACL_GROUP_OBJ",
            "ACL_MASK",
            "ACL_OTHER",
            "validate_systemd_acl",
            "ct_eq",
            "decode_credential_in_place",
            "bytes.zeroize()",
            "pub struct LoadedDiagnosticJournalKeys",
            "pub fn load_systemd_diagnostic_journal_keys",
            "pub fn key_ring",
        ):
            self.assertIn(required, production)
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct LoadedDiagnosticJournalKeys",
        )
        self.assertNotIn("bli-kvm-diagnostic-key-loader", builder)
        self.assertNotIn(
            '"/run/credentials/hardened-blikvm-platform-broker.service"',
            production,
        )
        for forbidden in (
            "pub fn load_from",
            "pub fn with_path",
            "pub fn parse",
            "std::env::args",
            "set_var",
            '"/etc/',
            "Command",
            "serde",
            "tokio::",
            "OsRng",
            "DiagnosticJournalStore",
            "DiagnosticExportClient",
            ".acknowledge(",
            "BrokerReadClient",
            "Http",
            "Tcp",
            "println!",
            "eprintln!",
            "log::",
            "tracing",
            "encode_credential",
        ):
            self.assertNotIn(forbidden, production)
        for required_test in (
            "credential_format_vector_is_stable",
            "systemd_249_acl_parser_accepts_only_the_exact_service_uid_grant",
            "active_only_credential_loads_and_raw_buffer_is_zeroized",
            "previous_key_rotation_is_typed_and_decrypts_the_old_generation",
            "format_rejects_every_noncanonical_header_and_key_relationship",
            "unsafe_file_type_mode_link_length_and_acl_are_rejected",
            "inode_replacement_and_same_inode_rewrite_are_concurrent_changes",
        ):
            self.assertIn(required_test, tests)
        self.assertIn("systemd 249", design)
        self.assertIn("96-byte", design)
        self.assertIn("does not protect the persistent source", design)
        self.assertIn("no public", design)

    def test_diagnostic_maintenance_is_empty_only_credential_gated_and_unshipped(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-maintenance"
        manifest = (crate / "Cargo.toml").read_text()
        library = (crate / "src/lib.rs").read_text()
        production = library.split("#[cfg(test)]", 1)[0]
        main = (crate / "src/main.rs").read_text()
        design = (
            ROOT / "docs/design/diagnostic-maintenance-v1.md"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-maintenance"', workspace)
        self.assertIn("bli-kvm-diagnostic-key-loader", manifest)
        self.assertIn("bli-kvm-diagnostic-store", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-diagnostic-maintenance", other_manifest.read_text()
                )
        for required in (
            '"initialize-empty"',
            '"inspect-empty"',
            "load_systemd_diagnostic_journal_keys",
            "DiagnosticJournalStore::open()",
            ".initialize(&self.keys.key_ring())",
            ".load_read_only(&self.keys.key_ring(), 1)",
            "entry_count: selected.journal().entry_count()",
            "pub struct ExactStoreBackend",
            "pub fn parse_arguments",
            "pub fn run_exact",
            "result=empty generation=1 copies=healthy",
        ):
            self.assertIn(required, production)
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct ExactStoreBackend",
        )
        for forbidden in (
            '"repair"',
            '"rotate-key"',
            '"acknowledge"',
            "std::process",
            "tokio::",
            "systemctl",
            "serde",
            "Tcp",
            "Http",
            "DiagnosticExportClient",
            ".repair(",
            ".update(",
            ".acknowledge(",
        ):
            self.assertNotIn(forbidden, production)
            self.assertNotIn(forbidden, main)

        init_unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-diagnostic-store-init.service"
        ).read_text()
        inspect_unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-diagnostic-store-inspect.service"
        ).read_text()
        credential = (
            "LoadCredential=diagnostic-journal-keyring:"
            "/etc/hardened-blikvm/diagnostic-journal-keyring"
        )
        for unit in (init_unit, inspect_unit):
            self.assertIn("User=hardened-blikvm-broker", unit)
            self.assertIn("Group=hardened-blikvm", unit)
            self.assertIn(credential, unit)
            self.assertIn("PrivateMounts=yes", unit)
            self.assertIn("PrivateNetwork=yes", unit)
            self.assertIn("PrivateDevices=yes", unit)
            self.assertIn("ProtectSystem=strict", unit)
            self.assertIn("RestrictAddressFamilies=AF_UNIX", unit)
            self.assertIn("CapabilityBoundingSet=\n", unit)
            self.assertIn("DevicePolicy=closed", unit)
            self.assertIn("IPAddressDeny=any", unit)
            self.assertNotIn("[Install]", unit)
            self.assertNotIn("systemctl", unit)
        self.assertIn(" initialize-empty", init_unit)
        self.assertIn(" inspect-empty", inspect_unit)
        self.assertIn(
            "ReadWritePaths=/var/lib/hardened-blikvm-diagnostics", init_unit
        )
        self.assertNotIn("ReadWritePaths=", inspect_unit)

        tmpfiles = (
            ROOT / "packaging/tmpfiles.d/hardened-blikvm-diagnostics.conf"
        ).read_text()
        self.assertEqual(
            tmpfiles,
            "# Type Path Mode User Group Age Argument\n"
            "d /var/lib/hardened-blikvm-diagnostics 0700 "
            "hardened-blikvm-broker hardened-blikvm - -\n",
        )
        self.assertIn("inspect-empty", design)
        self.assertIn("never removes", design)
        self.assertIn("no physical Power or", design)
        self.assertIn("Reset action is permitted", design)
        for forbidden in (
            "bli-kvm-diagnostic-maintenance",
            "hardened-blikvm-diagnostic-store-init",
            "hardened-blikvm-diagnostics.conf",
        ):
            self.assertNotIn(forbidden, builder)

    def test_platform_diagnostic_route_is_feature_gated_exact_peer_and_read_only(
        self,
    ) -> None:
        platform = ROOT / "crates/bli-kvm-platform-broker"
        source = (platform / "src/server.rs").read_text()
        protocol = (
            ROOT / "crates/bli-kvm-broker-protocol/src/lib.rs"
        ).read_text()
        client = (ROOT / "crates/bli-kvm-broker-client/src/lib.rs").read_text()
        server = (ROOT / "crates/bli-kvm-server/src/lib.rs").read_text()
        check = (ROOT / "scripts/check.sh").read_text()
        for required in (
            '#[cfg(feature = "diagnostic-export")]',
            '"hardened-blikvm-server"',
            '"hardened-blikvm"',
            "User::from_name",
            "Group::from_name",
            "stream.peer_cred()",
            "credentials.pid()",
            "credentials.uid()",
            "credentials.gid()",
            "DiagnosticPeerAssignments::new",
            "DiagnosticSource::PlatformBroker",
            "DiagnosticSource::Server",
            "DiagnosticExportClient::new()",
            "DiagnosticExportClient::read_sealed(self).await",
            "BrokerReadRequest::Diagnostics",
            "BrokerErrorCode::Unauthorized",
            "BrokerDiagnosticSnapshot::from_bundle",
        ):
            self.assertIn(required, source)
        for account in (
            "hardened-blikvm-diagnostics",
            "hardened-blikvm-server",
            "hardened-blikvm-broker",
            "hardened-blikvm-video",
            "hardened-blikvm-control",
            "hardened-blikvm-update",
        ):
            self.assertIn(account, source)
        self.assertIn('#[cfg(feature = "diagnostic-export")]', protocol)
        self.assertIn("BROKER_DIAGNOSTIC_SCHEMA_VERSION: u16 = 2", protocol)
        self.assertIn("Diagnostics", protocol)
        self.assertIn("DiagnosticBundle::decode", protocol)
        self.assertIn('#[cfg(feature = "diagnostic-export")]', client)
        self.assertIn("pub async fn diagnostics", client)
        self.assertNotIn(".acknowledge(", source)
        self.assertNotIn(".diagnostics()", server)
        self.assertNotIn('route("/diagnostics', server)
        self.assertNotIn("diagnostic-export", (ROOT / "scripts/build-prototype-bundle.sh").read_text())
        self.assertIn(
            "cargo check --workspace --all-targets --all-features --locked", check
        )
        for package in (
            "bli-kvm-broker-protocol",
            "bli-kvm-broker-client",
            "bli-kvm-platform-broker",
        ):
            self.assertIn(
                f"cargo test -p {package} --all-targets --locked --features diagnostic-export",
                check,
            )
            self.assertIn(
                f"cargo clippy -p {package} --all-targets --locked --features diagnostic-export",
                check,
            )

    def test_diagnostic_protocol_is_fixed_no_std_and_linked_only_to_control_broker(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-protocol"
        window = ROOT / "crates/bli-kvm-diagnostic-window-policy"
        adapter = ROOT / "crates/bli-kvm-diagnostic-peer-adapter"
        producer = ROOT / "crates/bli-kvm-diagnostic-producer-client"
        control_broker = ROOT / "crates/bli-kvm-control-broker"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-protocol"', workspace)
        self.assertIn(
            'bli-kvm-diagnostic-export = { path = "../bli-kvm-diagnostic-export" }',
            manifest,
        )
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                window / "Cargo.toml",
                adapter / "Cargo.toml",
                producer / "Cargo.toml",
                control_broker / "Cargo.toml",
            }:
                self.assertNotIn("bli-kvm-diagnostic-protocol", other_manifest.read_text())
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("DIAGNOSTIC_EVENT_FRAME_BYTES", source)
        self.assertIn("DIAGNOSTIC_RESPONSE_FRAME_BYTES", source)
        self.assertIn("pub fn encode_event", source)
        self.assertIn("pub fn decode_event", source)
        self.assertIn("pub fn encode_response", source)
        self.assertIn("pub fn decode_response", source)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "/dev/",
            "/var/",
            "tokio::",
            "libc::",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_producer_client_is_fixed_bounded_and_control_broker_only(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-producer-client"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        reporter_source = (crate / "src/reporter.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        reporter_production = reporter_source.split("#[cfg(test)]", 1)[0]
        control_broker = ROOT / "crates/bli-kvm-control-broker"
        control_manifest = (control_broker / "Cargo.toml").read_text()
        control_source = (control_broker / "src/server.rs").read_text()
        control_production = control_source.split("#[cfg(test)]", 1)[0]
        collector = (
            ROOT / "crates/bli-kvm-diagnostic-collector/src/lib.rs"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-producer-client"', workspace)
        self.assertIn("bli-kvm-diagnostic-protocol", manifest)
        self.assertIn("tokio.workspace = true", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                control_broker / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-producer-client = {",
                    other_manifest.read_text(),
                )
        for required in (
            "#![forbid(unsafe_code)]",
            '"/run/hardened-blikvm-diagnostics/collector.sock"',
            "pub enum DiagnosticNotSentReason",
            "pub enum DiagnosticAmbiguousStage",
            "pub enum DiagnosticSendError",
            "pub enum DiagnosticSendOutcome",
            "pub struct DiagnosticProducerClient",
            "pub fn new()",
            "pub async fn send",
            "state.take()",
            "timeout(write_timeout",
            "timeout(response_timeout",
            "decode_response(&response)",
        ):
            self.assertIn(required, production)
        self.assertIn(DIAGNOSTIC_SOCKET_LITERAL, production)
        self.assertIn(DIAGNOSTIC_SOCKET_LITERAL, collector)
        self.assertNotRegex(
            production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticProducerClient",
        )
        for forbidden in (
            "pub fn with_parts",
            "pub fn with_timeout",
            "pub fn with_socket",
            "std::env",
            "Command",
            "std::fs",
            "Tcp",
            "serde",
            "tracing",
            "log::",
            "println!",
            "eprintln!",
            "Io(",
            "/dev/",
            "/var/",
        ):
            self.assertNotIn(forbidden, production)
        for required in (
            "DIAGNOSTIC_REPORT_QUEUE_CAPACITY: usize = 8",
            "DIAGNOSTIC_REPORTER_SHUTDOWN_TIMEOUT",
            "pub enum DiagnosticReportDropReason",
            "pub enum DiagnosticReportReceipt",
            "pub struct DiagnosticReporterSnapshot",
            "pub struct DiagnosticReporterHandle",
            "pub struct DiagnosticReporter",
            "pub fn try_report",
            ".try_lock()",
            "[Option<DiagnosticEvent>; DIAGNOSTIC_REPORT_QUEUE_CAPACITY]",
            "runtime.spawn",
            "notify.notify_one()",
            "value.saturating_add(1)",
            "worker.abort()",
        ):
            self.assertIn(required, reporter_production)
        self.assertEqual(reporter_production.count("runtime.spawn"), 1)
        self.assertNotRegex(
            reporter_production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticReporter(?:\s|\{)",
        )
        for forbidden in (
            "tokio::sync::mpsc",
            "Vec<",
            "String",
            "HashMap",
            "Box<",
            "std::env",
            "Command",
            "std::fs",
            "println!",
            "eprintln!",
            "tracing",
            "log::",
            "/dev/",
            "/var/",
        ):
            self.assertNotIn(forbidden, reporter_production)
        self.assertNotIn("bli-kvm-diagnostic-producer-client", builder)
        self.assertIn("bli-kvm-diagnostic-producer-client", control_manifest)
        self.assertIn("bli-kvm-diagnostic-protocol", control_manifest)
        self.assertNotIn("thiserror", control_manifest)
        self.assertIn("DiagnosticReporter::start().ok()", control_production)
        self.assertIn("DiagnosticEvent::RequestRejected", control_production)
        self.assertIn("RejectReason::CapabilityDisabled", control_production)
        self.assertIn("let _ = diagnostics.try_report", control_production)
        self.assertLess(
            control_production.index("stream.write_all"),
            control_production.index("let _ = diagnostics.try_report"),
        )
        for forbidden in (".snapshot()", ".shutdown().await", "DiagnosticReporterSnapshot"):
            self.assertNotIn(forbidden, control_production)

    def test_diagnostic_collector_policy_has_fixed_pure_sources_and_is_unlinked(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-collector-policy"
        window = ROOT / "crates/bli-kvm-diagnostic-window-policy"
        peer = ROOT / "crates/bli-kvm-diagnostic-peer-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-collector-policy"', workspace)
        self.assertIn(
            'bli-kvm-diagnostic-export = { path = "../bli-kvm-diagnostic-export" }',
            manifest,
        )
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                window / "Cargo.toml",
                peer / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-collector-policy",
                    other_manifest.read_text(),
                )
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub enum DiagnosticSource", source)
        self.assertIn("pub struct SourceRecord", source)
        self.assertIn("fn event_allowed", source)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "/dev/",
            "/var/",
            "tokio::",
            "libc::",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_window_policy_is_pure_bounded_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-window-policy"
        adapter = ROOT / "crates/bli-kvm-diagnostic-peer-adapter"
        export_listener = ROOT / "crates/bli-kvm-diagnostic-export-listener"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-window-policy"', workspace)
        for dependency in (
            'bli-kvm-diagnostic-collector-policy = { path = "../bli-kvm-diagnostic-collector-policy" }',
            'bli-kvm-diagnostic-export = { path = "../bli-kvm-diagnostic-export" }',
            'bli-kvm-diagnostic-protocol = { path = "../bli-kvm-diagnostic-protocol" }',
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                adapter / "Cargo.toml",
                export_listener / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-window-policy",
                    other_manifest.read_text(),
                )
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub struct DiagnosticWindowManager", source)
        self.assertIn("pub struct DiagnosticSession", source)
        self.assertIn("pub struct DiagnosticWindowSnapshot", source)
        before_session = production.split("pub struct DiagnosticSession", 1)[0]
        session_derive = before_session.rsplit("#[derive(", 1)[-1].split(")]", 1)[0]
        self.assertNotIn("Debug", session_derive)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "/dev/",
            "/var/",
            "tokio::",
            "libc::",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_peer_policy_maps_only_kernel_credentials_and_default_off_broker(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-peer-policy"
        adapter = ROOT / "crates/bli-kvm-diagnostic-peer-adapter"
        platform_broker = ROOT / "crates/bli-kvm-platform-broker"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-peer-policy"', workspace)
        self.assertIn(
            'bli-kvm-diagnostic-collector-policy = { path = "../bli-kvm-diagnostic-collector-policy" }',
            manifest,
        )
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                adapter / "Cargo.toml",
                platform_broker / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-peer-policy",
                    other_manifest.read_text(),
                )
        self.assertIn("#![no_std]", source)
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("pub struct KernelPeerCredentials", source)
        self.assertIn("pub struct DiagnosticPeerAssignments", source)
        self.assertIn("pub const fn map_peer", source)
        for identity_type in (
            "KernelPeerCredentials",
            "DiagnosticPeerAssignments",
        ):
            before = production.split(f"pub struct {identity_type}", 1)[0]
            derive = before.rsplit("#[derive(", 1)[-1].split(")]", 1)[0]
            self.assertNotIn("Debug", derive)
        for forbidden in (
            "std::",
            "String",
            "Vec<",
            "Path",
            "Command",
            "/dev/",
            "/var/",
            "tokio::",
            "libc::",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)

    def test_diagnostic_peer_adapter_has_only_fixed_identity_and_unshipped_consumer(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-peer-adapter"
        collector = ROOT / "crates/bli-kvm-diagnostic-collector/Cargo.toml"
        export_listener = ROOT / "crates/bli-kvm-diagnostic-export-listener/Cargo.toml"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        listener_source = (crate / "src/listener.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        listener_production = listener_source.split("#[cfg(test)]", 1)[0]
        self.assertIn('"crates/bli-kvm-diagnostic-peer-adapter"', workspace)
        for dependency in (
            'bli-kvm-diagnostic-peer-policy = { path = "../bli-kvm-diagnostic-peer-policy" }',
            'bli-kvm-diagnostic-protocol = { path = "../bli-kvm-diagnostic-protocol" }',
            'bli-kvm-diagnostic-window-policy = { path = "../bli-kvm-diagnostic-window-policy" }',
            "nix.workspace = true",
            "rand_core.workspace = true",
            "tokio.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in (crate / "Cargo.toml", collector, export_listener):
                self.assertNotIn(
                    "bli-kvm-diagnostic-peer-adapter",
                    other_manifest.read_text(),
                )
        self.assertIn("bli-kvm-diagnostic-peer-adapter", collector.read_text())
        self.assertIn("bli-kvm-diagnostic-peer-adapter", export_listener.read_text())
        self.assertIn("#![forbid(unsafe_code)]", source)
        self.assertIn("User::from_name", source)
        self.assertIn("Group::from_name", source)
        self.assertIn(".peer_cred()", source)
        self.assertIn("pub struct AuthenticatedDiagnosticPeer", source)
        self.assertIn("pub struct AuthenticatedDiagnosticExportConsumer", source)
        self.assertIn("pub struct DiagnosticExportConsumerAuthenticator", source)
        self.assertIn("pub struct DiagnosticConnection", source)
        self.assertIn("pub struct DiagnosticRuntimeCore", source)
        self.assertIn("pub fn handle_frame", source)
        self.assertIn("pub struct DiagnosticCollectorRuntime", listener_source)
        self.assertIn("pub fn from_current_process", listener_source)
        self.assertIn("pub async fn serve_until", listener_source)
        self.assertNotRegex(
            listener_production,
            r"#\[derive\([^]]*(?:Debug|Clone)[^]]*\)\]\s*pub struct DiagnosticCollectorRuntime",
        )
        self.assertNotIn("pub fn with_parts", listener_production)
        self.assertNotIn("pub struct RuntimeLimits", listener_production)
        for required in (
            "UnixListener",
            "listener.accept()",
            "map_tokio_peer",
            "OsRng",
            ".try_fill_bytes",
            "Instant::now()",
            "Mutex<DiagnosticRuntimeCore>",
            "read_exact",
            "write_all",
            "timeout(",
            "watch::channel(false)",
            "JoinSet",
            ".compare_exchange",
            "MissedTickBehavior::Delay",
            "DIAGNOSTIC_PARTIAL_FRAME_TIMEOUT",
            "DIAGNOSTIC_RESPONSE_WRITE_TIMEOUT",
            "DIAGNOSTIC_WINDOW_TICK_INTERVAL",
        ):
            self.assertIn(required, listener_production)
        for handle_type in (
            "AuthenticatedDiagnosticPeer",
            "AuthenticatedDiagnosticExportConsumer",
            "DiagnosticConnection",
        ):
            before = production.split(f"pub struct {handle_type}", 1)[0]
            derive = before.rsplit("#[derive(", 1)[-1].split(")]", 1)[0]
            self.assertNotIn("Debug", derive)
            self.assertNotIn("Clone", derive)
        for account in (
            "hardened-blikvm-diagnostics",
            "hardened-blikvm-server",
            "hardened-blikvm-broker",
            "hardened-blikvm-video",
            "hardened-blikvm-control",
            "hardened-blikvm-update",
            "hardened-blikvm",
        ):
            self.assertIn(account, source)
        for forbidden in (
            "unsafe {",
            "libc::",
            "std::env",
            "Command",
            "Path",
            "/dev/",
            "/var/",
        ):
            self.assertNotIn(forbidden, production)
        for forbidden in (
            "unsafe {",
            "libc::",
            "std::env",
            "Command",
            "Path",
            "String",
            "Vec<",
            "format!",
            "SystemTime",
            "Tcp",
            "serde",
            "tracing",
            "log::",
            "fs::",
            "UnixListener::bind",
            "UnixStream::connect",
            "remove_file",
            "println!",
            "eprintln!",
            "/dev/",
            "/run/",
            "/var/",
        ):
            self.assertNotIn(forbidden, listener_production)

    def test_diagnostic_collector_is_fixed_unshipped_and_confined(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-collector"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        main = (crate / "src/main.rs").read_text()
        unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-diagnostic-collector.service"
        ).read_text()
        sysusers = (
            ROOT
            / "packaging/sysusers.d/hardened-blikvm-diagnostics.conf"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        verifier = (ROOT / "scripts/verify-systemd-units.sh").read_text()

        self.assertIn('"crates/bli-kvm-diagnostic-collector"', workspace)
        self.assertIn("bli-kvm-diagnostic-export-listener", manifest)
        self.assertIn("bli-kvm-diagnostic-peer-adapter", manifest)
        self.assertIn("nix.workspace = true", manifest)
        self.assertIn("signal-hook.workspace = true", manifest)
        self.assertIn("tokio.workspace = true", manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-diagnostic-collector = {",
                    other_manifest.read_text(),
                )
        for required in (
            "#![forbid(unsafe_code)]",
            '"/run/hardened-blikvm-diagnostics"',
            '"/run/hardened-blikvm-diagnostics/collector.sock"',
            '"/run/hardened-blikvm-diagnostics/export.sock"',
            "DiagnosticCollectorRuntime::from_current_process()",
            "DiagnosticExportListener::from_current_process",
            "bind_fixed_listeners()",
            "watch::channel(false)",
            "tokio::spawn",
            "tokio::join!",
            "UnixListener::bind",
            "Signals::new([SIGTERM, SIGINT])",
            ".name(\"diagnostic-signal\".into())",
            "signal_handle.close()",
            "DIAGNOSTIC_RUNTIME_DIRECTORY_MODE: u32 = 0o750",
            "DIAGNOSTIC_SOCKET_MODE: u32 = 0o660",
            "metadata.dev()",
            "metadata.ino()",
            "metadata.uid()",
            "metadata.gid()",
        ):
            self.assertIn(required, production)
        for forbidden in (
            "unsafe {",
            "std::env",
            "args_os",
            "Command",
            "Tcp",
            "/dev/",
            "/var/",
            "raw log",
            "read_to_",
            "serde",
            "tracing",
            "log::",
        ):
            self.assertNotIn(forbidden, production)
        self.assertNotIn("std::env", main)
        self.assertIn("Hardened BliKVM diagnostic collector failed", main)

        for directive in (
            "User=hardened-blikvm-diagnostics",
            "Group=hardened-blikvm",
            "DynamicUser=yes",
            "UMask=0117",
            "RuntimeDirectory=hardened-blikvm-diagnostics",
            "RuntimeDirectoryMode=0750",
            "RuntimeDirectoryPreserve=no",
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-collector",
            "StandardOutput=null",
            "NoNewPrivileges=yes",
            "PrivateDevices=yes",
            "PrivateNetwork=yes",
            "ProtectSystem=strict",
            "ReadWritePaths=/run/hardened-blikvm-diagnostics",
            "RestrictAddressFamilies=AF_UNIX",
            "CapabilityBoundingSet=",
            "DevicePolicy=closed",
            "IPAddressDeny=any",
        ):
            self.assertIn(directive, unit)
        self.assertNotIn("[Install]", unit)
        self.assertIn(
            'u hardened-blikvm-diagnostics - "Hardened BliKVM typed diagnostic collector" /nonexistent /usr/sbin/nologin',
            sysusers,
        )
        self.assertIn("m hardened-blikvm-diagnostics hardened-blikvm", sysusers)
        self.assertNotIn("bli-kvm-diagnostic-collector", builder)
        self.assertNotIn("hardened-blikvm-diagnostics.conf", builder)
        self.assertIn("bli-kvm-diagnostic-collector", verifier)
        self.assertIn("hardened-blikvm-diagnostics.conf", verifier)

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
        self.assertNotIn("hardened-blikvm-diagnostic-collector", unit)

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
