#[cfg(feature = "diagnostic-export")]
use std::future::Future;
use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    sync::Arc,
};

#[cfg(feature = "diagnostic-export")]
use bli_kvm_broker_protocol::BrokerDiagnosticSnapshot;
use bli_kvm_broker_protocol::{
    BrokerErrorCode, BrokerReadRequest, BrokerReadResponse, BrokerRequestEnvelope,
    BrokerResponseEnvelope, MAX_BROKER_MESSAGE_BYTES, decode_request, encode_frame,
    encode_response,
};
#[cfg(feature = "diagnostic-export")]
use bli_kvm_diagnostic_export_client::{
    DiagnosticExportClient, DiagnosticExportExchangeStage, DiagnosticExportNotSentReason,
    DiagnosticExportReadError, DiagnosticExportReadOutcome,
};
#[cfg(feature = "diagnostic-export")]
use bli_kvm_diagnostic_peer_policy::{
    DiagnosticPeerAssignments, DiagnosticSource, KernelPeerCredentials,
};
use bli_kvm_kvmd_client::{KvmdClientError, KvmdCredentials, KvmdReadClient};
use bli_kvm_platform_bli::{
    BliPlatformObservations, read_h616_external_rtc, read_h616_temperature_millidegrees,
    sanitize_kvmd_inventory_with_observations,
};
#[cfg(feature = "diagnostic-export")]
use nix::unistd::{Group, User, getegid, geteuid};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    time::timeout,
};

use crate::{BrokerConfig, BrokerConfigError};

#[cfg(feature = "diagnostic-export")]
const DIAGNOSTIC_ACCOUNTS: [&str; 6] = [
    "hardened-blikvm-diagnostics",
    "hardened-blikvm-server",
    "hardened-blikvm-broker",
    "hardened-blikvm-video",
    "hardened-blikvm-control",
    "hardened-blikvm-update",
];
#[cfg(feature = "diagnostic-export")]
const DIAGNOSTIC_SHARED_GROUP: &str = "hardened-blikvm";

#[derive(Debug, Error)]
pub enum BrokerServerError {
    #[error("broker configuration is invalid")]
    Config(#[from] BrokerConfigError),
    #[error("broker runtime directory is invalid")]
    InvalidRuntimeDirectory,
    #[error("broker socket path already exists")]
    SocketAlreadyExists,
    #[error("broker I/O failed")]
    Io(#[from] io::Error),
}

#[cfg(feature = "diagnostic-export")]
#[derive(Clone, PartialEq, Eq)]
enum BrokerDiagnosticReadOutcome {
    NoSealed,
    Sealed(BrokerDiagnosticSnapshot),
    CollectorUnavailable,
}

#[cfg(feature = "diagnostic-export")]
trait DiagnosticReader: Send + Sync + 'static {
    fn read_sealed(
        &self,
    ) -> impl Future<Output = Result<BrokerDiagnosticReadOutcome, BrokerErrorCode>> + Send;
}

#[cfg(feature = "diagnostic-export")]
impl DiagnosticReader for DiagnosticExportClient {
    async fn read_sealed(&self) -> Result<BrokerDiagnosticReadOutcome, BrokerErrorCode> {
        match DiagnosticExportClient::read_sealed(self).await {
            Ok(DiagnosticExportReadOutcome::NoSealed) => Ok(BrokerDiagnosticReadOutcome::NoSealed),
            Ok(DiagnosticExportReadOutcome::Sealed(snapshot)) => {
                let binding = snapshot.binding();
                BrokerDiagnosticSnapshot::from_bundle(
                    binding.instance_id(),
                    binding.window_id(),
                    snapshot.omitted_lifecycle_events(),
                    snapshot.bundle(),
                )
                .map(BrokerDiagnosticReadOutcome::Sealed)
                .map_err(|_| BrokerErrorCode::InvalidBackendResponse)
            }
            Ok(DiagnosticExportReadOutcome::CollectorUnavailable) => {
                Ok(BrokerDiagnosticReadOutcome::CollectorUnavailable)
            }
            Err(error) => Err(map_diagnostic_error(error)),
        }
    }
}

#[cfg(feature = "diagnostic-export")]
trait DiagnosticReaderAuthorizer: Send + Sync + 'static {
    fn permits(&self, stream: &UnixStream) -> bool;
}

