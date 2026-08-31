use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    sync::Arc,
};

use bli_kvm_control_protocol::{
    CONTROL_SCHEMA_VERSION, ControlRejectionCode, ControlRequestEnvelope, ControlResponse,
    ControlResponseEnvelope, MAX_CONTROL_MESSAGE_BYTES, decode_request, encode_frame,
    encode_response,
};
use bli_kvm_diagnostic_producer_client::{
    DiagnosticReportReceipt, DiagnosticReporter, DiagnosticReporterHandle,
};
use bli_kvm_diagnostic_protocol::{DiagnosticEvent, RejectReason};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixListener,
    sync::Semaphore,
    time::timeout,
};

use crate::{ControlBrokerConfig, ControlBrokerConfigError};

#[derive(Debug, Error)]
pub enum ControlBrokerError {
    #[error("control broker configuration is invalid")]
    Config(#[from] ControlBrokerConfigError),
    #[error("control broker runtime directory is invalid")]
    InvalidRuntimeDirectory,
    #[error("control broker socket already exists")]
    SocketAlreadyExists,
    #[error("control broker I/O failed")]
    Io(#[from] io::Error),
}

pub async fn run(config: ControlBrokerConfig) -> Result<(), ControlBrokerError> {
    let diagnostic_reporter = DiagnosticReporter::start().ok();
    let diagnostics = diagnostic_reporter
        .as_ref()
        .map(|reporter| ControlDiagnostics::Reporter(reporter.handle()))
        .unwrap_or(ControlDiagnostics::Disabled);
    let result = run_with_diagnostics(config, diagnostics).await;
    drop(diagnostic_reporter);
    result
}

async fn run_with_diagnostics<D>(
    config: ControlBrokerConfig,
    diagnostics: D,
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

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("control broker semaphore closed"))?;
        let request_timeout = config.request_timeout;
        let diagnostics = diagnostics.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, request_timeout, diagnostics).await;
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
) -> Result<(), io::Error>
where
    S: AsyncRead + AsyncWrite + Unpin,
    D: DiagnosticSubmitter,
{
    let request = timeout(request_timeout, read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control request timed out"))??;
    let response = deny_request(request);
    let payload = encode_response(&response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control response invalid"))?;
    timeout(
        request_timeout,
        stream.write_all(&encode_frame(&payload).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "control response too large")
        })?),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control response timed out"))??;
    let _ = diagnostics.try_report(DiagnosticEvent::RequestRejected {
        reason: RejectReason::CapabilityDisabled,
    });
    Ok(())
}

async fn read_request<S>(stream: &mut S) -> Result<ControlRequestEnvelope, io::Error>
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
    decode_request(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control request invalid"))
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
    use bli_kvm_control_protocol::{decode_response, encode_request};
    use bli_kvm_core::{AtxControlAction, HidReport, PlatformControlRequest, VirtualMediaCommand};
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
        let handler = tokio::spawn(handle_connection(server, TEST_WAIT, diagnostics));
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
        let handler = tokio::spawn(handle_connection(server, TEST_WAIT, diagnostics.clone()));
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
