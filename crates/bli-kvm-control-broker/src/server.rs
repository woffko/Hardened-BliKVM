use std::{
    error::Error,
    fmt,
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    sync::Arc,
    time::Instant,
};

use bli_kvm_control_protocol::{
    CONTROL_SCHEMA_VERSION, CONTROL_SCHEMA_VERSION_V2, ControlProtocolError, ControlRejectionCode,
    ControlRequestEnvelope, ControlResponse, ControlResponseEnvelope, MAX_CONTROL_MESSAGE_BYTES,
    decode_request, decode_request_v2, encode_frame, encode_response, encode_response_v2,
};
use bli_kvm_diagnostic_producer_client::{
    DiagnosticReportReceipt, DiagnosticReporter, DiagnosticReporterHandle,
};
use bli_kvm_diagnostic_protocol::{DiagnosticEvent, RejectReason};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixListener,
    sync::{Mutex as TokioMutex, Semaphore},
    time::timeout,
};

use crate::{ControlBrokerConfig, ControlBrokerConfigError, ControlV2State};

#[derive(Debug)]
pub enum ControlBrokerError {
    Config(ControlBrokerConfigError),
    InvalidRuntimeDirectory,
    SocketAlreadyExists,
    TransitionUnavailable,
    Io(io::Error),
}

impl fmt::Display for ControlBrokerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Config(_) => "control broker configuration is invalid",
            Self::InvalidRuntimeDirectory => "control broker runtime directory is invalid",
            Self::SocketAlreadyExists => "control broker socket already exists",
            Self::TransitionUnavailable => "control broker transition backend is unavailable",
            Self::Io(_) => "control broker I/O failed",
        })
    }
}

impl Error for ControlBrokerError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::Io(error) => Some(error),
            Self::InvalidRuntimeDirectory
            | Self::SocketAlreadyExists
            | Self::TransitionUnavailable => None,
        }
    }
}

impl From<ControlBrokerConfigError> for ControlBrokerError {
    fn from(error: ControlBrokerConfigError) -> Self {
        Self::Config(error)
    }
}

impl From<io::Error> for ControlBrokerError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