#[cfg(feature = "diagnostic-export")]
struct RuntimeDiagnosticReaderAuthorizer {
    assignments: Option<DiagnosticPeerAssignments>,
}

#[cfg(feature = "diagnostic-export")]
impl RuntimeDiagnosticReaderAuthorizer {
    fn from_system_accounts() -> Self {
        Self {
            assignments: resolve_diagnostic_assignments(),
        }
    }

    fn permits_values(&self, pid: Option<i32>, uid: u32, gid: u32) -> bool {
        let Some(assignments) = self.assignments else {
            return false;
        };
        let pid = pid.and_then(|pid| u32::try_from(pid).ok());
        assignments.map_peer(KernelPeerCredentials::new(pid, uid, gid))
            == Ok(DiagnosticSource::Server)
    }
}

#[cfg(feature = "diagnostic-export")]
impl DiagnosticReaderAuthorizer for RuntimeDiagnosticReaderAuthorizer {
    fn permits(&self, stream: &UnixStream) -> bool {
        stream.peer_cred().is_ok_and(|credentials| {
            self.permits_values(credentials.pid(), credentials.uid(), credentials.gid())
        })
    }
}

#[cfg(feature = "diagnostic-export")]
fn resolve_diagnostic_assignments() -> Option<DiagnosticPeerAssignments> {
    let mut uids = [0_u32; 6];
    for (index, expected) in DIAGNOSTIC_ACCOUNTS.into_iter().enumerate() {
        let user = User::from_name(expected).ok().flatten()?;
        if user.name != expected {
            return None;
        }
        uids[index] = user.uid.as_raw();
    }
    let group = Group::from_name(DIAGNOSTIC_SHARED_GROUP).ok().flatten()?;
    if group.name != DIAGNOSTIC_SHARED_GROUP {
        return None;
    }
    let assignments = DiagnosticPeerAssignments::new(
        uids[0],
        uids[1],
        uids[2],
        uids[3],
        uids[4],
        uids[5],
        group.gid.as_raw(),
    )
    .ok()?;
    if assignments.map_peer(KernelPeerCredentials::new(
        Some(std::process::id()),
        geteuid().as_raw(),
        getegid().as_raw(),
    )) != Ok(DiagnosticSource::PlatformBroker)
    {
        return None;
    }
    Some(assignments)
}

#[cfg(not(feature = "diagnostic-export"))]
pub async fn run(
    config: BrokerConfig,
    credentials: KvmdCredentials,
) -> Result<(), BrokerServerError> {
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.broker_socket).is_ok() {
        return Err(BrokerServerError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.broker_socket)?;
    fs::set_permissions(&config.broker_socket, Permissions::from_mode(0o660))?;

    let client = Arc::new(
        KvmdReadClient::new(&config.kvmd_socket)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    );
    let credentials = Arc::new(credentials);
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("broker semaphore closed"))?;
        let client = client.clone();
        let credentials = credentials.clone();
        let request_timeout = config.request_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, client, credentials, request_timeout).await;
        });
    }
}

#[cfg(feature = "diagnostic-export")]
pub async fn run(
    config: BrokerConfig,
    credentials: KvmdCredentials,
) -> Result<(), BrokerServerError> {
    run_with_parts(
        config,
        credentials,
        Arc::new(DiagnosticExportClient::new()),
        Arc::new(RuntimeDiagnosticReaderAuthorizer::from_system_accounts()),
    )
    .await
}

#[cfg(feature = "diagnostic-export")]
async fn run_with_parts<D, A>(
    config: BrokerConfig,
    credentials: KvmdCredentials,
    diagnostics: Arc<D>,
    diagnostic_authorizer: Arc<A>,
) -> Result<(), BrokerServerError>
where
    D: DiagnosticReader,
    A: DiagnosticReaderAuthorizer,
{
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.broker_socket).is_ok() {
        return Err(BrokerServerError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.broker_socket)?;
    fs::set_permissions(&config.broker_socket, Permissions::from_mode(0o660))?;

    let client = Arc::new(
        KvmdReadClient::new(&config.kvmd_socket)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    );
    let credentials = Arc::new(credentials);
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("broker semaphore closed"))?;
        let client = client.clone();
        let credentials = credentials.clone();
        let diagnostics = diagnostics.clone();
        let diagnostics_permitted = diagnostic_authorizer.permits(&stream);
        let request_timeout = config.request_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(
                stream,
                client,
                credentials,
                diagnostics,
                diagnostics_permitted,
                request_timeout,
            )
            .await;
        });
    }
}

