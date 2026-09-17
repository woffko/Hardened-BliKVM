from pathlib import Path
import re
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

    def test_st7789_model_is_pure_no_std_and_used_only_by_the_board_daemon(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-st7789-model"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-st7789-model"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        # The model stays pure: exactly one reviewed consumer, the board daemon
        # that owns the panel runtime, links it.
        consumers = [
            other_manifest
            for other_manifest in (ROOT / "crates").glob("*/Cargo.toml")
            if other_manifest != crate / "Cargo.toml"
            and "bli-kvm-st7789-model" in other_manifest.read_text()
        ]
        self.assertEqual(
            [consumer.parent.name for consumer in consumers],
            ["bli-kvm-boardd"],
        )
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

    def test_display_button_policy_is_pure_no_std_and_single_consumer(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-display-button-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-display-button-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        self._assert_only_board_daemon_consumes("bli-kvm-display-button-policy")
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

    def _assert_only_board_daemon_consumes(self, crate_name: str) -> None:
        crate = ROOT / "crates" / crate_name
        consumers = [
            manifest
            for manifest in (ROOT / "crates").glob("*/Cargo.toml")
            if manifest != crate / "Cargo.toml" and crate_name in manifest.read_text()
        ]
        self.assertEqual(
            [consumer.parent.name for consumer in consumers],
            ["bli-kvm-boardd"],
            f"{crate_name} must have exactly one reviewed consumer",
        )

    def test_activity_led_policy_is_pure_no_std_and_single_consumer(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-activity-led-policy"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-activity-led-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        self._assert_only_board_daemon_consumes("bli-kvm-activity-led-policy")
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
        diagnostic_persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
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
                diagnostic_persistence / "Cargo.toml",
                persistence_gate / "Cargo.toml",
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
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
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
                persistence_gate / "Cargo.toml",
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

    def test_diagnostic_export_client_is_fixed_bounded_and_default_off_consumers_only(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-export-client"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production = source.split("#[cfg(test)]", 1)[0]
        platform_broker = ROOT / "crates/bli-kvm-platform-broker"
        persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
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
                persistence / "Cargo.toml",
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
        persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
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
                persistence / "Cargo.toml",
                persistence_gate / "Cargo.toml",
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
        persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                persistence / "Cargo.toml",
                persistence_gate / "Cargo.toml",
            }:
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

    def test_diagnostic_persistence_is_fixed_one_step_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-persistence"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        production, tests = source.split("#[cfg(test)]", 1)
        design = (
            ROOT / "docs/design/diagnostic-persistence-v1.md"
        ).read_text()
        baseline = (
            ROOT / "docs/baselines/diagnostic-persistence.md"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
        self.assertIn('"crates/bli-kvm-diagnostic-persistence"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-coordinator =",
            "bli-kvm-diagnostic-export =",
            "bli-kvm-diagnostic-export-client =",
            "bli-kvm-diagnostic-journal =",
            "bli-kvm-diagnostic-key-loader =",
            "bli-kvm-diagnostic-store =",
        ):
            self.assertIn(dependency, manifest)
        self.assertNotIn("tokio.workspace = true", manifest.split("[dev-dependencies]", 1)[0])
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in {
                crate / "Cargo.toml",
                persistence_gate / "Cargo.toml",
            }:
                self.assertNotIn(
                    "bli-kvm-diagnostic-persistence", other_manifest.read_text()
                )
        for required in (
            "#![forbid(unsafe_code)]",
            "pub enum DiagnosticPersistenceStep",
            "pub enum DiagnosticPersistenceError",
            "pub struct DiagnosticPersistenceRuntime",
            "pub fn new(",
            "keys: LoadedDiagnosticJournalKeys",
            "store: DiagnosticJournalStore",
            "DiagnosticExportClient::new()",
            "pub async fn poll",
            ".load_read_only(&self.keys.key_ring(), minimum_generation)",
            ".update(&self.keys.key_ring(), minimum_generation, mutation)",
            ".recover_cancelled_acknowledgement(floor)",
            ".prepare_acknowledgement(floor)",
            "transport.acknowledge_once(binding).await",
            "DiagnosticAcknowledgementCompletion::DeliveryAmbiguous",
            "DiagnosticAcknowledgementCompletion::CollectorUnavailable",
            "DiagnosticPersistenceStep::AcknowledgementNotSent",
            "#[allow(clippy::large_enum_variant)]",
        ):
            self.assertIn(required, production)
        for required_test in (
            "new_snapshot_is_persisted_without_ack_until_floor_advances",
            "read_cancellation_is_nonmutating_and_needs_an_explicit_new_poll",
            "cancellation_during_ack_blocks_read_until_ambiguity_is_durable",
            "ack_failure_after_first_copy_requires_repair_and_keeps_marker",
            "ack_failure_after_both_copies_discovers_terminal_without_read",
            "export_client_outcomes_map_exhaustively_without_retry",
            "poll_future_size_is_bounded_for_fake_and_exact_transport",
        ):
            self.assertIn(required_test, tests)
        self.assertNotIn("bli-kvm-diagnostic-persistence", builder)
        for forbidden in (
            "load_systemd_diagnostic_journal_keys",
            "DiagnosticJournalStore::open",
            "std::path",
            "std::env",
            "PathBuf",
            "Command",
            "tokio::spawn",
            "spawn(",
            "loop {",
            "retry(",
            "serde",
            "tracing",
            "log::",
            "println!",
            "eprintln!",
            "Tcp",
            "Http",
            "/run/",
            "/var/",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)
        for required_design in (
            "fixed-boundary objects",
            "One-step runtime",
            "Async cancellation",
            "no trusted production floor",
            "dedicated Diagnostics tab",
        ):
            self.assertIn(required_design, design)
        for required_baseline in (
            "57959b04010cc6b4bcf01ea1fb861538b63aa4c8",
            "Nineteen focused Rust tests",
            "document limit exceeded: 100/100",
            "06164420cfaad73eebc854751e3830386f32c48b34275c2d3a91654b0815922f",
            "32bcbf8df703cc9ed0efb44942ca35eb21b346fcd407524e387d9e4680e2316c",
            "d7bb40e780d94af79f258009fee3e1ad",
            "deviceAccepted=false",
        ):
            self.assertIn(required_baseline, baseline)

    def test_diagnostic_persistence_gate_is_synthetic_confined_and_unshipped(
        self,
    ) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/main.rs").read_text()
        production, tests = source.split("#[cfg(test)]", 1)
        unit = (
            ROOT
            / "packaging/systemd/hardened-blikvm-diagnostic-persistence-gate.service"
        ).read_text()
        design = (
            ROOT / "docs/design/diagnostic-persistence-gate-v1.md"
        ).read_text()
        baseline = (
            ROOT / "docs/baselines/device-diagnostic-persistence.md"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        verifier = (ROOT / "scripts/verify-systemd-units.sh").read_text()
        self.assertIn('"crates/bli-kvm-diagnostic-persistence-gate"', workspace)
        for dependency in (
            "bli-kvm-diagnostic-coordinator =",
            "bli-kvm-diagnostic-export =",
            "bli-kvm-diagnostic-export-protocol =",
            "bli-kvm-diagnostic-journal =",
            "bli-kvm-diagnostic-key-loader =",
            "bli-kvm-diagnostic-persistence =",
            "bli-kvm-diagnostic-store =",
            "tokio.workspace = true",
        ):
            self.assertIn(dependency, manifest)
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest != crate / "Cargo.toml":
                self.assertNotIn(
                    "bli-kvm-diagnostic-persistence-gate",
                    other_manifest.read_text(),
                )
        for required in (
            "#![forbid(unsafe_code)]",
            'const EXPORT_SOCKET: &str = "/run/hardened-blikvm-diagnostics/export.sock"',
            'const RESULT_FILE: &str = "/run/hardened-blikvm-diagnostics/gate.result"',
            "load_systemd_diagnostic_journal_keys()",
            "DiagnosticJournalStore::open()",
            ".initialize(&keys.key_ring())",
            "DiagnosticPersistenceRuntime::new(keys, store)",
            "stall_started.notified()",
            "EXPECTED_READ_REQUESTS: u32 = 7",
            "EXPECTED_ACKNOWLEDGEMENT_REQUESTS: u32 = 3",
            '"automatic_retries=0\\n"',
            "selected.journal().entry_count() != 2",
            "DiagnosticJournalEntryState::Acknowledged",
        ):
            self.assertIn(required, production)
        for required_test in (
            "scripted_collector_sequence_is_exact_and_typed",
            "mismatch_preserves_the_active_window",
            "receipt_and_expected_counts_are_fixed",
            "fixed_paths_and_bindings_are_nonempty",
        ):
            self.assertIn(required_test, tests)
        for forbidden in (
            "std::env",
            "args(",
            "clap",
            "serde",
            "Command",
            "Tcp",
            "Http",
            "println!",
            "eprintln!",
            "systemctl",
            "reboot",
            "Power",
            "Reset",
            "unsafe {",
        ):
            self.assertNotIn(forbidden, production)
        for required in (
            "Type=oneshot",
            "User=hardened-blikvm-broker",
            "Group=hardened-blikvm",
            "RuntimeDirectory=hardened-blikvm-diagnostics",
            "RuntimeDirectoryPreserve=yes",
            "LoadCredential=diagnostic-journal-keyring:",
            "ExecStart=/usr/libexec/hardened-blikvm/bli-kvm-diagnostic-persistence-gate",
            "PrivateNetwork=yes",
            "PrivateDevices=yes",
            "ProtectSystem=strict",
            "RestrictAddressFamilies=AF_UNIX",
            "CapabilityBoundingSet=\n",
            "DevicePolicy=closed",
            "IPAddressDeny=any",
            "ReadWritePaths=/var/lib/hardened-blikvm-diagnostics /run/hardened-blikvm-diagnostics",
        ):
            self.assertIn(required, unit)
        self.assertNotIn("[Install]", unit)
        self.assertNotIn("bli-kvm-diagnostic-persistence-gate", builder)
        self.assertNotIn("hardened-blikvm-diagnostic-persistence-gate.service", builder)
        self.assertIn("hardened-blikvm-diagnostic-persistence-gate.service", verifier)
        for required in (
            "Starting from a freshly initialized encrypted generation one",
            "seven reads and three acknowledgement requests",
            "171-byte semantic result",
            "ten-minute systemd watchdog",
            "Power/Reset",
            "dedicated Diagnostics tab",
        ):
            self.assertIn(required, design)
        for required in (
            "7bef2bc827c4d660b00a9700e16cef39a58cde86",
            "e55709c19a30438fb9f66d047e946a9c",
            "e02729175d8a8f76acd3e2592755b74733dba2d74d039174f2f80713512288a3",
            "f9d5b603ab1499921a356a2be0d9366f3741217e19e87b9005babf108c1d2b22",
            "9b7745572aaa41c9b9fbc4177ada3f99",
            "7420391703f7416ca71c5a3aca25b599",
            "seven reads and three acknowledgement requests",
            "Power/Reset, reboot",
        ):
            self.assertIn(required, baseline)

    def test_diagnostic_store_is_fixed_atomic_keyless_and_unlinked(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-diagnostic-store"
        maintenance = ROOT / "crates/bli-kvm-diagnostic-maintenance"
        persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
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
                persistence / "Cargo.toml",
                persistence_gate / "Cargo.toml",
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
        persistence = ROOT / "crates/bli-kvm-diagnostic-persistence"
        persistence_gate = ROOT / "crates/bli-kvm-diagnostic-persistence-gate"
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
                persistence / "Cargo.toml",
                persistence_gate / "Cargo.toml",
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
        self.assertIn('route("/diagnostics", get(diagnostics_view))', server)
        self.assertIn("DiagnosticsView::from_inventory(&inventory)", server)
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

    def test_system_action_policy_is_pure_and_has_reviewed_consumers(self) -> None:
        workspace = (ROOT / "Cargo.toml").read_text()
        crate = ROOT / "crates/bli-kvm-system-action-policy"
        journal = ROOT / "crates/bli-kvm-system-action-journal/Cargo.toml"
        manifest = (crate / "Cargo.toml").read_text()
        source = (crate / "src/lib.rs").read_text()
        self.assertIn('"crates/bli-kvm-system-action-policy"', workspace)
        self.assertNotIn("[dependencies]", manifest)
        # The policy stays pure and has exactly two reviewed consumers: the
        # journal that persists it and the broker profile that arms it.
        consumers = sorted(
            manifest.parent.name
            for manifest in (ROOT / "crates").glob("*/Cargo.toml")
            if manifest not in (crate / "Cargo.toml", journal)
            and "bli-kvm-system-action-policy" in manifest.read_text()
        )
        # The maintenance binary is the third reviewed consumer: the boot
        # acceptance step restores the same pure engine to close the record
        # the owner persisted. It is static, confined and unshipped.
        self.assertEqual(
            consumers, ["bli-kvm-control-broker", "bli-kvm-system-action-maintenance"]
        )
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
        # The root-only store has exactly two reviewed consumers: the static
        # maintenance binary, and the board owner's system-action profile. The
        # owner runs as root and is the component that must persist a prepared
        # action across a restart; the dependency stays optional so a build
        # without that profile cannot link the store at all.
        broker = ROOT / "crates/bli-kvm-control-broker/Cargo.toml"
        for other_manifest in (ROOT / "crates").glob("*/Cargo.toml"):
            if other_manifest not in (crate / "Cargo.toml", maintenance, broker):
                self.assertNotIn("bli-kvm-system-action-store", other_manifest.read_text())
        self.assertIn("bli-kvm-system-action-store", maintenance.read_text())
        broker_manifest = broker.read_text()
        self.assertIn(
            'bli-kvm-system-action-store = { path = "../bli-kvm-system-action-store", optional = true }',
            broker_manifest,
        )
        self.assertIn(
            'system-action-profile = ["dep:bli-kvm-system-action-policy", "dep:bli-kvm-system-action-store"]',
            broker_manifest,
        )
        broker_sources = [
            path
            for path in (ROOT / "crates/bli-kvm-control-broker/src").glob("*.rs")
            if "bli_kvm_system_action_store" in path.read_text()
        ]
        self.assertEqual(
            sorted(path.name for path in broker_sources), ["server.rs", "system_action.rs"]
        )
        self.assertIn(
            "SystemActionStore::open()",
            (ROOT / "crates/bli-kvm-control-broker/src/server.rs").read_text(),
        )
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

    def test_the_board_owner_allows_exactly_the_network_write_surface(self) -> None:
        unit = (
            ROOT / "packaging/systemd/hardened-blikvm-board-owner.service"
        ).read_text()
        self.assertIn(
            "ReadWritePaths=/sys/kernel/config/usb_gadget /run/hardened-blikvm-gadget "
            "/var/lib/hardened-blikvm-gadget /etc/resolv.conf",
            unit,
        )
        self.assertIn("CapabilityBoundingSet=CAP_NET_ADMIN", unit)
        self.assertNotIn("CAP_SYS_ADMIN", unit)
        self.assertNotIn("AmbientCapabilities=CAP", unit)

    def test_the_board_profile_takes_precedence_over_the_atx_only_entry_point(self) -> None:
        # The ATX-only entry point binds the same control socket without the
        # board state, so a build that runs the board profile must never take
        # it: the board profile serves the ATX lines itself.
        main = (ROOT / "crates/bli-kvm-control-broker/src/main.rs").read_text()
        atx_only = main.index('std::env::var("BLI_KVM_GPIO_ATX_PROFILE")')
        board_profile = main.index('std::env::var("BLI_KVM_BOARD_PROFILE")')
        self.assertLess(board_profile, atx_only)
        guard = main[board_profile:atx_only]
        self.assertIn('!= Ok("1")', guard)

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
        self.assertIn('"accept-post-boot"', production_library)
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
        accept_unit = (
            ROOT / "packaging/systemd/hardened-blikvm-system-action-accept.service"
        ).read_text()
        self.assertIn("User=root", accept_unit)
        self.assertIn("PrivateNetwork=yes", accept_unit)
        self.assertIn("PrivateDevices=yes", accept_unit)
        self.assertIn("ProtectSystem=strict", accept_unit)
        self.assertIn("ReadWritePaths=/var/lib/hardened-blikvm-system-actions", accept_unit)
        self.assertNotIn("[Install]", accept_unit)
        self.assertIn(
            "bli-kvm-system-action-maintenance accept-post-boot --pending --guards-ready --require-accepted",
            accept_unit,
        )
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertNotIn("bli-kvm-system-action-maintenance", builder)
        self.assertNotIn("hardened-blikvm-system-action-store-init", builder)
        self.assertNotIn("hardened-blikvm-system-action-accept", builder)
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

    def test_diagnostics_view_is_authenticated_current_only_and_store_free(self) -> None:
        core = (ROOT / "crates/bli-kvm-core/src/diagnostics.rs").read_text()
        core_production, core_tests = core.split("#[cfg(test)]", 1)
        server = (ROOT / "crates/bli-kvm-server/src/lib.rs").read_text()
        server_production, server_tests = server.split("#[cfg(test)]", 1)
        server_manifest = (ROOT / "crates/bli-kvm-server/Cargo.toml").read_text()
        api = (ROOT / "docs/api/diagnostics-v1.md").read_text()
        baseline = (ROOT / "docs/baselines/device-diagnostics-api.md").read_text()
        web_type = (ROOT / "web/src/types/diagnostics.ts").read_text()
        web_api = (ROOT / "web/src/api/diagnostics.ts").read_text()
        device_gate = (ROOT / "tools/stage_device_systemd_gate.py").read_text()
        device_gate_tests = (
            ROOT / "tools/test_stage_device_systemd_gate.py"
        ).read_text()
        builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()

        for required in (
            "DIAGNOSTICS_VIEW_SCHEMA_VERSION: u16 = 1",
            "pub struct DiagnosticsView",
            "pub struct DiagnosticsCurrent",
            "pub struct DiagnosticComponents",
            "pub enum DiagnosticBooleanState",
            "pub enum DiagnosticMetricState",
            "PersistenceNotProvisioned",
            "pub fn from_inventory",
            ".validate()",
            "pub fn validate",
        ):
            self.assertIn(required, core_production)
        for forbidden in (
            "DiagnosticBundle",
            "DiagnosticBinding",
            "instance_id",
            "window_id",
            "Path",
            "Command",
            "tokio::",
            "std::fs",
            "raw_log",
            "credential",
        ):
            self.assertNotIn(forbidden, core_production)
        for required_test in (
            "sanitized_inventory_maps_to_current_only_view",
            "absent_temperature_and_rtc_are_explicitly_unavailable",
            "serialized_view_excludes_identity_version_log_and_persistence_payloads",
            "decoding_rejects_unknown_fields_and_invalid_metric_coherence",
        ):
            self.assertIn(required_test, core_tests)

        protected = server_production.index("let protected_routes")
        route = server_production.index('.route("/diagnostics", get(diagnostics_view))')
        layer = server_production.index(".route_layer(", route)
        self.assertLess(protected, route)
        self.assertLess(route, layer)
        self.assertIn("DiagnosticsView::from_inventory(&inventory)", server_production)
        self.assertIn(".inventory()", server_production)
        for forbidden in (
            "bli-kvm-diagnostic-persistence",
            "bli-kvm-diagnostic-key-loader",
            "bli-kvm-diagnostic-store",
            "bli-kvm-diagnostic-export-client",
        ):
            self.assertNotIn(forbidden, server_manifest)
        for required_test in (
            "diagnostics_require_an_authenticated_session",
            "authenticated_diagnostics_are_current_only_typed_and_private",
            "diagnostics_are_get_only_and_backend_errors_are_sanitized",
        ):
            self.assertIn(required_test, server_tests)

        self.assertIn("schemaVersion: 1", web_type)
        self.assertIn("availability: 'current_only'", web_type)
        self.assertIn("reason: 'persistence_not_provisioned'", web_type)
        self.assertIn("http.request<DiagnosticsView>", web_api)
        self.assertIn("method: 'get'", web_api)
        self.assertIn("url: '/api/diagnostics'", web_api)
        self.assertIn("signal", web_api)
        self.assertNotIn("http.post", web_api)
        self.assertIn("GET /api/diagnostics", api)
        self.assertIn("opaque HttpOnly session", api)
        self.assertIn("persistence_not_provisioned", api)
        self.assertIn("does not read the unshipped persistence stack", api)
        for required in (
            "phase=diagnostics-unauthorized",
            '"availability":"current_only"',
            "persistence_not_provisioned",
            "diagnostics_temperature_bounded=yes",
            "diagnostics_rtc_bounded=yes",
            "diagnostics_component_states=typed",
        ):
            self.assertIn(required, device_gate)
            self.assertIn(required, device_gate_tests)
        self.assertIn("docs/api/diagnostics-v1.md", builder)
        for required in (
            "f0973ce30ce485ca8a4d7cbddae7047498acb76a",
            "9ce28d88917246d28003d6cba2156a6a",
            "0372390059283922e95e4d7163aa83fe60a7be2af764f89b316f1d3288802209",
            "5b9d0904308175fe62c805d3b8dfd0792a58d6b21654ce27996643b22ad729be",
            "8e76c6656168d01106cae84d506db67c3a23141ac0326658f67af71b165ae094",
            "8c5ee6fe988f40edaafd9165da90bfdf",
            "de66f771b2bc48479c711aca37312358",
            "remoteCleanupConfirmed=true",
            "persistence_not_provisioned",
            "Power/Reset, reboot",
        ):
            self.assertIn(required, baseline)

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

    def test_diagnostics_tab_is_localized_bounded_and_action_free(self) -> None:
        settings = (
            ROOT / "web/src/pages/desktop/menu/settings/index.tsx"
        ).read_text()
        page = (
            ROOT
            / "web/src/pages/desktop/menu/settings/diagnostics/index.tsx"
        ).read_text()
        hook = (ROOT / "web/src/hooks/useDiagnostics.ts").read_text()
        validator = (ROOT / "web/src/lib/diagnostics.ts").read_text()
        api = (ROOT / "web/src/api/diagnostics.ts").read_text()
        capabilities = (ROOT / "web/src/lib/capabilities.ts").read_text()
        locales = (ROOT / "web/src/i18n/diagnostics-locales.ts").read_text()
        locale_extras = (ROOT / "web/src/i18n/locale-extras.ts").read_text()
        baseline = (ROOT / "docs/baselines/device-diagnostics-ui.md").read_text()

        self.assertIn("id: 'diagnostics'", settings)
        self.assertIn("component: <Diagnostics />", settings)
        self.assertIn("| 'diagnostics'", capabilities)
        self.assertRegex(
            capabilities,
            r"case 'about':\s+case 'diagnostics':\s+case 'appearance':",
        )
        for required in (
            "useDiagnostics()",
            "settings.diagnostics.title",
            "settings.diagnostics.temperature",
            "settings.diagnostics.externalRtc",
            "settings.diagnostics.components",
            "settings.diagnostics.historyNotProvisioned",
            "primaryComponentState",
            "grid-cols-1 gap-3 sm:grid-cols-2",
        ):
            self.assertIn(required, page)
        for forbidden in (
            "getRuntimeInventory",
            "system-log",
            "rawLog",
            "bundle",
            "binding",
            "instanceId",
            "windowId",
            "PowerIcon",
            "Reset",
            "reboot",
            "shutdown",
        ):
            self.assertNotIn(forbidden, page)

        for required in (
            "DIAGNOSTICS_REFRESH_MS = 10_000",
            "new AbortController()",
            "controllerRef.current",
            "document.visibilityState",
            "visibilitychange",
            "window.setTimeout",
            "window.clearTimeout",
            "controllerRef.current?.abort()",
            "setRefreshGeneration((generation) => generation + 1)",
            "if (active && scheduleNext)",
            "active = false",
            "current.data ? 'stale' : 'error'",
            "isDiagnosticsView(response.data)",
        ):
            self.assertIn(required, hook)
        self.assertNotIn("setInterval", hook)
        self.assertNotIn("getRuntimeInventory", hook)
        self.assertIn("method: 'get'", api)
        self.assertIn("signal", api)
        self.assertNotIn("method: 'post'", api)

        for required in (
            "hasExactKeys",
            "millidegreesCelsius, -40_000, 150_000",
            "unixSeconds, 946_684_800, 4_102_444_799",
            "offsetFromSystemSeconds, -315_576_000, 315_576_000",
            "persistence_not_provisioned",
            "cz: 'cs'",
            "pt_br: 'pt-BR'",
            "se: 'sv'",
            "zh_tw: 'zh-TW'",
            "new Intl.DateTimeFormat(locale",
            "toISOString().slice(11, 19) + ' UTC'",
        ):
            self.assertIn(required, validator)
        canonical_locales = (
            "ca",
            "cz",
            "da",
            "de",
            "en",
            "es",
            "et",
            "fi",
            "fr",
            "hu",
            "id",
            "it",
            "ja",
            "ko",
            "nb",
            "nl",
            "pl",
            "pt_br",
            "ru",
            "se",
            "th",
            "tr",
            "uk",
            "vi",
            "zh",
            "zh_tw",
        )
        declared_locales_match = re.search(
            r"export const DIAGNOSTICS_LOCALES = \[(.*?)\] as const;",
            locales,
            re.DOTALL,
        )
        if declared_locales_match is None:
            self.fail("diagnostics locale declaration is missing")
        declared_locales = re.findall(r"'([^']+)'", declared_locales_match.group(1))
        self.assertEqual(declared_locales, list(canonical_locales))
        for locale in canonical_locales:
            self.assertIn(f"  {locale}: [", locales)
        self.assertEqual(locales.count("'settings.diagnostics."), 23)
        self.assertIn("type DiagnosticsPack = readonly [", locales)
        self.assertEqual(locales.count("  string,"), 22)
        self.assertIn("diagnosticsLocaleExtras(key)", locale_extras)
        self.assertIn("no: 'nb'", locale_extras)
        self.assertIn("sv: 'se'", locale_extras)
        for required in (
            "8339018c74bdc4edd9889a418d7fdbce633cb769",
            "e8c485567f6040079ae0ee5b6257a0f6",
            "756ca101b9f24127a29b0a7c3955ec45b2ec7d56",
            "da1368ab1e484a3d9ea8d9475f5e27bc",
            "f81d30996d7449d689966aea44ecb735",
            "3311d78ffb304836904d74aa3a4e0f88",
            "9f13b56432dc0a73e700e881da23d4f31f57400b0cf69b6fd73dbf42fcd7f2e9",
            "d9d6e3667e4d4ea8ab0b9ef3aa449f3d",
            "e69c92dc79994a418b4be8baf59cfc12",
            "diagnosticsMobileUi",
            "diagnosticsStaleUi",
            "no Power/Reset",
        ):
            self.assertIn(required, baseline)

    def test_debian_runtime_package_is_inert_private_and_evidenced(self) -> None:
        builder = (ROOT / "tools/build_debian_runtime_package.py").read_text()
        gate = (ROOT / "tools/stage_device_debian_runtime_package.py").read_text()
        remap = (ROOT / "scripts/rustc-remap-wrapper.sh").read_text()
        verifier = (ROOT / "scripts/verify-aarch64-elf.sh").read_text()
        baseline = (ROOT / "docs/baselines/device-debian-runtime-package.md").read_text()

        for required in (
            'PACKAGE_NAME = "hardened-blikvm-runtime"',
            '"autoStart": False',
            '"unitTemplatesActive": False',
            '"accountsProvisioned": False',
            '"credentialsIncluded": False',
            '"nginxConfigActive": False',
            '"updateRuntimeIncluded": False',
            '"diagnosticPersistenceIncluded": False',
            '"hardwareMutationIncluded": False',
            "clean_builder_commit()",
            "audit_package_tree(package_root)",
        ):
            self.assertIn(required, builder)
        for forbidden in (
            '"usr/lib/sysusers.d/',
            '"etc/nginx/',
            '"etc/hardened-blikvm/',
        ):
            self.assertNotIn(forbidden, builder)

        self.assertIn("--remap-path-prefix=", remap)
        self.assertIn("/usr/src/hardened-blikvm", remap)
        self.assertIn("host-specific build path entered ELF", verifier)
        for required in (
            "dpkg -i",
            "dpkg --purge",
            "sleep 300",
            "installed_hashes_verified",
            "account_databases_unchanged",
            "nginx_tree_unchanged",
            '"dpkgAuditTrailRetained": True',
            '"physicalActionsInvoked": False',
        ):
            self.assertIn(required, gate)
        for forbidden in (
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemd-sysusers",
            "systemd-tmpfiles",
        ):
            self.assertNotIn(forbidden, gate)

        for required in (
            "6b1add8dfe1abf01cf62d606b712e4f7e77a3ec1",
            "d3e9f393f9714300b33a50fdc0769153",
            "fc544c445d944df7ac095d9afa416b42",
            "7fbe7d27828f929ba614214860d015b2fb8bed316ea7274828fe1710c88dea21",
            "c0edad8fc26ccbc3199e5714e92860c39c15fcb0",
            "53a4d4bfc062057954634e0421a2ec7801e917a488d22c957cd8244d36dba6a5",
            "20c5ec588bb94ddcb6250d926ea11260",
            "aa12cc4ee7107a73ad4ce45b154a0bd02b6b77c7",
            "f95700b0a866428499672ff68b937248",
            "eeef03b316c14f608c001dc9cb9c27ad",
            "d8b9e7a0bb6f483eb2b6d733e611fab4",
            "db45b25705f2423abd0ed52579d582b2",
            "package and normal dpkg audit history are the only intentional retained state",
            "dpkg audit-log entries",
        ):
            self.assertIn(required, baseline)

    def test_retained_debian_package_gate_is_separate_crash_safe_and_inert(
        self,
    ) -> None:
        ephemeral = (ROOT / "tools/stage_device_debian_runtime_package.py").read_text()
        retained = (ROOT / "tools/stage_device_debian_runtime_retain.py").read_text()
        preflight = (
            ROOT / "tools/capture_debian_runtime_retain_preflight.py"
        ).read_text()
        inspector = (ROOT / "tools/capture_debian_runtime_retained.py").read_text()
        checks = (ROOT / "scripts/check.sh").read_text()

        self.assertNotIn("packageRetained", ephemeral)
        for required in (
            "package_gate.validate_package(",
            '"packageRetained": True',
            '"servicesActive": False',
            '"servicesEnabled": False',
            '"bootSafe": True',
            "WantedBy=sysinit.target",
            "Before=kvmd.service kvmd-nginx.service",
            "systemd-run --quiet --collect",
            "dpkg --audit",
            "other_package_hash",
            "df --output=iavail",
            "getcap",
            "installed_tree_exact",
            "units_loaded_disabled_inactive",
            "sync",
        ):
            self.assertIn(required, retained)
        for forbidden in (
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "systemd-sysusers",
            "systemd-tmpfiles",
            "rm -rf",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, retained)
        self.assertIn("tools/test_stage_device_debian_runtime_retain.py", checks)
        self.assertIn("tools/stage_device_debian_runtime_retain.py", checks)
        self.assertIn("tools/test_capture_debian_runtime_retained.py", checks)
        self.assertIn("tools/capture_debian_runtime_retained.py", checks)
        self.assertIn(
            "tools/test_capture_debian_runtime_retain_preflight.py", checks
        )
        self.assertIn("tools/capture_debian_runtime_retain_preflight.py", checks)
        self.assertIn("retain.build_preflight_script", preflight)
        self.assertIn('"readOnly": True', preflight)
        self.assertIn("retain.build_final_script", inspector)
        self.assertIn('"readOnly": True', inspector)
        for forbidden in (
            "dpkg -i",
            "dpkg --purge",
            "systemctl start",
            "systemctl stop",
            "systemctl enable",
            "systemctl disable",
            "systemd-sysusers",
            "systemd-tmpfiles",
        ):
            self.assertNotIn(forbidden, preflight)
            self.assertNotIn(forbidden, inspector)

    def test_runtime_provisioning_policy_is_fixed_complete_and_not_activated(
        self,
    ) -> None:
        policy = (ROOT / "tools/runtime_provisioning_policy.py").read_text()
        design = (ROOT / "docs/design/runtime-provision-gate-v1.md").read_text()
        checks = (ROOT / "scripts/check.sh").read_text()

        for required in (
            'KVMD_ACCOUNT = "hardened-blikvm-broker"',
            'SHARED_GROUP = "hardened-blikvm"',
            '"hardened-blikvm-server"',
            '"hardened-blikvm-broker"',
            '"hardened-blikvm-video"',
            '"hardened-blikvm-control"',
            '"hardened-blikvm-hid"',
            '"hardened-blikvm-update"',
            'CONFIG_ROOT = "/etc/hardened-blikvm"',
            "validate_sysusers_payload",
            "retained package sysusers documentation does not match source",
            "629bfaa0e3653acd59271c135d3322910c275ecb6533221b6b689639ff9ea070",
        ):
            self.assertIn(required, policy + design)
        for forbidden in (
            'KVMD_ACCOUNT = "admin"',
            'KVMD_ACCOUNT = "root"',
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "physical Power/Reset",
        ):
            self.assertNotIn(forbidden, policy)
        self.assertIn("tools/runtime_provisioning_policy.py", checks)
        self.assertIn("tools/test_runtime_provisioning_policy.py", checks)

    def test_runtime_provisioning_gate_is_boot_safe_secret_private_and_inactive(
        self,
    ) -> None:
        gate = (ROOT / "tools/stage_device_runtime_provision.py").read_text()
        prepare = (
            ROOT / "tools/stage_device_runtime_provision_prepare.py"
        ).read_text()
        recovery = (ROOT / "tools/recover_device_runtime_provision.py").read_text()
        preflight = (ROOT / "tools/capture_runtime_provision_preflight.py").read_text()
        inspector = (ROOT / "tools/capture_runtime_provisioned.py").read_text()
        baseline = (ROOT / "docs/baselines/device-runtime-provisioning.md").read_text()
        checks = (ROOT / "scripts/check.sh").read_text()

        for required in (
            "policy.load_sysusers_source(package)",
            "systemd-sysusers --root=",
            'systemd-sysusers "$pinned_sysusers"',
            "os.getrandom(32, os.GRND_NONBLOCK).hex()",
            'kvmd-htpasswd set -i -q "$kvmd_account" < "$password_file"',
            "before_meta_%s",
            "after_meta_%s",
            "precheck_aux pwd_lock .pwd.lock",
            "restore_aux pwd_lock .pwd.lock",
            "WantedBy=sysinit.target",
            "systemd-run --quiet --collect",
            'mv "$config_stage" "$config_root"',
            "setpriv --reuid",
            '"secretEmitted": False',
            '"servicesActive": False',
            '"servicesEnabled": False',
        ):
            self.assertIn(required, gate)
        for forbidden in (
            "kvmd-htpasswd del",
            "rm -rf",
            "systemctl start hardened-blikvm",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg --purge",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, gate)
        self.assertIn("retain.build_final_script", preflight)
        self.assertIn("provision.build_preflight_script", preflight)
        self.assertIn("provision.build_prepare_script", prepare)
        self.assertIn("provision.build_cleanup_script", prepare)
        self.assertNotIn("provision.build_apply_script", prepare)
        self.assertNotIn("provision.build_commit_script", prepare)
        self.assertIn("provision.build_final_script", inspector)
        self.assertIn("provision.build_recovery_discovery_script", recovery)
        self.assertIn("provision.build_rollback_replacement_script", recovery)
        self.assertIn("provision.build_execute_recovery_script", recovery)
        self.assertNotIn("provision.build_apply_script", recovery)
        self.assertNotIn("provision.build_commit_script", recovery)
        for path in (
            "tools/stage_device_runtime_provision.py",
            "tools/stage_device_runtime_provision_prepare.py",
            "tools/capture_runtime_provision_preflight.py",
            "tools/capture_runtime_provisioned.py",
            "tools/test_stage_device_runtime_provision.py",
            "tools/test_capture_runtime_provision.py",
            "tools/recover_device_runtime_provision.py",
        ):
            self.assertIn(path, checks)
        for required in (
            "e60f37c90bcb7d837050b1c22f2c6d541454c0e6",
            "fff0cf4d8fb043ed90f5895247178727",
            "548af6f081d048a0bf7620f0d4862571",
            "7747aac04c5044f59c1a9bbb5cf2a574",
            "be45ac08a8e9406cb5158cca5d58c2a6",
            "320a52c91f735e9681c106054cbd7c3b72f2eb79",
            "a9cb3a05d23f4a0b9ec7a5dcaaee00d5",
            "4fac25c173ea43d2bdb75a31a7f02736",
            "3dd45ee8ab954ab88792e6eb7baf0d8d",
            "5210974a13af4c3bbdb27f0d6d059d58",
            "e609c6fbcf884862bd0f85579f2ecff4",
            "290b2dd8acdb4bdb9ce1b98f5840d30b",
            "password that never entered host output",
        ):
            self.assertIn(required, baseline)

    def test_runtime_activation_gate_is_reversible_loopback_and_control_free(
        self,
    ) -> None:
        gate = (ROOT / "tools/stage_device_runtime_activation.py").read_text()
        design = (ROOT / "docs/design/runtime-activation-gate-v1.md").read_text()
        baseline = (ROOT / "docs/baselines/device-runtime-activation.md").read_text()
        checks = (ROOT / "scripts/check.sh").read_text()

        for required in (
            'systemctl start "$broker_unit"',
            'systemctl start "$video_unit"',
            'systemctl start "$server_unit"',
            "BLI_KVM_SECURE_COOKIES=0",
            "hardened-blikvm-control-broker.socket",
            "control_boundary_absent",
            "process_no_udp_or_abstract",
            "server_listener_exact",
            "cgroup_empty",
            "socket_tail_clean",
            "CREDENTIALS_DIRECTORY",
            "setpriv --reuid",
            "/api/inventory",
            "/api/diagnostics",
            "/api/stream/mjpeg",
            "/api/auth/logout",
            '"controlServiceStarted": False',
            '"servicesEnabled": False',
            '"nginxMutation": False',
            '"secretEmitted": False',
            "WantedBy=sysinit.target",
        ):
            self.assertIn(required, gate + design)
        for forbidden in (
            "systemctl start hardened-blikvm-control",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg -i",
            "dpkg --purge",
            "rm -rf",
            "/etc/nginx/",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, gate)
        self.assertIn("tools/stage_device_runtime_activation.py", checks)
        self.assertIn("tools/test_stage_device_runtime_activation.py", checks)
        for required in (
            "795d2639e8f07e8c1d07772776fdf64592e24b13",
            "f1ac7387abde40d58dd15349d28effd6",
            "05ba8c0c32aa4f39abd52fd707cb4fcf",
            "17b7e0a8147c490fa1897ab29e24b52a",
            "4a83f5e",
            "51106cd86da24182938d1c6264dff605",
            "d296470e2c05417eb49635d305da71e6",
            "eb5d08b9f0f764a4a02164dba2d9eb88c268bfac",
            "6c4c31dd79874fcaafac18de189967ad",
            "28bd2fea56244731b2d621d1f73396fe",
            "CLOSE_WAIT",
            "FIN_WAIT2",
        ):
            self.assertIn(required, baseline)

    def test_installed_tls_browser_gate_is_shadowed_unprivileged_and_reversible(
        self,
    ) -> None:
        gate = (ROOT / "tools/stage_device_installed_tls_browser.py").read_text()
        design = (ROOT / "docs/design/installed-tls-browser-gate-v1.md").read_text()
        baseline = (ROOT / "docs/baselines/device-installed-tls-browser.md").read_text()
        checks = (ROOT / "scripts/check.sh").read_text()

        for required in (
            "listen 127.0.0.1:{TLS_PORT} ssl;",
            "proxy_pass http://127.0.0.1:18080;",
            "ssl_protocols TLSv1.2 TLSv1.3;",
            "ssl_session_tickets off;",
            "--property=User=www-data",
            "--property=IPAddressDeny=any",
            "--property=IPAddressAllow=localhost",
            "--property=SocketBindAllow=tcp:18443",
            "--property=CapabilityBoundingSet=",
            "tls.browser_runtime()",
            "tls.tunnel_command",
            "tls.validate_browser",
            "provision.build_final_script",
            "nginx_tree_hash",
            "control_boundary_absent",
            "socket_tail_clean",
            '"workingNginxMutation": False',
            '"servicesEnabled": False',
            '"secretEmitted": False',
            "WantedBy=sysinit.target",
        ):
            self.assertIn(required, gate + design)
        for forbidden in (
            "systemctl start hardened-blikvm-control",
            "systemctl enable hardened-blikvm",
            "systemctl restart kvmd",
            "systemctl stop kvmd",
            "dpkg -i",
            "dpkg --purge",
            "rm -rf",
            "/etc/nginx/",
            "/etc/hardened-blikvm/tls",
            "reboot",
            "shutdown",
            "poweroff",
        ):
            self.assertNotIn(forbidden, gate)
        self.assertIn("tools/stage_device_installed_tls_browser.py", checks)
        self.assertIn("tools/test_stage_device_installed_tls_browser.py", checks)
        for required in (
            "7fc49ad78c24749fbc889c5f9ab848590f28d014",
            "43e531fd7ba143e4b4399969d13f6f18",
            "08fcd876a4664a6e9e970fc056b07385",
            "38 API responses",
            "zero control requests",
            "zero WebSockets",
            "MJPEG decoded and reconnected four times",
        ):
            self.assertIn(required, baseline)

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

    def test_read_only_services_do_not_skip_boot_on_late_kvmd_sockets(self) -> None:
        broker = (
            ROOT / "packaging/systemd/hardened-blikvm-platform-broker.service"
        ).read_text()
        video = (
            ROOT / "packaging/systemd/hardened-blikvm-video-proxy.service"
        ).read_text()
        for unit in (broker, video):
            self.assertIn("Requires=kvmd.service", unit)
            self.assertIn("After=kvmd.service", unit)
            self.assertIn("Restart=on-failure", unit)
            self.assertNotIn("ConditionPathExists=/run/kvmd/", unit)

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
        self.assertIn("docs/api/diagnostics-v1.md", builder)
        self.assertNotIn("tls.key", builder)
        self.assertNotIn("tls.crt", builder)

    def test_public_trust_enrollment_baseline_is_bounded_and_nonsecret(self) -> None:
        baseline = (
            ROOT / "docs/baselines/application-trust-enrollment.md"
        ).read_text()
        contract = (
            ROOT / "docs/api/application-trust-enrollment-v1.md"
        ).read_text()
        source = (
            ROOT / "crates/bli-kvm-update-trust-config/src/lib.rs"
        ).read_text()
        for required in (
            "c70f8ff99d4a5a917adb014fd1e7b50d7563d5622363de5f478983d38a51af38",
            "No real production key",
            "public-only",
            "ApplicationTrustRoot",
            "ATX Power/Reset",
        ):
            self.assertIn(required, baseline + contract + source)
        for forbidden in (
            "10.0.",
            "admin1234",
            "ssh_password",
            "gui_password",
            "private_key_hex",
        ):
            self.assertNotIn(forbidden, baseline + contract + source)
        aarch64_builder = (ROOT / "scripts/build-aarch64.sh").read_text()
        prototype_builder = (ROOT / "scripts/build-prototype-bundle.sh").read_text()
        self.assertIn(
            '"$release_dir/bli-kvm-update-trust-config"', aarch64_builder
        )
        self.assertNotIn(
            'install -m 0755 "$release_dir/bli-kvm-update-trust-config"',
            prototype_builder,
        )

    def test_update_supervisor_is_local_health_only_and_sandboxed(self) -> None:
        recovery = (ROOT / "packaging/systemd/hardened-blikvm-update-recover.service").read_text()
        self.assertIn("Type=oneshot", recovery)
        self.assertIn("RemainAfterExit=yes", recovery)
        self.assertIn("User=hardened-blikvm-update", recovery)
        self.assertIn("watchdog-once", recovery)
        self.assertIn("LoadCredential=update-supervisor-config:", recovery)
        self.assertNotIn("Condition", recovery)
        self.assertNotIn("OnSuccess=", recovery)
        for name in ("current", "candidate"):
            server = (ROOT / f"packaging/systemd/hardened-blikvm-server-{name}.service").read_text()
            for directive in ("Requires=", "After="):
                self.assertTrue(any(line.startswith(directive) and "hardened-blikvm-update-recover.service" in line.split("=", 1)[1].split()
                                    for line in server.splitlines()))
        watchdog = (
            ROOT / "packaging/systemd/hardened-blikvm-update-watchdog.service"
        ).read_text()
        for directive in ("Requires=", "After="):
            self.assertTrue(any(line.startswith(directive) and "hardened-blikvm-update-recover.service" in line.split("=", 1)[1].split()
                                for line in watchdog.splitlines()))
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
            self.assertIn("LoadCredential=update-supervisor-config:/etc/hardened-blikvm/update-supervisor.json", unit)
            self.assertIn("--config /run/credentials/%n/update-supervisor-config", unit)
            self.assertNotIn("%d", unit)
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

    def test_update_status_worker_has_no_write_or_apply_authority(self) -> None:
        worker = (ROOT / "packaging/systemd/hardened-blikvm-update-status@.service").read_text()
        socket = (ROOT / "packaging/systemd/hardened-blikvm-update-status.socket").read_text()
        self.assertIn("User=hardened-blikvm-update", worker)
        self.assertIn("ProtectSystem=strict", worker)
        self.assertIn("status-socket --peer-user hardened-blikvm-server", worker)
        self.assertIn("StandardInput=socket", worker)
        self.assertIn("StandardOutput=socket", worker)
        for directive in ("OnSuccess=", "ReadWritePaths=", "StateDirectory=", "RuntimeDirectory="):
            self.assertNotIn(directive, worker)
        self.assertNotIn("hardened-blikvm-update-recover.service", worker)
        self.assertIn("MaxConnections=4", socket)
        self.assertIn("DirectoryMode=0755", socket)
        self.assertIn("SocketMode=0620", socket)

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
        self.assertIn("--credential-directory /run/hardened-blikvm-candidate-credentials/%n", candidate)
        self.assertIn("BindReadOnlyPaths=/run/credentials/%n:/run/hardened-blikvm-candidate-credentials/%n", candidate)
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
            "systemctl --no-ask-password restart hardened-blikvm-server-current.service",
            apply_confirmed,
        )
        self.assertIn("ConditionPathExists=/run/hardened-blikvm-update/confirmed-apply-request.json", apply_confirmed)
        self.assertIn("User=hardened-blikvm-update", apply_confirmed)
        self.assertIn("StandardInput=null", apply_confirmed)
        self.assertNotIn("StandardInput=file:", apply_confirmed)
        self.assertIn("ack-confirmed-apply-credential", apply_confirmed)
        self.assertIn("BindReadOnlyPaths=/run/credentials/%n:/run/hardened-blikvm-apply-credentials/%n", apply_confirmed)
        self.assertIn("--config /run/hardened-blikvm-apply-credentials/%n/update-supervisor-config ack-confirmed-apply-credential", apply_confirmed)
        self.assertNotIn("ExecStartPre=", apply_confirmed)
        self.assertIn("ExecStart=+/usr/bin/systemctl", apply_confirmed)
        watchdog = (ROOT / "packaging/systemd/hardened-blikvm-update-watchdog.service").read_text()
        self.assertNotIn("OnSuccess=hardened-blikvm-update-apply-candidate", watchdog)
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