pub async fn run(config: ControlBrokerConfig) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let result = serve(config, diagnostics, ControlV2State::disabled()).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with a HID transition backend attached from the dedicated
/// credential and sandbox. Only the `hid-transition-profile` feature builds
/// this entry point; shipped builds keep the capability disabled.
#[cfg(feature = "hid-transition-profile")]
pub async fn run_with_hid_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    kvmd_socket: impl Into<std::path::PathBuf>,
    credentials: bli_kvm_kvmd_client::KvmdCredentials,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let client = bli_kvm_kvmd_hid_client::KvmdHidClient::new(kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(journal_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let engine = bli_kvm_hid_transition::HidTransitionEngine::new(client, credentials, ledger)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state = ControlV2State::with_hid_policy(bli_kvm_hid_policy::HidControlPolicy::new(engine));
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with a HID backend attached from the Rust gadget
/// instead of KVMD, for the reviewed gadget test profile. Only the
/// `gadget-hid-profile` feature builds this entry point.
#[cfg(feature = "gadget-hid-profile")]
pub async fn run_with_gadget_hid_profile(
    config: ControlBrokerConfig,
    ledger_path: impl Into<std::path::PathBuf>,
    hidg_dir: impl Into<std::path::PathBuf>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(ledger_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let engine = bli_kvm_hid_transition::HidTransitionEngine::new_with_executor(
        bli_kvm_hid_transition::HidExecutor::Gadget(
            bli_kvm_hid_transition::GadgetHidExecutor::new(
                bli_kvm_hid_transition::GadgetHidConfig {
                    hidg_dir: hidg_dir.into(),
                    keyboard_function: "hid.usb0".into(),
                    absolute_mouse_function: "hid.usb1".into(),
                    relative_mouse_function: "hid.usb2".into(),
                },
            ),
        ),
        ledger,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state = ControlV2State::with_hid_policy(bli_kvm_hid_policy::HidControlPolicy::new(engine));
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with the ATX state machine over the reviewed GPIO
/// executor supplied by the owner. Only the `gpio-atx-profile` feature builds
/// this entry point; it links no KVMD crate.
#[cfg(feature = "gpio-atx-profile")]
pub async fn run_with_gpio_atx_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    runner: Box<dyn bli_kvm_atx_transition::AtxPulseRunner>,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let mut engine = bli_kvm_atx_transition::AtxTransitionEngine::open_with_executor(
        journal_path,
        bli_kvm_atx_transition::AtxExecutor::Gpio(std::sync::Mutex::new(runner)),
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let reconciled_at_ms = now_unix_millis();
    for reconciliation in reconciliations {
        engine
            .resolve_pending(
                reconciliation.operation_id,
                reconciliation.completion,
                reconciliation.observation,
                reconciled_at_ms,
            )
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    }
    let state = ControlV2State::with_atx_engine(engine);
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the media and HID backends from the Rust gadget in one process, the
/// single privileged owner of the composite gadget. Only a build with both
/// gadget features compiles this entry point; it links no KVMD crate.
#[cfg(all(feature = "gadget-media-profile", feature = "gadget-hid-profile"))]
pub async fn run_with_gadget_profile(
    config: ControlBrokerConfig,
    media_journal: impl Into<std::path::PathBuf>,
    catalog_path: impl Into<std::path::PathBuf>,
    gadget: bli_kvm_media_transition::GadgetMediaConfig,
    hid_ledger: impl Into<std::path::PathBuf>,
    hidg_dir: impl Into<std::path::PathBuf>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let catalog = bli_kvm_media_catalog::MediaCatalog::open(catalog_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let media = bli_kvm_media_transition::MediaTransitionEngine::open_with_executor(
        media_journal,
        catalog,
        bli_kvm_media_transition::MediaExecutor::Gadget(
            bli_kvm_media_transition::GadgetMediaExecutor::new(gadget),
        ),
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(hid_ledger)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid = bli_kvm_hid_transition::HidTransitionEngine::new_with_executor(
        bli_kvm_hid_transition::HidExecutor::Gadget(
            bli_kvm_hid_transition::GadgetHidExecutor::new(
                bli_kvm_hid_transition::GadgetHidConfig {
                    hidg_dir: hidg_dir.into(),
                    keyboard_function: "hid.usb0".into(),
                    absolute_mouse_function: "hid.usb1".into(),
                    relative_mouse_function: "hid.usb2".into(),
                },
            ),
        ),
        ledger,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state =
        ControlV2State::with_media_and_hid(media, bli_kvm_hid_policy::HidControlPolicy::new(hid));
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the ATX backend from the Linux GPIO character device supplied by the
/// owner. Only the `chardev-atx-profile` feature builds this entry point.
#[cfg(feature = "chardev-atx-profile")]
pub async fn run_with_chardev_atx_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    chip_path: impl AsRef<std::path::Path>,
    timing: bli_kvm_atx_gpio_executor::AtxTiming,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let runner = bli_kvm_atx_gpio_executor::chardev_atx_executor(chip_path.as_ref(), timing)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    run_with_gpio_atx_profile(config, journal_path, Box::new(runner), reconciliations).await
}

/// Serves the media, HID and ATX backends from one process: the single owner
/// of the composite gadget and the reviewed ATX lines. Only a build with all
/// three gadget/ATX profiles compiles this entry point; it links no KVMD crate.
#[cfg(all(
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "gpio-atx-profile"
))]
#[allow(clippy::too_many_arguments)]
pub async fn run_with_board_profile(
    config: ControlBrokerConfig,
    media_journal: impl Into<std::path::PathBuf>,
    catalog_path: impl Into<std::path::PathBuf>,
    gadget: bli_kvm_media_transition::GadgetMediaConfig,
    hid_ledger: impl Into<std::path::PathBuf>,
    hidg_dir: impl Into<std::path::PathBuf>,
    atx_journal: impl Into<std::path::PathBuf>,
    runner: Box<dyn bli_kvm_atx_transition::AtxPulseRunner>,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let catalog = bli_kvm_media_catalog::MediaCatalog::open(catalog_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let media = bli_kvm_media_transition::MediaTransitionEngine::open_with_executor(
        media_journal,
        catalog,
        bli_kvm_media_transition::MediaExecutor::Gadget(
            bli_kvm_media_transition::GadgetMediaExecutor::new(gadget),
        ),
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(hid_ledger)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid = bli_kvm_hid_transition::HidTransitionEngine::new_with_executor(
        bli_kvm_hid_transition::HidExecutor::Gadget(
            bli_kvm_hid_transition::GadgetHidExecutor::new(
                bli_kvm_hid_transition::GadgetHidConfig {
                    hidg_dir: hidg_dir.into(),
                    keyboard_function: "hid.usb0".into(),
                    absolute_mouse_function: "hid.usb1".into(),
                    relative_mouse_function: "hid.usb2".into(),
                },
            ),
        ),
        ledger,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let mut atx = bli_kvm_atx_transition::AtxTransitionEngine::open_with_executor(
        atx_journal,
        bli_kvm_atx_transition::AtxExecutor::Gpio(std::sync::Mutex::new(runner)),
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let reconciled_at_ms = now_unix_millis();
    for reconciliation in reconciliations {
        atx.resolve_pending(
            reconciliation.operation_id,
            reconciliation.completion,
            reconciliation.observation,
            reconciled_at_ms,
        )
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    }
    let state = ControlV2State::with_board_backends(
        media,
        bli_kvm_hid_policy::HidControlPolicy::new(hid),
        atx,
    );
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the board profile with the ATX runner built from the Linux GPIO
/// character device. Only the `chardev-atx-profile` feature builds this.
#[cfg(all(
    feature = "gadget-media-profile",
    feature = "gadget-hid-profile",
    feature = "chardev-atx-profile"
))]
#[allow(clippy::too_many_arguments)]
pub async fn run_with_chardev_board_profile(
    config: ControlBrokerConfig,
    media_journal: impl Into<std::path::PathBuf>,
    catalog_path: impl Into<std::path::PathBuf>,
    gadget: bli_kvm_media_transition::GadgetMediaConfig,
    hid_ledger: impl Into<std::path::PathBuf>,
    hidg_dir: impl Into<std::path::PathBuf>,
    atx_journal: impl Into<std::path::PathBuf>,
    chip_path: impl AsRef<std::path::Path>,
    timing: bli_kvm_atx_gpio_executor::AtxTiming,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let runner = bli_kvm_atx_gpio_executor::chardev_atx_executor(chip_path.as_ref(), timing)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    run_with_board_profile(
        config,
        media_journal,
        catalog_path,
        gadget,
        hid_ledger,
        hidg_dir,
        atx_journal,
        Box::new(runner),
        reconciliations,
    )
    .await
}

/// One reviewed reconciliation decision applied before the daemon serves./// One reviewed reconciliation decision applied before the daemon serves.
///
/// The observation is mandatory read-only evidence and never infers the
/// completion; the operator still classifies the outcome.
#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtxReconciliation {
    pub operation_id: [u8; 16],
    pub completion: bli_kvm_atx_policy::AtxCompletion,
    pub observation: bli_kvm_atx_transition::PowerLedObservation,
}

/// Serves the broker with the persisted ATX state machine attached.
///
/// Pending operations are reconciled from the supplied reviewed evidence
/// before the socket starts serving. Only the `atx-transition-profile` feature
/// builds this entry point.
#[cfg(feature = "atx-transition-profile")]
pub async fn run_with_atx_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    kvmd_socket: impl Into<std::path::PathBuf>,
    credentials: bli_kvm_kvmd_client::KvmdCredentials,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let client = bli_kvm_kvmd_atx_client::KvmdAtxClient::new(kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let mut engine =
        bli_kvm_atx_transition::AtxTransitionEngine::open(journal_path, client, credentials)
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let reconciled_at_ms = now_unix_millis();
    for reconciliation in reconciliations {
        engine
            .resolve_pending(
                reconciliation.operation_id,
                reconciliation.completion,
                reconciliation.observation,
                reconciled_at_ms,
            )
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    }
    let state = ControlV2State::with_atx_engine(engine);
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with both the HID and the ATX transition backends from
/// one state, for the restricted control test profile.
///
/// The KVMD credential is read twice from the systemd credential directory so
/// each backend owns an independent, non-shareable credential instance. Only a
/// build with both profile features compiles this entry point; shipped builds
/// keep both capabilities disabled.
#[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
pub async fn run_with_control_profile(
    config: ControlBrokerConfig,
    hid_ledger: impl Into<std::path::PathBuf>,
    hid_kvmd_socket: impl Into<std::path::PathBuf>,
    atx_journal: impl Into<std::path::PathBuf>,
    atx_kvmd_socket: impl Into<std::path::PathBuf>,
    credentials_directory: impl Into<std::path::PathBuf>,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let credentials_directory = credentials_directory.into();
    let hid_credentials = crate::profile::kvmd_credentials_from_directory(&credentials_directory)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let atx_credentials = crate::profile::kvmd_credentials_from_directory(&credentials_directory)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid_client = bli_kvm_kvmd_hid_client::KvmdHidClient::new(hid_kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(hid_ledger)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid_engine =
        bli_kvm_hid_transition::HidTransitionEngine::new(hid_client, hid_credentials, ledger)
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let atx_client = bli_kvm_kvmd_atx_client::KvmdAtxClient::new(atx_kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let mut atx_engine =
        bli_kvm_atx_transition::AtxTransitionEngine::open(atx_journal, atx_client, atx_credentials)
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let reconciled_at_ms = now_unix_millis();
    for reconciliation in reconciliations {
        atx_engine
            .resolve_pending(
                reconciliation.operation_id,
                reconciliation.completion,
                reconciliation.observation,
                reconciled_at_ms,
            )
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    }
    let state = ControlV2State::with_hid_and_atx(
        bli_kvm_hid_policy::HidControlPolicy::new(hid_engine),
        atx_engine,
    );
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with the HID, ATX and media backends from one state.
///
/// This is the combined restricted-control test profile entry point. The KVMD
/// credential is read three times from the systemd credential directory so each
/// backend owns an independent, non-shareable credential instance. Only a build
/// with all three profile features compiles this entry point.
#[cfg(all(
    feature = "hid-transition-profile",
    feature = "atx-transition-profile",
    feature = "media-transition-profile"
))]
#[allow(clippy::too_many_arguments)]
pub async fn run_with_restricted_profiles(
    config: ControlBrokerConfig,
    hid_ledger: impl Into<std::path::PathBuf>,
    hid_kvmd_socket: impl Into<std::path::PathBuf>,
    atx_journal: impl Into<std::path::PathBuf>,
    atx_kvmd_socket: impl Into<std::path::PathBuf>,
    media_journal: impl Into<std::path::PathBuf>,
    media_catalog: impl Into<std::path::PathBuf>,
    media_kvmd_socket: impl Into<std::path::PathBuf>,
    credentials_directory: impl Into<std::path::PathBuf>,
    reconciliations: Vec<AtxReconciliation>,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let credentials_directory = credentials_directory.into();
    let hid_credentials = crate::profile::kvmd_credentials_from_directory(&credentials_directory)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let atx_credentials = crate::profile::kvmd_credentials_from_directory(&credentials_directory)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let media_credentials = crate::profile::kvmd_credentials_from_directory(&credentials_directory)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid_client = bli_kvm_kvmd_hid_client::KvmdHidClient::new(hid_kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let ledger = bli_kvm_hid_transition::HidReleaseLedger::new(hid_ledger)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let hid_engine =
        bli_kvm_hid_transition::HidTransitionEngine::new(hid_client, hid_credentials, ledger)
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let atx_client = bli_kvm_kvmd_atx_client::KvmdAtxClient::new(atx_kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let mut atx_engine =
        bli_kvm_atx_transition::AtxTransitionEngine::open(atx_journal, atx_client, atx_credentials)
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let reconciled_at_ms = now_unix_millis();
    for reconciliation in reconciliations {
        atx_engine
            .resolve_pending(
                reconciliation.operation_id,
                reconciliation.completion,
                reconciliation.observation,
                reconciled_at_ms,
            )
            .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    }
    let media_client = bli_kvm_kvmd_media_client::KvmdMediaClient::new(media_kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let catalog = bli_kvm_media_catalog::MediaCatalog::open(media_catalog)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let media_engine = bli_kvm_media_transition::MediaTransitionEngine::open(
        media_journal,
        media_client,
        catalog,
        media_credentials,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state = ControlV2State::with_hid_and_atx(
        bli_kvm_hid_policy::HidControlPolicy::new(hid_engine),
        atx_engine,
    )
    .with_media_engine(media_engine);
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the broker with the persisted media engine attached.
///
/// The read-only `/msd` state is observed through the fixed KVMD client before
/// each operation, and only the `media-transition-profile` feature builds this
/// entry point. Shipped builds reject every media request.
#[cfg(feature = "media-transition-profile")]
pub async fn run_with_media_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    catalog_path: impl Into<std::path::PathBuf>,
    kvmd_socket: impl Into<std::path::PathBuf>,
    credentials: bli_kvm_kvmd_client::KvmdCredentials,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let client = bli_kvm_kvmd_media_client::KvmdMediaClient::new(kvmd_socket)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let catalog = bli_kvm_media_catalog::MediaCatalog::open(catalog_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let engine = bli_kvm_media_transition::MediaTransitionEngine::open(
        journal_path,
        client,
        catalog,
        credentials,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state = ControlV2State::disabled().with_media_engine(engine);
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

/// Serves the media transition backend from the Rust configfs gadget instead
/// of KVMD, for the reviewed gadget test profile.
#[cfg(feature = "gadget-media-profile")]
pub async fn run_with_gadget_media_profile(
    config: ControlBrokerConfig,
    journal_path: impl Into<std::path::PathBuf>,
    catalog_path: impl Into<std::path::PathBuf>,
    gadget: bli_kvm_media_transition::GadgetMediaConfig,
) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let catalog = bli_kvm_media_catalog::MediaCatalog::open(catalog_path)
        .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let executor = bli_kvm_media_transition::MediaExecutor::Gadget(
        bli_kvm_media_transition::GadgetMediaExecutor::new(gadget),
    );
    let engine = bli_kvm_media_transition::MediaTransitionEngine::open_with_executor(
        journal_path,
        catalog,
        executor,
    )
    .map_err(|_| ControlBrokerError::TransitionUnavailable)?;
    let state = ControlV2State::disabled().with_media_engine(engine);
    let result = serve(config, diagnostics, state).await;
    drop(diagnostic_reporter);
    result
}

#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
fn now_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
async fn serve<D>(
    config: ControlBrokerConfig,
    diagnostics: D,
    v2_state: ControlV2State,
) -> Result<(), ControlBrokerError>
where
    D: DiagnosticSubmitter,
{
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.socket_path).is_ok() {
        return Err(ControlBrokerError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.socket_path)?;
    let _cleanup = SocketCleanup(config.socket_path.clone());
    fs::set_permissions(&config.socket_path, Permissions::from_mode(0o660))?;
    let semaphore = Arc::new(Semaphore::new(config.max_connections));
    let v2 = Arc::new(TokioMutex::new(v2_state));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("control broker semaphore closed"))?;
        let request_timeout = config.request_timeout;
        let diagnostics = diagnostics.clone();
        let v2 = v2.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, request_timeout, diagnostics, v2).await;
        });
    }
}

fn validate_runtime_directory(config: &ControlBrokerConfig) -> Result<(), ControlBrokerError> {
    let parent = config
        .socket_path
        .parent()
        .ok_or(ControlBrokerError::InvalidRuntimeDirectory)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(ControlBrokerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

async fn handle_connection<S, D>(
    mut stream: S,
    request_timeout: std::time::Duration,
    diagnostics: D,
    v2: Arc<TokioMutex<ControlV2State>>,
) -> Result<(), io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    D: DiagnosticSubmitter,
{
    let request = timeout(request_timeout, read_payload(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control request timed out"))??;
    let response = respond(&request, &v2).await?;
    let payload = if response.schema_version == CONTROL_SCHEMA_VERSION_V2 {
        encode_response_v2(&response)
    } else {
        encode_response(&response)
    }
    .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control response invalid"))?;
    let rejection = match &response.response {
        ControlResponse::Rejected { code } => Some(reason_for(*code)),
        ControlResponse::Applied | ControlResponse::LeaseGranted { .. } => None,
    };
    timeout(
        request_timeout,
        stream.write_all(&encode_frame(&payload).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "control response too large")
        })?),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control response timed out"))??;
    if let Some(reason) = rejection {
        let _ = diagnostics.try_report(DiagnosticEvent::RequestRejected { reason });
    }
    Ok(())
}

async fn read_payload<S>(stream: &mut S) -> Result<Vec<u8>, io::Error>
where
    S: AsyncRead + Unpin,
{
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_CONTROL_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control request too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    Ok(payload)
}

async fn respond(
    payload: &[u8],
    v2: &Arc<TokioMutex<ControlV2State>>,
) -> Result<ControlResponseEnvelope, io::Error> {
    match decode_request(payload) {
        Ok(request) => Ok(deny_request(request)),
        Err(ControlProtocolError::UnsupportedSchemaVersion(version))
            if version == CONTROL_SCHEMA_VERSION_V2 =>
        {
            let request = decode_request_v2(payload).map_err(|_| invalid_request())?;
            let response = v2
                .lock()
                .await
                .handle_request(&request.operation_id, &request.request, Instant::now())
                .await;
            Ok(ControlResponseEnvelope {
                schema_version: CONTROL_SCHEMA_VERSION_V2,
                request_id: request.request_id,
                operation_id: request.operation_id,
                response,
            })
        }
        Err(_) => Err(invalid_request()),
    }
}

fn invalid_request() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "control request invalid")
}

fn reason_for(code: ControlRejectionCode) -> RejectReason {
    match code {
        ControlRejectionCode::CapabilityDisabled => RejectReason::CapabilityDisabled,
        ControlRejectionCode::BackendUnavailable
        | ControlRejectionCode::BackendRejected
        | ControlRejectionCode::Internal => RejectReason::BackendUnavailable,
        ControlRejectionCode::LeaseConflict | ControlRejectionCode::Duplicate => {
            RejectReason::Conflict
        }
        ControlRejectionCode::LeaseExpired
        | ControlRejectionCode::CooldownActive
        | ControlRejectionCode::PendingUnreconciled => RejectReason::Busy,
        ControlRejectionCode::NotArmed
        | ControlRejectionCode::NotAttested
        | ControlRejectionCode::ConfirmationStale
        | ControlRejectionCode::InvalidState => RejectReason::InvalidInput,
    }
}

fn deny_request(request: ControlRequestEnvelope) -> ControlResponseEnvelope {
    ControlResponseEnvelope {
        schema_version: CONTROL_SCHEMA_VERSION,
        request_id: request.request_id,
        operation_id: request.operation_id,
        response: ControlResponse::Rejected {
            code: ControlRejectionCode::CapabilityDisabled,
        },
    }
}

trait DiagnosticSubmitter: Clone + Send + Sync + 'static {
    fn try_report(&self, event: DiagnosticEvent) -> Option<DiagnosticReportReceipt>;
}

#[derive(Clone)]
enum ControlDiagnostics {
    Disabled,
    Reporter(DiagnosticReporterHandle),
}

impl DiagnosticSubmitter for ControlDiagnostics {
    fn try_report(&self, event: DiagnosticEvent) -> Option<DiagnosticReportReceipt> {
        match self {
            Self::Disabled => None,
            Self::Reporter(reporter) => Some(reporter.try_report(event)),
        }
    }
}

struct SocketCleanup(std::path::PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{
            Condvar, Mutex as StdMutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use bli_kvm_control_client::ControlClient;
    use bli_kvm_control_protocol::{
        ControlRequestEnvelopeV2, decode_response, decode_response_v2, encode_request,
        encode_request_v2,
    };
    use bli_kvm_core::{
        AtxControlAction, HidLeaseRequest, HidReport, PlatformControlRequest,
        PlatformControlRequestV2, VirtualMediaCommand,
    };
    use tokio::{io::DuplexStream, time::sleep};

    use super::*;

    const TEST_WAIT: Duration = Duration::from_millis(500);
    const TEST_START_WAIT: Duration = Duration::from_secs(2);

    async fn wait_for_socket(
        socket: &std::path::Path,
        broker: &mut tokio::task::JoinHandle<Result<(), ControlBrokerError>>,
    ) {
        timeout(TEST_START_WAIT, async {
            loop {
                if socket.exists() {
                    return;
                }
                tokio::select! {
                    result = &mut *broker => {
                        panic!("control broker ended before creating its socket: {result:?}");
                    }
                    () = sleep(Duration::from_millis(5)) => {}
                }
            }
        })
        .await
        .expect("live control broker did not create its socket within the start bound");
    }

    async fn stop_broker(broker: tokio::task::JoinHandle<Result<(), ControlBrokerError>>) {
        broker.abort();
        let error = timeout(TEST_WAIT, broker)
            .await
            .expect("control broker task retained process liveness")
            .expect_err("aborted control broker unexpectedly returned successfully");
        assert!(error.is_cancelled());
    }

    #[cfg(feature = "hid-transition-profile")]
    async fn serve_kvmd(
        socket: &std::path::Path,
        count: usize,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let Ok(read) = stream.read(&mut chunk).await else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = r#"{"ok":true}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                requests.push(String::from_utf8_lossy(&request).into_owned());
            }
            requests
        })
    }

    fn request(index: usize, request: PlatformControlRequest) -> ControlRequestEnvelope {
        ControlRequestEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id: u64::try_from(index + 1).unwrap(),
            operation_id: format!("{value:032x}", value = index + 1),
            request,
        }
    }

    async fn write_request(stream: &mut DuplexStream, request: &ControlRequestEnvelope) {
        let payload = encode_request(request).unwrap();
        stream
            .write_all(&encode_frame(&payload).unwrap())
            .await
            .unwrap();
    }

    async fn read_response(stream: &mut DuplexStream) -> ControlResponseEnvelope {
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let length = u32::from_be_bytes(prefix) as usize;
        assert!(length <= MAX_CONTROL_MESSAGE_BYTES);
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload).await.unwrap();
        decode_response(&payload).unwrap()
    }

    async fn exchange_with_diagnostics<D>(
        request: ControlRequestEnvelope,
        diagnostics: D,
    ) -> ControlResponseEnvelope
    where
        D: DiagnosticSubmitter,
    {
        let (server, mut client) = tokio::io::duplex(2 * MAX_CONTROL_MESSAGE_BYTES);
        let handler = tokio::spawn(handle_connection(
            server,
            TEST_WAIT,
            diagnostics,
            Arc::new(TokioMutex::new(ControlV2State::disabled())),
        ));
        write_request(&mut client, &request).await;
        let response = timeout(TEST_WAIT, read_response(&mut client))
            .await
            .expect("diagnostics delayed the control response");
        timeout(TEST_WAIT, handler)
            .await
            .expect("control handler did not finish")
            .unwrap()
            .unwrap();
        response
    }

    #[tokio::test]
    async fn all_valid_control_families_are_denied_without_a_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let mut broker = tokio::spawn(run(ControlBrokerConfig::new(&socket).unwrap()));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let requests = [
            PlatformControlRequest::Hid {
                report: HidReport::Keyboard {
                    data: [0, 0, 4, 0, 0, 0, 0, 0],
                },
            },
            PlatformControlRequest::Atx {
                action: AtxControlAction::PowerShort,
            },
            PlatformControlRequest::VirtualMedia {
                command: VirtualMediaCommand::SetConnected { connected: false },
            },
        ];
        for (index, request) in requests.into_iter().enumerate() {
            let operation_id = format!("{value:032x}", value = index + 1);
            assert_eq!(
                client.execute(operation_id, request).await.unwrap(),
                ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled
                }
            );
        }
        stop_broker(broker).await;
    }

    #[tokio::test]
    async fn v2_requests_are_denied_with_a_v2_response_and_a_fixed_report() {
        let request = ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 5,
            operation_id: format!("{value:032x}", value = 5),
            request: PlatformControlRequestV2::HidLease {
                lease: HidLeaseRequest::Acquire {
                    session_binding: 9,
                    target_binding: [0x11; 32],
                    ttl_seconds: 30,
                },
            },
        };
        let diagnostics = ScriptedDiagnostics::new([DiagnosticReportReceipt::Queued]);
        let (server, mut client) = tokio::io::duplex(2 * MAX_CONTROL_MESSAGE_BYTES);
        let handler = tokio::spawn(handle_connection(
            server,
            TEST_WAIT,
            diagnostics.clone(),
            Arc::new(TokioMutex::new(ControlV2State::disabled())),
        ));
        let payload = encode_request_v2(&request).unwrap();
        client
            .write_all(&encode_frame(&payload).unwrap())
            .await
            .unwrap();
        let mut prefix = [0_u8; 4];
        client.read_exact(&mut prefix).await.unwrap();
        let length = u32::from_be_bytes(prefix) as usize;
        let mut body = vec![0_u8; length];
        client.read_exact(&mut body).await.unwrap();
        let response = decode_response_v2(&body).unwrap();
        assert_eq!(response.schema_version, CONTROL_SCHEMA_VERSION_V2);
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(
            response.response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        timeout(TEST_WAIT, handler)
            .await
            .expect("control handler did not finish")
            .unwrap()
            .unwrap();
        let state = diagnostics.state.lock().unwrap();
        assert_eq!(
            state.events,
            vec![DiagnosticEvent::RequestRejected {
                reason: RejectReason::CapabilityDisabled
            }]
        );
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_profile_serves_attested_lease_grants_end_to_end() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let journal = directory.path().join("ledger.json");
        let kvmd = directory.path().join("absent-kvmd.sock");
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_hid_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            journal,
            kvmd,
            credentials,
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";

        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let response = client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap();
        match response {
            ControlResponse::LeaseGranted { lease_id } => {
                assert!(lease_id.iter().any(|byte| *byte != 0));
            }
            other => panic!("expected a lease grant, got {other:?}"),
        }
        stop_broker(broker).await;
    }

    #[cfg(feature = "media-transition-profile")]
    async fn serve_media_kvmd(
        socket: &std::path::Path,
        count: usize,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..count {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let Ok(read) = stream.read(&mut chunk).await else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request).into_owned();
                let body = if text.starts_with("GET /msd ") {
                    r#"{"ok":true,"result":{"enabled":true,"online":true,"busy":false,"drive":{"connected":false}}}"#
                } else {
                    r#"{"ok":true}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
                requests.push(text);
            }
            requests
        })
    }

    #[cfg(feature = "media-transition-profile")]
    #[tokio::test]
    async fn media_profile_applies_a_connect_command_to_the_kvmd_backend() {
        use bli_kvm_core::{MediaRequest, VirtualMediaCommand};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let kvmd = directory.path().join("kvmd.sock");
        let catalog = directory.path().join("catalog.json");
        std::fs::write(
            &catalog,
            r#"{"schemaVersion":1,"entries":[{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"1111111111111111111111111111111111111111111111111111111111111111","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}]}"#,
        )
        .unwrap();
        let backend = serve_media_kvmd(&kvmd, 2).await;
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_media_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("journal.json"),
            catalog,
            kvmd,
            credentials,
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let issued_at_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(
            client
                .execute_v2(
                    "0123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::SetConnected { connected: true },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let requests = timeout(TEST_WAIT, backend)
            .await
            .expect("mock KVMD did not finish")
            .unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /msd "));
        assert!(requests[1].starts_with("POST /msd/set_connected?connected=1 "));
        stop_broker(broker).await;
    }

    #[cfg(feature = "gadget-media-profile")]
    #[tokio::test]
    async fn gadget_media_profile_applies_commands_to_the_configfs_gadget() {
        use bli_kvm_core::{MediaRequest, VirtualMediaCommand, VirtualMediaControlMode};
        use bli_kvm_media_transition::{GadgetMediaConfig, GadgetStrings};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("rescue.iso"), b"image").unwrap();
        let catalog = image_dir.join("catalog.json");
        std::fs::write(
            &catalog,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"{}","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}}]}}"#,
                "1".repeat(64)
            ),
        )
        .unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();

        let mut broker = tokio::spawn(run_with_gadget_media_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("journal.json"),
            catalog,
            GadgetMediaConfig {
                configfs_root: configfs.clone(),
                udc: "musb-hdrc.2.auto".into(),
                image_directory: image_dir,
                strings: GadgetStrings {
                    manufacturer: "Hardened BLi-KVM".into(),
                    product: "BLi-KVM Composite Device".into(),
                    serial: "BLK-broker-test".into(),
                    configuration: "BLi-KVM configuration".into(),
                },
                id_vendor: 0x1d6b,
                id_product: 0x0104,
                bcd_usb: 0x0200,
                bcd_device: 0x0100,
            },
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let issued_at_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;

        assert_eq!(
            client
                .execute_v2(
                    "0123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::Select {
                                image_id: 7,
                                mode: VirtualMediaControlMode::CdRom,
                                writable: false,
                            },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;
        assert_eq!(
            client
                .execute_v2(
                    "1123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::SetConnected { connected: true },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );

        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.present);
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        assert!(state.mass_storage.unwrap().file.ends_with("rescue.iso"));
        stop_broker(broker).await;
    }

    #[cfg(feature = "gadget-hid-profile")]
    #[tokio::test]
    async fn gadget_hid_profile_writes_keyboard_reports_to_the_device() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let hidg = directory.path().join("hidg");
        std::fs::create_dir_all(&hidg).unwrap();
        std::fs::write(hidg.join("hidg0"), b"").unwrap();
        let mut broker = tokio::spawn(run_with_gadget_hid_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("ledger.json"),
            hidg.clone(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(
            std::fs::read(hidg.join("hidg0")).unwrap(),
            vec![0, 0, 4, 0, 0, 0, 0, 0]
        );
        stop_broker(broker).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_profile_applies_reports_to_the_kvmd_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let kvmd = directory.path().join("kvmd.sock");
        let backend = serve_kvmd(&kvmd, 1).await;
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_hid_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("ledger.json"),
            kvmd,
            credentials,
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let requests = timeout(TEST_WAIT, backend)
            .await
            .expect("mock KVMD did not finish")
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains("send_key?"),
            "unexpected KVMD request: {}",
            requests[0]
        );
        stop_broker(broker).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_profile_renews_only_the_active_lease() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_hid_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("ledger.json"),
            directory.path().join("absent-kvmd.sock"),
            credentials,
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Renew {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let mut other_id = lease_id;
        other_id[0] ^= 0xff;
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Renew {
                            lease_id: other_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Rejected {
                code: ControlRejectionCode::LeaseConflict
            }
        );
        stop_broker(broker).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_profile_applies_mouse_reports_to_the_kvmd_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let kvmd = directory.path().join("kvmd.sock");
        let backend = serve_kvmd(&kvmd, 1).await;
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_hid_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("ledger.json"),
            kvmd,
            credentials,
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::RelativeMouse {
                                data: [0, 10, 246, 0],
                            },
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let requests = timeout(TEST_WAIT, backend)
            .await
            .expect("mock KVMD did not finish")
            .unwrap();
        assert_eq!(requests.len(), 1);
        assert!(
            requests[0].contains("send_mouse_relative?delta_x=10&delta_y=-10"),
            "unexpected KVMD request: {}",
            requests[0]
        );
        stop_broker(broker).await;
    }

    #[cfg(all(
        feature = "hid-transition-profile",
        feature = "atx-transition-profile",
        feature = "media-transition-profile"
    ))]
    #[tokio::test]
    async fn restricted_profiles_attach_media_alongside_hid_and_atx() {
        use bli_kvm_core::{MediaRequest, VirtualMediaCommand};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let kvmd = directory.path().join("kvmd.sock");
        let credentials = directory.path().join("credentials");
        std::fs::create_dir(&credentials).unwrap();
        std::fs::write(credentials.join("kvmd-user"), b"test-user\n").unwrap();
        std::fs::write(credentials.join("kvmd-password"), b"not-a-real-password\n").unwrap();
        let catalog = directory.path().join("catalog.json");
        std::fs::write(
            &catalog,
            r#"{"schemaVersion":1,"entries":[{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"1111111111111111111111111111111111111111111111111111111111111111","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}]}"#,
        )
        .unwrap();
        let backend = serve_media_kvmd(&kvmd, 2).await;
        let mut broker = tokio::spawn(run_with_restricted_profiles(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("hid-ledger.json"),
            directory.path().join("absent-hid.sock"),
            directory.path().join("atx-journal.json"),
            directory.path().join("absent-atx.sock"),
            directory.path().join("media-journal.json"),
            catalog,
            kvmd,
            credentials,
            Vec::new(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let issued_at_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(
            client
                .execute_v2(
                    "0123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::SetConnected { connected: true },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let requests = timeout(TEST_WAIT, backend)
            .await
            .expect("mock KVMD did not finish")
            .unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /msd "));
        assert!(requests[1].starts_with("POST /msd/set_connected?connected=1 "));
        stop_broker(broker).await;
    }

    #[cfg(feature = "atx-transition-profile")]
    #[tokio::test]
    async fn atx_profile_reconciles_pending_before_serving() {
        use bli_kvm_atx_policy::{
            AtxCompletion, AtxDecision, AtxEligibility, AtxOperation, AtxPolicyEngine,
        };
        use bli_kvm_atx_transition::PowerLedObservation;
        use bli_kvm_core::AtxConfirmation;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let journal = directory.path().join("journal.json");
        let now_ms = now_unix_millis();
        {
            let mut policy = AtxPolicyEngine::open(&journal).unwrap();
            let eligibility = AtxEligibility::reviewed_for_confirmation(
                9, [0x44; 32], now_ms, now_ms, true, true,
            )
            .unwrap();
            let operation =
                AtxOperation::new([5; 16], 9, [0x44; 32], now_ms, AtxControlAction::PowerShort)
                    .unwrap();
            assert_eq!(
                policy.prepare(operation, eligibility, now_ms).unwrap(),
                AtxDecision::Permit
            );
        }
        let credentials =
            bli_kvm_kvmd_client::KvmdCredentials::new("test-user", "not-a-real-password").unwrap();
        let mut broker = tokio::spawn(run_with_atx_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            journal,
            directory.path().join("absent-atx.sock"),
            credentials,
            vec![AtxReconciliation {
                operation_id: [5; 16],
                completion: AtxCompletion::Applied,
                observation: PowerLedObservation::Off,
            }],
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let response = client
            .execute_v2(
                "05".repeat(16),
                PlatformControlRequestV2::Atx {
                    action: AtxControlAction::PowerShort,
                    confirmation: AtxConfirmation {
                        session_binding: 9,
                        confirmation_binding: [0x44; 32],
                        confirmed_at_unix_ms: now_ms,
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(response, ControlResponse::Applied);
        stop_broker(broker).await;
    }

    #[cfg(all(feature = "gadget-media-profile", feature = "gadget-hid-profile"))]
    #[tokio::test]
    async fn gadget_profile_serves_media_and_hid_from_one_owner() {
        use bli_kvm_core::{MediaRequest, VirtualMediaCommand, VirtualMediaControlMode};
        use bli_kvm_media_transition::{GadgetMediaConfig, GadgetStrings};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("rescue.iso"), b"image").unwrap();
        let catalog = image_dir.join("catalog.json");
        std::fs::write(
            &catalog,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"{}","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}}]}}"#,
                "1".repeat(64)
            ),
        )
        .unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let hidg = directory.path().join("hidg");
        std::fs::create_dir_all(&hidg).unwrap();
        std::fs::write(hidg.join("hidg0"), b"").unwrap();

        let mut broker = tokio::spawn(run_with_gadget_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("media-journal.json"),
            catalog,
            GadgetMediaConfig {
                configfs_root: configfs.clone(),
                udc: "musb-hdrc.2.auto".into(),
                image_directory: image_dir,
                strings: GadgetStrings {
                    manufacturer: "Hardened BLi-KVM".into(),
                    product: "BLi-KVM Composite Device".into(),
                    serial: "BLK-owner-test".into(),
                    configuration: "BLi-KVM configuration".into(),
                },
                id_vendor: 0x1d6b,
                id_product: 0x0104,
                bcd_usb: 0x0200,
                bcd_device: 0x0100,
            },
            directory.path().join("ledger.json"),
            hidg.clone(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";

        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(
            std::fs::read(hidg.join("hidg0")).unwrap(),
            vec![0, 0, 4, 0, 0, 0, 0, 0]
        );

        let issued_at_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(
            client
                .execute_v2(
                    "1123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::Select {
                                image_id: 7,
                                mode: VirtualMediaControlMode::CdRom,
                                writable: false,
                            },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;
        assert_eq!(
            client
                .execute_v2(
                    "2123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::SetConnected { connected: true },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.present);
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        stop_broker(broker).await;
    }

    #[cfg(all(
        feature = "gadget-media-profile",
        feature = "gadget-hid-profile",
        feature = "gpio-atx-profile"
    ))]
    #[tokio::test]
    async fn board_profile_serves_media_hid_and_atx_from_one_owner() {
        use bli_kvm_atx_transition::AtxPulseRunner;
        use bli_kvm_core::{
            AtxConfirmation, MediaRequest, VirtualMediaCommand, VirtualMediaControlMode,
        };
        use bli_kvm_media_transition::{GadgetMediaConfig, GadgetStrings};
        use std::sync::{Arc, Mutex};

        struct RecordingRunner(Arc<Mutex<Vec<AtxControlAction>>>);
        impl AtxPulseRunner for RecordingRunner {
            fn execute(&mut self, action: AtxControlAction) -> Result<(), String> {
                self.0.lock().unwrap().push(action);
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let image_dir = directory.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("rescue.iso"), b"image").unwrap();
        let catalog = image_dir.join("catalog.json");
        std::fs::write(
            &catalog,
            format!(
                r#"{{"schemaVersion":1,"entries":[{{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"{}","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}}]}}"#,
                "1".repeat(64)
            ),
        )
        .unwrap();
        let configfs = directory.path().join("configfs");
        std::fs::create_dir_all(&configfs).unwrap();
        let hidg = directory.path().join("hidg");
        std::fs::create_dir_all(&hidg).unwrap();
        std::fs::write(hidg.join("hidg0"), b"").unwrap();
        let recorded = Arc::new(Mutex::new(Vec::new()));

        let mut broker = tokio::spawn(run_with_board_profile(
            ControlBrokerConfig::new(socket.clone()).unwrap(),
            directory.path().join("media-journal.json"),
            catalog,
            GadgetMediaConfig {
                configfs_root: configfs.clone(),
                udc: "musb-hdrc.2.auto".into(),
                image_directory: image_dir,
                strings: GadgetStrings {
                    manufacturer: "Hardened BLi-KVM".into(),
                    product: "BLi-KVM Composite Device".into(),
                    serial: "BLK-owner-test".into(),
                    configuration: "BLi-KVM configuration".into(),
                },
                id_vendor: 0x1d6b,
                id_product: 0x0104,
                bcd_usb: 0x0200,
                bcd_device: 0x0100,
            },
            directory.path().join("ledger.json"),
            hidg.clone(),
            directory.path().join("atx-journal.json"),
            Box::new(RecordingRunner(recorded.clone())),
            Vec::new(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";

        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: bli_kvm_core::TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let lease_id = match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => lease_id,
            other => panic!("expected a lease grant, got {other:?}"),
        };
        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(
            std::fs::read(hidg.join("hidg0")).unwrap(),
            vec![0, 0, 4, 0, 0, 0, 0, 0]
        );

        let confirmed_at_unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        assert_eq!(
            client
                .execute_v2(
                    "05".repeat(16),
                    PlatformControlRequestV2::Atx {
                        action: AtxControlAction::PowerShort,
                        confirmation: AtxConfirmation {
                            session_binding: 9,
                            confirmation_binding: [0x44; 32],
                            confirmed_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(
            *recorded.lock().unwrap(),
            vec![AtxControlAction::PowerShort]
        );

        assert_eq!(
            client
                .execute_v2(
                    "1123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::Select {
                                image_id: 7,
                                mode: VirtualMediaControlMode::CdRom,
                                writable: false,
                            },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms: confirmed_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        tokio::time::sleep(std::time::Duration::from_millis(2_200)).await;
        assert_eq!(
            client
                .execute_v2(
                    "2123456789abcdef0123456789abcdef",
                    PlatformControlRequestV2::VirtualMedia {
                        request: MediaRequest {
                            command: VirtualMediaCommand::SetConnected { connected: true },
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            issued_at_unix_ms: confirmed_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        let state = bli_kvm_usb_gadget::observe(&configfs).unwrap();
        assert!(state.present);
        assert_eq!(state.udc.as_deref(), Some("musb-hdrc.2.auto"));
        stop_broker(broker).await;
    }

    #[cfg(feature = "chardev-atx-profile")]
    #[tokio::test]
    async fn chardev_atx_profile_rejects_a_missing_chip() {
        let directory = tempfile::tempdir().unwrap();
        let result = run_with_chardev_atx_profile(
            ControlBrokerConfig::new(directory.path().join("control.sock")).unwrap(),
            directory.path().join("journal.json"),
            "/nonexistent/gpiochip0",
            bli_kvm_atx_gpio_executor::AtxTiming {
                power_short_ms: 500,
                power_long_ms: 4_000,
                reset_ms: 200,
            },
            Vec::new(),
        )
        .await;
        assert!(result.is_err());
    }

    #[cfg(feature = "gpio-atx-profile")]
    #[tokio::test]
    async fn gpio_atx_profile_pulses_through_the_supplied_runner() {
        use bli_kvm_atx_transition::AtxPulseRunner;
        use bli_kvm_core::AtxConfirmation;
        use std::sync::{Arc, Mutex};

        struct RecordingRunner(Arc<Mutex<Vec<AtxControlAction>>>);
        impl AtxPulseRunner for RecordingRunner {
            fn execute(&mut self, action: AtxControlAction) -> Result<(), String> {
                self.0.lock().unwrap().push(action);
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let mut broker = tokio::spawn(run_with_gpio_atx_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("journal.json"),
            Box::new(RecordingRunner(recorded.clone())),
            Vec::new(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let confirmed_at_unix_ms = now_unix_millis();
        assert_eq!(
            client
                .execute_v2(
                    "05".repeat(16),
                    PlatformControlRequestV2::Atx {
                        action: AtxControlAction::PowerShort,
                        confirmation: AtxConfirmation {
                            session_binding: 9,
                            confirmation_binding: [0x44; 32],
                            confirmed_at_unix_ms,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        assert_eq!(
            *recorded.lock().unwrap(),
            vec![AtxControlAction::PowerShort]
        );
        stop_broker(broker).await;
    }

    #[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
    #[tokio::test]
    async fn control_profile_serves_hid_and_atx_from_one_state() {
        use bli_kvm_core::{AtxConfirmation, TargetAttestation};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let credentials = directory.path().join("credentials");
        std::fs::create_dir(&credentials).unwrap();
        std::fs::write(credentials.join("kvmd-user"), b"test-user\n").unwrap();
        std::fs::write(credentials.join("kvmd-password"), b"not-a-real-password\n").unwrap();
        let mut broker = tokio::spawn(run_with_control_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("hid-ledger.json"),
            directory.path().join("absent-hid.sock"),
            directory.path().join("atx-journal.json"),
            directory.path().join("absent-atx.sock"),
            credentials,
            Vec::new(),
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let operation_id = "0123456789abcdef0123456789abcdef";

        assert_eq!(
            client
                .execute_v2(
                    operation_id,
                    PlatformControlRequestV2::TargetAttest {
                        attestation: TargetAttestation {
                            session_binding: 9,
                            target_binding: [0x11; 32],
                            focus_binding: [0x22; 16],
                            ttl_seconds: 30,
                        },
                    },
                )
                .await
                .unwrap(),
            ControlResponse::Applied
        );
        match client
            .execute_v2(
                operation_id,
                PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: [0x11; 32],
                        ttl_seconds: 30,
                    },
                },
            )
            .await
            .unwrap()
        {
            ControlResponse::LeaseGranted { lease_id } => {
                assert!(lease_id.iter().any(|byte| *byte != 0));
            }
            other => panic!("expected a lease grant, got {other:?}"),
        }

        let response = client
            .execute_v2(
                "05".repeat(16),
                PlatformControlRequestV2::Atx {
                    action: AtxControlAction::PowerShort,
                    confirmation: AtxConfirmation {
                        session_binding: 9,
                        confirmation_binding: [0x44; 32],
                        confirmed_at_unix_ms: now_unix_millis(),
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            },
            "the ATX route must be active and reach the (absent) backend"
        );
        stop_broker(broker).await;
    }

    #[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
    #[tokio::test]
    async fn control_profile_reconciles_pending_before_serving() {
        use bli_kvm_atx_policy::{
            AtxCompletion, AtxDecision, AtxEligibility, AtxOperation, AtxPolicyEngine,
        };
        use bli_kvm_atx_transition::PowerLedObservation;
        use bli_kvm_core::{AtxConfirmation, AtxControlAction};

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let credentials = directory.path().join("credentials");
        std::fs::create_dir(&credentials).unwrap();
        std::fs::write(credentials.join("kvmd-user"), b"test-user\n").unwrap();
        std::fs::write(credentials.join("kvmd-password"), b"not-a-real-password\n").unwrap();
        let journal = directory.path().join("atx-journal.json");
        let now_ms = now_unix_millis();
        {
            let mut policy = AtxPolicyEngine::open(&journal).unwrap();
            let eligibility = AtxEligibility::reviewed_for_confirmation(
                9, [0x44; 32], now_ms, now_ms, true, true,
            )
            .unwrap();
            let operation =
                AtxOperation::new([5; 16], 9, [0x44; 32], now_ms, AtxControlAction::PowerShort)
                    .unwrap();
            assert_eq!(
                policy.prepare(operation, eligibility, now_ms).unwrap(),
                AtxDecision::Permit
            );
        }
        let mut broker = tokio::spawn(run_with_control_profile(
            ControlBrokerConfig::new(&socket).unwrap(),
            directory.path().join("hid-ledger.json"),
            directory.path().join("absent-hid.sock"),
            journal,
            directory.path().join("absent-atx.sock"),
            credentials,
            vec![AtxReconciliation {
                operation_id: [5; 16],
                completion: AtxCompletion::Applied,
                observation: PowerLedObservation::Off,
            }],
        ));
        wait_for_socket(&socket, &mut broker).await;
        let client = ControlClient::new(&socket).unwrap();
        let response = client
            .execute_v2(
                "05".repeat(16),
                PlatformControlRequestV2::Atx {
                    action: AtxControlAction::PowerShort,
                    confirmation: AtxConfirmation {
                        session_binding: 9,
                        confirmation_binding: [0x44; 32],
                        confirmed_at_unix_ms: now_ms,
                    },
                },
            )
            .await
            .unwrap();
        assert_eq!(response, ControlResponse::Applied);
        stop_broker(broker).await;
    }

    #[derive(Clone)]
    struct ScriptedDiagnostics {
        state: Arc<StdMutex<ScriptedDiagnosticState>>,
    }

    struct ScriptedDiagnosticState {
        receipts: VecDeque<DiagnosticReportReceipt>,
        events: Vec<DiagnosticEvent>,
    }

    impl ScriptedDiagnostics {
        fn new(receipts: impl IntoIterator<Item = DiagnosticReportReceipt>) -> Self {
            Self {
                state: Arc::new(StdMutex::new(ScriptedDiagnosticState {
                    receipts: receipts.into_iter().collect(),
                    events: Vec::new(),
                })),
            }
        }
    }

    impl DiagnosticSubmitter for ScriptedDiagnostics {
        fn try_report(&self, event: DiagnosticEvent) -> Option<DiagnosticReportReceipt> {
            let mut state = self.state.lock().unwrap();
            state.events.push(event);
            Some(
                state
                    .receipts
                    .pop_front()
                    .expect("one diagnostic receipt per request"),
            )
        }
    }

    #[tokio::test]
    async fn every_queue_receipt_preserves_the_exact_denial() {
        let receipts = [
            DiagnosticReportReceipt::Queued,
            DiagnosticReportReceipt::Dropped {
                reason: bli_kvm_diagnostic_producer_client::DiagnosticReportDropReason::Busy,
            },
            DiagnosticReportReceipt::Dropped {
                reason: bli_kvm_diagnostic_producer_client::DiagnosticReportDropReason::Full,
            },
            DiagnosticReportReceipt::Dropped {
                reason: bli_kvm_diagnostic_producer_client::DiagnosticReportDropReason::Closed,
            },
        ];
        let diagnostics = ScriptedDiagnostics::new(receipts);

        for index in 0..receipts.len() {
            let request = request(
                index,
                PlatformControlRequest::Atx {
                    action: AtxControlAction::PowerShort,
                },
            );
            let response = exchange_with_diagnostics(request.clone(), diagnostics.clone()).await;
            assert_eq!(response.request_id, request.request_id);
            assert_eq!(response.operation_id, request.operation_id);
            assert_eq!(
                response.response,
                ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled,
                }
            );
        }

        {
            let state = diagnostics.state.lock().unwrap();
            assert!(state.receipts.is_empty());
            assert_eq!(
                state.events,
                vec![
                    DiagnosticEvent::RequestRejected {
                        reason: RejectReason::CapabilityDisabled,
                    };
                    receipts.len()
                ]
            );
        }
    }

    #[derive(Clone)]
    struct BlockingDiagnostics {
        entered: Arc<AtomicBool>,
        gate: Arc<(StdMutex<bool>, Condvar)>,
    }

    impl BlockingDiagnostics {
        fn new() -> Self {
            Self {
                entered: Arc::new(AtomicBool::new(false)),
                gate: Arc::new((StdMutex::new(false), Condvar::new())),
            }
        }

        fn release(&self) {
            let (released, wake) = &*self.gate;
            *released.lock().unwrap() = true;
            wake.notify_all();
        }
    }

    impl DiagnosticSubmitter for BlockingDiagnostics {
        fn try_report(&self, event: DiagnosticEvent) -> Option<DiagnosticReportReceipt> {
            assert_eq!(
                event,
                DiagnosticEvent::RequestRejected {
                    reason: RejectReason::CapabilityDisabled,
                }
            );
            self.entered.store(true, Ordering::Release);
            let (released, wake) = &*self.gate;
            let guard = released.lock().unwrap();
            let (guard, result) = wake
                .wait_timeout_while(guard, Duration::from_secs(2), |released| !*released)
                .unwrap();
            assert!(*guard && !result.timed_out());
            Some(DiagnosticReportReceipt::Queued)
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn response_is_written_before_diagnostic_submission_finishes() {
        let diagnostics = BlockingDiagnostics::new();
        let request = request(
            0,
            PlatformControlRequest::VirtualMedia {
                command: VirtualMediaCommand::SetConnected { connected: false },
            },
        );
        let (server, mut client) = tokio::io::duplex(2 * MAX_CONTROL_MESSAGE_BYTES);
        let handler = tokio::spawn(handle_connection(
            server,
            TEST_WAIT,
            diagnostics.clone(),
            Arc::new(TokioMutex::new(ControlV2State::disabled())),
        ));
        write_request(&mut client, &request).await;

        for _ in 0..100 {
            if diagnostics.entered.load(Ordering::Acquire) {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        assert!(diagnostics.entered.load(Ordering::Acquire));
        let observed = timeout(TEST_WAIT, read_response(&mut client)).await;
        diagnostics.release();
        let response = observed.expect("control response waited for diagnostic submission");
        assert_eq!(response.request_id, request.request_id);
        assert_eq!(response.operation_id, request.operation_id);
        assert_eq!(
            response.response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled,
            }
        );
        timeout(TEST_WAIT, handler)
            .await
            .expect("released control handler did not finish")
            .unwrap()
            .unwrap();
    }

    #[test]
    fn rejects_group_writable_runtime_directories() {
        let directory = tempfile::tempdir().unwrap();
        let config = ControlBrokerConfig::new(directory.path().join("control.sock")).unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            validate_runtime_directory(&config),
            Err(ControlBrokerError::InvalidRuntimeDirectory)
        ));
    }
}