fn validate_runtime_directory(config: &BrokerConfig) -> Result<(), BrokerServerError> {
    let parent = config
        .broker_socket
        .parent()
        .ok_or(BrokerServerError::InvalidRuntimeDirectory)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.mode() & 0o002 != 0
    {
        return Err(BrokerServerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

#[cfg(not(feature = "diagnostic-export"))]
async fn handle_connection(
    mut stream: UnixStream,
    client: Arc<KvmdReadClient>,
    credentials: Arc<KvmdCredentials>,
    request_timeout: std::time::Duration,
) -> Result<(), io::Error> {
    let request = timeout(request_timeout, read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker request timed out"))??;
    let response = process_request(request, &client, &credentials).await;
    write_response(&mut stream, &response, request_timeout).await
}

#[cfg(feature = "diagnostic-export")]
async fn handle_connection<D>(
    mut stream: UnixStream,
    client: Arc<KvmdReadClient>,
    credentials: Arc<KvmdCredentials>,
    diagnostics: Arc<D>,
    diagnostics_permitted: bool,
    request_timeout: std::time::Duration,
) -> Result<(), io::Error>
where
    D: DiagnosticReader,
{
    let request = timeout(request_timeout, read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker request timed out"))??;
    let response = process_request(
        request,
        &client,
        &credentials,
        diagnostics.as_ref(),
        diagnostics_permitted,
    )
    .await;
    write_response(&mut stream, &response, request_timeout).await
}

async fn write_response(
    stream: &mut UnixStream,
    response: &BrokerResponseEnvelope,
    request_timeout: std::time::Duration,
) -> Result<(), io::Error> {
    let payload = encode_response(response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker response invalid"))?;
    let frame = encode_frame(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker response too large"))?;
    timeout(request_timeout, stream.write_all(&frame))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker response timed out"))??;
    Ok(())
}

async fn read_request(stream: &mut UnixStream) -> Result<BrokerRequestEnvelope, io::Error> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_BROKER_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "broker request too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    decode_request(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker request invalid"))
}

#[cfg(not(feature = "diagnostic-export"))]
async fn process_request(
    request: BrokerRequestEnvelope,
    client: &KvmdReadClient,
    credentials: &KvmdCredentials,
) -> BrokerResponseEnvelope {
    let response = match request.request {
        BrokerReadRequest::Inventory => inventory_response(client, credentials).await,
    };
    BrokerResponseEnvelope {
        schema_version: request.schema_version,
        request_id: request.request_id,
        response,
    }
}

#[cfg(feature = "diagnostic-export")]
async fn process_request<D>(
    request: BrokerRequestEnvelope,
    client: &KvmdReadClient,
    credentials: &KvmdCredentials,
    diagnostics: &D,
    diagnostics_permitted: bool,
) -> BrokerResponseEnvelope
where
    D: DiagnosticReader,
{
    let response = match request.request {
        BrokerReadRequest::Inventory => inventory_response(client, credentials).await,
        BrokerReadRequest::Diagnostics if !diagnostics_permitted => BrokerReadResponse::Error {
            code: BrokerErrorCode::Unauthorized,
        },
        BrokerReadRequest::Diagnostics => match diagnostics.read_sealed().await {
            Ok(BrokerDiagnosticReadOutcome::NoSealed) => {
                BrokerReadResponse::Diagnostics { snapshot: None }
            }
            Ok(BrokerDiagnosticReadOutcome::Sealed(snapshot)) => BrokerReadResponse::Diagnostics {
                snapshot: Some(snapshot),
            },
            Ok(BrokerDiagnosticReadOutcome::CollectorUnavailable) => BrokerReadResponse::Error {
                code: BrokerErrorCode::BackendUnavailable,
            },
            Err(code) => BrokerReadResponse::Error { code },
        },
    };
    BrokerResponseEnvelope {
        schema_version: request.schema_version,
        request_id: request.request_id,
        response,
    }
}

async fn inventory_response(
    client: &KvmdReadClient,
    credentials: &KvmdCredentials,
) -> BrokerReadResponse {
    match client.inventory(credentials).await {
        Ok(source) => {
            let thermal = match read_h616_temperature_millidegrees() {
                Ok(value) => Some(value),
                Err(error) => {
                    eprintln!("H616 thermal observation failed with {}", error.class());
                    None
                }
            };
            let external_rtc = match read_h616_external_rtc() {
                Ok(value) => Some(value),
                Err(error) => {
                    eprintln!(
                        "H616 external RTC observation failed with {}",
                        error.class()
                    );
                    None
                }
            };
            BrokerReadResponse::Inventory {
                inventory: sanitize_kvmd_inventory_with_observations(
                    source,
                    BliPlatformObservations {
                        temperature_millidegrees_celsius: thermal,
                        external_rtc,
                    },
                ),
            }
        }
        Err(error) => {
            let code = map_client_error(&error.source);
            eprintln!(
                "KVMD inventory request failed with {code:?} at {} with {}",
                error.endpoint.as_str(),
                client_error_class(&error.source),
            );
            BrokerReadResponse::Error { code }
        }
    }
}

fn client_error_class(error: &KvmdClientError) -> String {
    match error {
        KvmdClientError::Status(status) => format!("status_{}", status.as_u16()),
        KvmdClientError::Io(_) => "io".into(),
        KvmdClientError::Timeout => "timeout".into(),
        KvmdClientError::Hyper(_) => "hyper".into(),
        KvmdClientError::ResponseTooLarge => "response_too_large".into(),
        KvmdClientError::InvalidJson(_) => "invalid_json".into(),
        KvmdClientError::ApiRejected => "api_rejected".into(),
        KvmdClientError::MissingResult => "missing_result".into(),
        KvmdClientError::RelativeSocketPath => "relative_socket_path".into(),
        KvmdClientError::InvalidResponseLimit => "invalid_response_limit".into(),
        KvmdClientError::Http(_) => "http_build".into(),
    }
}

fn map_client_error(error: &KvmdClientError) -> BrokerErrorCode {
    match error {
        KvmdClientError::Status(status) if matches!(status.as_u16(), 401 | 403) => {
            BrokerErrorCode::BackendUnauthorized
        }
        KvmdClientError::Io(_) | KvmdClientError::Timeout => BrokerErrorCode::BackendUnavailable,
        KvmdClientError::Status(_)
        | KvmdClientError::Hyper(_)
        | KvmdClientError::ResponseTooLarge
        | KvmdClientError::InvalidJson(_)
        | KvmdClientError::ApiRejected
        | KvmdClientError::MissingResult => BrokerErrorCode::InvalidBackendResponse,
        KvmdClientError::RelativeSocketPath
        | KvmdClientError::InvalidResponseLimit
        | KvmdClientError::Http(_) => BrokerErrorCode::Internal,
    }
}

#[cfg(feature = "diagnostic-export")]
fn map_diagnostic_error(error: DiagnosticExportReadError) -> BrokerErrorCode {
    match error {
        DiagnosticExportReadError::NotSent {
            reason:
                DiagnosticExportNotSentReason::ConnectTimeout
                | DiagnosticExportNotSentReason::ConnectFailed,
        }
        | DiagnosticExportReadError::Retryable {
            stage:
                DiagnosticExportExchangeStage::WriteTimeout
                | DiagnosticExportExchangeStage::WriteFailed
                | DiagnosticExportExchangeStage::ResponseTimeout
                | DiagnosticExportExchangeStage::ResponseClosed
                | DiagnosticExportExchangeStage::ResponseReadFailed,
        } => BrokerErrorCode::BackendUnavailable,
        DiagnosticExportReadError::Retryable {
            stage:
                DiagnosticExportExchangeStage::ResponseTooLarge
                | DiagnosticExportExchangeStage::InvalidResponse,
        } => BrokerErrorCode::InvalidBackendResponse,
        DiagnosticExportReadError::NotSent {
            reason: DiagnosticExportNotSentReason::InvalidBinding,
        } => BrokerErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "diagnostic-export")]
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use std::{path::Path, time::Duration};

    #[cfg(feature = "diagnostic-export")]
    use bli_kvm_broker_protocol::BrokerDiagnosticSnapshot;
    use bli_kvm_broker_protocol::{BrokerReadResponse, decode_response, encode_frame};
    #[cfg(feature = "diagnostic-export")]
    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };
    use tokio::time::sleep;

    use super::*;

    #[cfg(feature = "diagnostic-export")]
    #[derive(Clone)]
    struct MockDiagnosticReader {
        outcome: Result<BrokerDiagnosticReadOutcome, BrokerErrorCode>,
        calls: Arc<AtomicUsize>,
    }

    #[cfg(feature = "diagnostic-export")]
    impl MockDiagnosticReader {
        fn new(outcome: Result<BrokerDiagnosticReadOutcome, BrokerErrorCode>) -> Self {
            Self {
                outcome,
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[cfg(feature = "diagnostic-export")]
    impl DiagnosticReader for MockDiagnosticReader {
        async fn read_sealed(&self) -> Result<BrokerDiagnosticReadOutcome, BrokerErrorCode> {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.outcome.clone()
        }
    }

    #[cfg(feature = "diagnostic-export")]
    struct FixedDiagnosticReaderAuthorizer(bool);

    #[cfg(feature = "diagnostic-export")]
    impl DiagnosticReaderAuthorizer for FixedDiagnosticReaderAuthorizer {
        fn permits(&self, _stream: &UnixStream) -> bool {
            self.0
        }
    }

    #[cfg(feature = "diagnostic-export")]
    fn diagnostic_snapshot() -> BrokerDiagnosticSnapshot {
        let mut exporter = DiagnosticExporter::new();
        exporter
            .push(
                DiagnosticRecord::new(
                    7,
                    DiagnosticComponent::ControlBroker,
                    DiagnosticEvent::Ready,
                )
                .unwrap(),
            )
            .unwrap();
        BrokerDiagnosticSnapshot::from_bundle(9, 4, 1, &exporter.export().unwrap()).unwrap()
    }

    #[cfg(feature = "diagnostic-export")]
    fn diagnostic_request(request_id: u64) -> BrokerRequestEnvelope {
        BrokerRequestEnvelope {
            schema_version: bli_kvm_broker_protocol::BROKER_DIAGNOSTIC_SCHEMA_VERSION,
            request_id,
            request: BrokerReadRequest::Diagnostics,
        }
    }

    #[cfg(feature = "diagnostic-export")]
    fn unused_kvmd_client() -> KvmdReadClient {
        KvmdReadClient::new("/tmp/hardened-blikvm-unused-kvmd.sock").unwrap()
    }

    #[cfg(feature = "diagnostic-export")]
    fn test_credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    #[cfg(feature = "diagnostic-export")]
    async fn broker_round_trip(
        socket: &Path,
        request: BrokerRequestEnvelope,
    ) -> BrokerResponseEnvelope {
        let payload = bli_kvm_broker_protocol::encode_request(&request).unwrap();
        let mut stream = UnixStream::connect(socket).await.unwrap();
        stream
            .write_all(&encode_frame(&payload).unwrap())
            .await
            .unwrap();
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let mut response = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut response).await.unwrap();
        decode_response(&response).unwrap()
    }

    #[test]
    fn maps_backend_errors_without_messages_or_payloads() {
        assert_eq!(
            map_client_error(&KvmdClientError::Status(http::StatusCode::UNAUTHORIZED)),
            BrokerErrorCode::BackendUnauthorized
        );
        assert_eq!(
            map_client_error(&KvmdClientError::ResponseTooLarge),
            BrokerErrorCode::InvalidBackendResponse
        );
        assert_eq!(
            client_error_class(&KvmdClientError::Status(http::StatusCode::BAD_REQUEST)),
            "status_400"
        );
        #[cfg(feature = "diagnostic-export")]
        {
            assert_eq!(
                map_diagnostic_error(DiagnosticExportReadError::Retryable {
                    stage: DiagnosticExportExchangeStage::InvalidResponse,
                }),
                BrokerErrorCode::InvalidBackendResponse
            );
            assert_eq!(
                map_diagnostic_error(DiagnosticExportReadError::NotSent {
                    reason: DiagnosticExportNotSentReason::ConnectFailed,
                }),
                BrokerErrorCode::BackendUnavailable
            );
        }
    }

    #[cfg(feature = "diagnostic-export")]
    #[tokio::test]
    async fn diagnostics_require_exact_authorization_and_never_acknowledge() {
        let diagnostics = MockDiagnosticReader::new(Ok(BrokerDiagnosticReadOutcome::Sealed(
            diagnostic_snapshot(),
        )));
        let client = unused_kvmd_client();
        let credentials = test_credentials();

        let denied = process_request(
            diagnostic_request(10),
            &client,
            &credentials,
            &diagnostics,
            false,
        )
        .await;
        assert_eq!(
            denied.response,
            BrokerReadResponse::Error {
                code: BrokerErrorCode::Unauthorized,
            }
        );
        assert_eq!(diagnostics.calls.load(Ordering::Acquire), 0);

        let allowed = process_request(
            diagnostic_request(11),
            &client,
            &credentials,
            &diagnostics,
            true,
        )
        .await;
        let BrokerReadResponse::Diagnostics {
            snapshot: Some(snapshot),
        } = allowed.response
        else {
            panic!("expected diagnostic snapshot");
        };
        assert_eq!(snapshot.instance_id(), 9);
        assert_eq!(snapshot.window_id(), 4);
        assert_eq!(snapshot.record_count(), 1);
        assert_eq!(diagnostics.calls.load(Ordering::Acquire), 1);
    }

    #[cfg(feature = "diagnostic-export")]
    #[tokio::test]
    async fn diagnostic_absence_and_failures_are_typed_and_inventory_independent() {
        let client = unused_kvmd_client();
        let credentials = test_credentials();
        let none = MockDiagnosticReader::new(Ok(BrokerDiagnosticReadOutcome::NoSealed));
        assert_eq!(
            process_request(diagnostic_request(12), &client, &credentials, &none, true,)
                .await
                .response,
            BrokerReadResponse::Diagnostics { snapshot: None }
        );

        let unavailable =
            MockDiagnosticReader::new(Ok(BrokerDiagnosticReadOutcome::CollectorUnavailable));
        assert_eq!(
            process_request(
                diagnostic_request(13),
                &client,
                &credentials,
                &unavailable,
                true,
            )
            .await
            .response,
            BrokerReadResponse::Error {
                code: BrokerErrorCode::BackendUnavailable,
            }
        );

        let invalid = MockDiagnosticReader::new(Err(BrokerErrorCode::InvalidBackendResponse));
        assert_eq!(
            process_request(
                diagnostic_request(14),
                &client,
                &credentials,
                &invalid,
                true,
            )
            .await
            .response,
            BrokerReadResponse::Error {
                code: BrokerErrorCode::InvalidBackendResponse,
            }
        );
    }

    #[cfg(feature = "diagnostic-export")]
    #[tokio::test]
    async fn exact_peer_reads_same_unacknowledged_snapshot_over_real_broker_socket() {
        let directory = tempfile::tempdir().unwrap();
        let broker_socket = directory.path().join("broker.sock");
        let config = BrokerConfig::new(
            &broker_socket,
            directory.path().join("unused-kvmd.sock"),
            directory.path(),
        )
        .unwrap();
        let diagnostics = Arc::new(MockDiagnosticReader::new(Ok(
            BrokerDiagnosticReadOutcome::Sealed(diagnostic_snapshot()),
        )));
        let authorizer = Arc::new(FixedDiagnosticReaderAuthorizer(true));
        let broker = tokio::spawn(run_with_parts(
            config,
            test_credentials(),
            diagnostics.clone(),
            authorizer,
        ));
        for _ in 0..100 {
            if broker_socket.exists() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        assert!(broker_socket.exists());

        for request_id in [20, 21] {
            let response = broker_round_trip(&broker_socket, diagnostic_request(request_id)).await;
            assert_eq!(response.request_id, request_id);
            let BrokerReadResponse::Diagnostics {
                snapshot: Some(snapshot),
            } = response.response
            else {
                panic!("expected diagnostic snapshot");
            };
            assert_eq!(snapshot.instance_id(), 9);
            assert_eq!(snapshot.window_id(), 4);
        }
        assert_eq!(diagnostics.calls.load(Ordering::Acquire), 2);
        broker.abort();
    }

    #[cfg(feature = "diagnostic-export")]
    #[tokio::test]
    async fn runtime_authorizer_requires_exact_nonroot_uid_gid_and_live_pid() {
        let assignments =
            DiagnosticPeerAssignments::new(100, 101, 102, 103, 104, 105, 200).unwrap();
        let authorizer = RuntimeDiagnosticReaderAuthorizer {
            assignments: Some(assignments),
        };
        assert!(authorizer.permits_values(Some(7), 101, 200));
        assert!(!authorizer.permits_values(Some(7), 102, 200));
        assert!(!authorizer.permits_values(Some(7), 101, 201));
        assert!(!authorizer.permits_values(None, 101, 200));
        assert!(!authorizer.permits_values(Some(0), 101, 200));
        assert!(!authorizer.permits_values(Some(7), 0, 200));

        let (first, _second) = UnixStream::pair().unwrap();
        assert!(!authorizer.permits(&first));
        let unavailable = RuntimeDiagnosticReaderAuthorizer { assignments: None };
        assert!(!unavailable.permits_values(Some(7), 101, 200));
    }

    async fn mock_kvmd(socket: &Path) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let body = match path {
                    "/info" => {
                        r#"{"ok":true,"result":{"system":{"kvmd":{"version":"3.291"},"streamer":{"version":"6.11"}},"extras":{"janus":{"enabled":false}}}}"#
                    }
                    "/atx" => {
                        r#"{"ok":true,"result":{"enabled":true,"busy":false,"leds":{"power":true}}}"#
                    }
                    "/hid" => {
                        r#"{"ok":true,"result":{"online":true,"busy":false,"mouse":{"online":true}}}"#
                    }
                    "/msd" => {
                        r#"{"ok":true,"result":{"enabled":true,"online":true,"busy":false,"drive":{"connected":false}}}"#
                    }
                    "/streamer" => {
                        r#"{"ok":true,"result":{"streamer":{"source":{"online":true}}}}"#
                    }
                    _ => panic!("unexpected read-only KVMD path"),
                };
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("connection: close\r\n")
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: keep-alive\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                let mut trailing = [0_u8; 1];
                assert_eq!(stream.read(&mut trailing).await.unwrap(), 0);
            }
        })
    }

    #[tokio::test]
    async fn broker_exposes_only_sanitized_inventory_end_to_end() {
        let directory = tempfile::tempdir().unwrap();
        let kvmd_socket = directory.path().join("kvmd.sock");
        let broker_socket = directory.path().join("broker.sock");
        let backend = mock_kvmd(&kvmd_socket).await;
        let config = BrokerConfig::new(&broker_socket, &kvmd_socket, directory.path()).unwrap();
        let broker = tokio::spawn(run(
            config,
            KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
        ));

        for _ in 0..100 {
            if broker_socket.exists() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        assert!(broker_socket.exists());

        let payload = br#"{"schemaVersion":1,"requestId":42,"request":{"type":"inventory"}}"#;
        let mut stream = UnixStream::connect(&broker_socket).await.unwrap();
        stream
            .write_all(&encode_frame(payload).unwrap())
            .await
            .unwrap();
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let mut response = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut response).await.unwrap();
        let response = decode_response(&response).unwrap();
        assert_eq!(response.request_id, 42);
        let inventory = match response.response {
            BrokerReadResponse::Inventory { inventory } => inventory,
            #[cfg(feature = "diagnostic-export")]
            BrokerReadResponse::Diagnostics { .. } => {
                panic!("inventory request returned diagnostics")
            }
            BrokerReadResponse::Error { code } => {
                panic!("expected inventory response, received {code:?}")
            }
        };
        assert_eq!(inventory.video.online, Some(true));
        assert_eq!(inventory.hid.online, Some(true));
        assert_eq!(inventory.power_led, Some(true));
        assert_eq!(inventory.virtual_media.connected, Some(false));
        assert_eq!(inventory.janus_enabled, Some(false));

        backend.await.unwrap();
        broker.abort();
    }
}
