#![forbid(unsafe_code)]

//! Caller-bound authenticated listener for ephemeral sealed diagnostic windows.
//!
//! The listener owns no socket path, account, persistence, client, HTTP route,
//! or retry policy. It authenticates only the fixed platform-broker role,
//! handles one fixed request/response per connection, and releases the shared
//! collector runtime before bounded response I/O.

use std::{
    error::Error,
    fmt,
    future::Future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use bli_kvm_diagnostic_export::DiagnosticBundle;
use bli_kvm_diagnostic_export_protocol::{
    DIAGNOSTIC_EXPORT_REQUEST_BYTES, DiagnosticExportRequest, DiagnosticExportResponse,
    DiagnosticExportResponseFrame, decode_request, encode_response,
};
use bli_kvm_diagnostic_peer_adapter::{
    DiagnosticCollectorRuntime, DiagnosticExportConsumerAuthenticator, DiagnosticListenerError,
    DiagnosticPeerAdapterError, DiagnosticRuntimeError,
};
use bli_kvm_diagnostic_window_policy::{DiagnosticWindowError, DiagnosticWindowSnapshot};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::{JoinError, JoinSet},
    time::timeout,
};

pub const DIAGNOSTIC_EXPORT_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
pub const DIAGNOSTIC_EXPORT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);
pub const DIAGNOSTIC_EXPORT_MAX_CONNECTIONS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportListenerError {
    AccountResolution(DiagnosticPeerAdapterError),
    AlreadyStarted,
    AcceptFailed,
    ConnectionTaskFailed,
}

impl fmt::Display for DiagnosticExportListenerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::AccountResolution(_) => "diagnostic export account resolution failed",
            Self::AlreadyStarted => "diagnostic export listener was already started",
            Self::AcceptFailed => "diagnostic export accept failed",
            Self::ConnectionTaskFailed => "diagnostic export connection task failed",
        })
    }
}

impl Error for DiagnosticExportListenerError {}

impl From<DiagnosticPeerAdapterError> for DiagnosticExportListenerError {
    fn from(error: DiagnosticPeerAdapterError) -> Self {
        Self::AccountResolution(error)
    }
}

/// Single-use owner for one caller-bound export listener.
///
/// It intentionally implements neither `Clone` nor `Debug`.
pub struct DiagnosticExportListener {
    state: Arc<RuntimeExportState>,
    authenticator: Arc<RuntimeAuthenticator>,
    limits: ListenerLimits,
    started: AtomicBool,
}

impl DiagnosticExportListener {
    pub fn from_current_process(
        runtime: Arc<DiagnosticCollectorRuntime>,
    ) -> Result<Self, DiagnosticExportListenerError> {
        Ok(Self {
            state: Arc::new(RuntimeExportState { runtime }),
            authenticator: Arc::new(RuntimeAuthenticator {
                inner: DiagnosticExportConsumerAuthenticator::from_current_process()?,
            }),
            limits: ListenerLimits::PRODUCTION,
            started: AtomicBool::new(false),
        })
    }

    pub async fn serve_until<F>(
        &self,
        listener: UnixListener,
        shutdown: F,
    ) -> Result<(), DiagnosticExportListenerError>
    where
        F: Future<Output = ()> + Send,
    {
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DiagnosticExportListenerError::AlreadyStarted);
        }
        serve_with_parts(
            listener,
            Arc::clone(&self.state),
            Arc::clone(&self.authenticator),
            self.limits,
            shutdown,
        )
        .await
    }
}

#[derive(Clone, Copy)]
struct ListenerLimits {
    request_timeout: Duration,
    response_timeout: Duration,
    max_connections: usize,
}

impl ListenerLimits {
    const PRODUCTION: Self = Self {
        request_timeout: DIAGNOSTIC_EXPORT_REQUEST_TIMEOUT,
        response_timeout: DIAGNOSTIC_EXPORT_RESPONSE_TIMEOUT,
        max_connections: DIAGNOSTIC_EXPORT_MAX_CONNECTIONS,
    };

    const fn valid(self) -> bool {
        !self.request_timeout.is_zero()
            && !self.response_timeout.is_zero()
            && self.max_connections != 0
    }
}

trait ConsumerAuthenticator: Send + Sync + 'static {
    fn permits(&self, stream: &UnixStream) -> bool;
}

struct RuntimeAuthenticator {
    inner: DiagnosticExportConsumerAuthenticator,
}

impl ConsumerAuthenticator for RuntimeAuthenticator {
    fn permits(&self, stream: &UnixStream) -> bool {
        self.inner.authenticate(stream).is_ok()
    }
}

#[derive(Clone)]
struct ExportWindow {
    instance_id: u64,
    window_id: u32,
    omitted_lifecycle_events: u8,
    bundle: DiagnosticBundle,
}

impl From<DiagnosticWindowSnapshot> for ExportWindow {
    fn from(snapshot: DiagnosticWindowSnapshot) -> Self {
        let (instance_id, window_id, omitted_lifecycle_events, bundle) = snapshot.into_parts();
        Self {
            instance_id,
            window_id,
            omitted_lifecycle_events,
            bundle,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcknowledgementOutcome {
    Applied,
    NoSealed,
    Mismatch,
    Unavailable,
}

trait ExportState: Send + Sync + 'static {
    fn sealed_window(&self) -> impl Future<Output = Result<Option<ExportWindow>, ()>> + Send;

    fn acknowledge(
        &self,
        instance_id: u64,
        window_id: u32,
    ) -> impl Future<Output = AcknowledgementOutcome> + Send;
}

struct RuntimeExportState {
    runtime: Arc<DiagnosticCollectorRuntime>,
}

impl ExportState for RuntimeExportState {
    async fn sealed_window(&self) -> Result<Option<ExportWindow>, ()> {
        self.runtime
            .sealed_window()
            .await
            .map(|window| window.map(Into::into))
            .map_err(|_| ())
    }

    async fn acknowledge(&self, instance_id: u64, window_id: u32) -> AcknowledgementOutcome {
        map_acknowledgement(
            self.runtime
                .acknowledge_sealed(instance_id, window_id)
                .await,
        )
    }
}

fn map_acknowledgement(result: Result<(), DiagnosticListenerError>) -> AcknowledgementOutcome {
    match result {
        Ok(()) => AcknowledgementOutcome::Applied,
        Err(DiagnosticListenerError::Runtime(DiagnosticRuntimeError::Window(
            DiagnosticWindowError::NoSealedWindow,
        ))) => AcknowledgementOutcome::NoSealed,
        Err(DiagnosticListenerError::Runtime(DiagnosticRuntimeError::Window(
            DiagnosticWindowError::AcknowledgementMismatch,
        ))) => AcknowledgementOutcome::Mismatch,
        Err(_) => AcknowledgementOutcome::Unavailable,
    }
}

async fn serve_with_parts<S, A, F>(
    listener: UnixListener,
    state: Arc<S>,
    authenticator: Arc<A>,
    limits: ListenerLimits,
    shutdown: F,
) -> Result<(), DiagnosticExportListenerError>
where
    S: ExportState,
    A: ConsumerAuthenticator,
    F: Future<Output = ()> + Send,
{
    debug_assert!(limits.valid());
    let permits = Arc::new(Semaphore::new(limits.max_connections));
    let mut connections = JoinSet::new();
    tokio::pin!(shutdown);
    let mut result = Ok(());

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => break,
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(joined) = joined {
                    if connection_task_result(joined).is_err() {
                        result = Err(DiagnosticExportListenerError::ConnectionTaskFailed);
                        break;
                    }
                }
            }
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(_) => {
                        result = Err(DiagnosticExportListenerError::AcceptFailed);
                        break;
                    }
                };
                if !authenticator.permits(&stream) {
                    continue;
                }
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else {
                    continue;
                };
                let state = Arc::clone(&state);
                connections.spawn(async move {
                    let _permit = permit;
                    handle_connection(stream, state, limits).await;
                });
            }
        }
    }

    while let Some(joined) = connections.join_next().await {
        if result.is_ok() && connection_task_result(joined).is_err() {
            result = Err(DiagnosticExportListenerError::ConnectionTaskFailed);
        }
    }
    result
}

fn connection_task_result(result: Result<(), JoinError>) -> Result<(), ()> {
    result.map_err(|_| ())
}

async fn handle_connection<IO, S>(mut stream: IO, state: Arc<S>, limits: ListenerLimits)
where
    IO: AsyncRead + AsyncWrite + Unpin,
    S: ExportState,
{
    let mut bytes = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
    if timeout(limits.request_timeout, stream.read_exact(&mut bytes))
        .await
        .ok()
        .and_then(Result::ok)
        .is_none()
    {
        return;
    }
    let Ok(request) = decode_request(&bytes) else {
        return;
    };
    let Some(frame) = response_for_request(state.as_ref(), request).await else {
        return;
    };
    let _ = timeout(limits.response_timeout, stream.write_all(frame.as_bytes())).await;
}

async fn response_for_request<S>(
    state: &S,
    request: DiagnosticExportRequest,
) -> Option<DiagnosticExportResponseFrame>
where
    S: ExportState,
{
    match request {
        DiagnosticExportRequest::ReadSealed => match state.sealed_window().await {
            Ok(Some(window)) => encode_or_unavailable(DiagnosticExportResponse::Sealed {
                instance_id: window.instance_id,
                window_id: window.window_id,
                omitted_lifecycle_events: window.omitted_lifecycle_events,
                bundle: &window.bundle,
            }),
            Ok(None) => encode_or_unavailable(DiagnosticExportResponse::NoSealed),
            Err(()) => encode_or_unavailable(DiagnosticExportResponse::CollectorUnavailable),
        },
        DiagnosticExportRequest::Acknowledge {
            instance_id,
            window_id,
        } => {
            let response = match state.acknowledge(instance_id, window_id).await {
                AcknowledgementOutcome::Applied => DiagnosticExportResponse::Acknowledged {
                    instance_id,
                    window_id,
                },
                AcknowledgementOutcome::NoSealed => DiagnosticExportResponse::NoSealed,
                AcknowledgementOutcome::Mismatch => {
                    DiagnosticExportResponse::AcknowledgementMismatch {
                        instance_id,
                        window_id,
                    }
                }
                AcknowledgementOutcome::Unavailable => {
                    DiagnosticExportResponse::CollectorUnavailable
                }
            };
            // An applied acknowledgement must never be rewritten as an explicit
            // unavailable response. If fixed response encoding ever fails,
            // closing the connection preserves delivery ambiguity for the client.
            encode_response(response).ok()
        }
    }
}

fn encode_or_unavailable(
    response: DiagnosticExportResponse<'_>,
) -> Option<DiagnosticExportResponseFrame> {
    encode_response(response)
        .or_else(|_| encode_response(DiagnosticExportResponse::CollectorUnavailable))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::{
        io::ErrorKind,
        sync::{Mutex as StdMutex, atomic::AtomicUsize},
    };

    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };
    use bli_kvm_diagnostic_export_protocol::{
        DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES, DecodedDiagnosticExportResponse,
        MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES, decode_response, encode_request,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex},
        sync::oneshot,
        time::{sleep, timeout},
    };

    use super::*;

    const TEST_LIMIT: Duration = Duration::from_millis(30);
    const TEST_WAIT: Duration = Duration::from_millis(500);

    fn limits() -> ListenerLimits {
        ListenerLimits {
            request_timeout: TEST_LIMIT,
            response_timeout: TEST_LIMIT,
            max_connections: 2,
        }
    }

    fn window() -> ExportWindow {
        let mut exporter = DiagnosticExporter::new();
        exporter
            .push(
                DiagnosticRecord::new(
                    9,
                    DiagnosticComponent::ControlBroker,
                    DiagnosticEvent::Ready,
                )
                .unwrap(),
            )
            .unwrap();
        ExportWindow {
            instance_id: 7,
            window_id: 3,
            omitted_lifecycle_events: 1,
            bundle: exporter.export().unwrap(),
        }
    }

    #[derive(Clone)]
    struct MockState {
        inner: Arc<StdMutex<MockStateInner>>,
        sealed_calls: Arc<AtomicUsize>,
    }

    struct MockStateInner {
        window: Result<Option<ExportWindow>, ()>,
        acknowledgement: AcknowledgementOutcome,
        acknowledged_binding: Option<(u64, u32)>,
    }

    impl MockState {
        fn new(
            window: Result<Option<ExportWindow>, ()>,
            acknowledgement: AcknowledgementOutcome,
        ) -> Self {
            Self {
                inner: Arc::new(StdMutex::new(MockStateInner {
                    window,
                    acknowledgement,
                    acknowledged_binding: None,
                })),
                sealed_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    impl ExportState for MockState {
        async fn sealed_window(&self) -> Result<Option<ExportWindow>, ()> {
            self.sealed_calls.fetch_add(1, Ordering::AcqRel);
            self.inner.lock().unwrap().window.clone()
        }

        async fn acknowledge(&self, instance_id: u64, window_id: u32) -> AcknowledgementOutcome {
            let mut inner = self.inner.lock().unwrap();
            inner.acknowledged_binding = Some((instance_id, window_id));
            inner.acknowledgement
        }
    }

    async fn write_request(stream: &mut DuplexStream, request: DiagnosticExportRequest) {
        stream
            .write_all(&encode_request(request).unwrap())
            .await
            .unwrap();
    }

    async fn read_response(stream: &mut DuplexStream) -> Vec<u8> {
        let mut header = [0_u8; DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES];
        stream.read_exact(&mut header).await.unwrap();
        let length = usize::from(u16::from_be_bytes([header[16], header[17]]));
        let mut response = Vec::with_capacity(header.len() + length);
        response.extend_from_slice(&header);
        response.resize(header.len() + length, 0);
        stream
            .read_exact(&mut response[header.len()..])
            .await
            .unwrap();
        response
    }

    fn closed_without_response(result: std::io::Result<usize>) -> bool {
        matches!(result, Ok(0))
            || matches!(
                result,
                Err(error)
                    if matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe)
            )
    }

    async fn exchange(state: MockState, request: DiagnosticExportRequest) -> Vec<u8> {
        let (server, mut client) = duplex(MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES * 2);
        let handler = tokio::spawn(handle_connection(server, Arc::new(state), limits()));
        write_request(&mut client, request).await;
        let response = timeout(TEST_WAIT, read_response(&mut client))
            .await
            .unwrap();
        timeout(TEST_WAIT, handler).await.unwrap().unwrap();
        response
    }

    #[tokio::test]
    async fn read_returns_only_the_bounded_sealed_snapshot() {
        let expected = window();
        let response = exchange(
            MockState::new(Ok(Some(expected.clone())), AcknowledgementOutcome::Applied),
            DiagnosticExportRequest::ReadSealed,
        )
        .await;
        match decode_response(&response).unwrap() {
            DecodedDiagnosticExportResponse::Sealed {
                instance_id,
                window_id,
                omitted_lifecycle_events,
                record_count,
                payload,
            } => {
                assert_eq!(instance_id, expected.instance_id);
                assert_eq!(window_id, expected.window_id);
                assert_eq!(omitted_lifecycle_events, 1);
                assert_eq!(record_count, 1);
                assert_eq!(payload, expected.bundle.as_bytes());
            }
            _ => panic!("unexpected response"),
        }
    }

    #[tokio::test]
    async fn connection_closes_after_one_response() {
        let state = MockState::new(Ok(None), AcknowledgementOutcome::Applied);
        let (server, mut client) = duplex(128);
        let handler = tokio::spawn(handle_connection(server, Arc::new(state), limits()));
        write_request(&mut client, DiagnosticExportRequest::ReadSealed).await;
        let response = read_response(&mut client).await;
        assert!(matches!(
            decode_response(&response),
            Ok(DecodedDiagnosticExportResponse::NoSealed)
        ));
        handler.await.unwrap();
        let mut probe = [0_u8; 1];
        assert_eq!(client.read(&mut probe).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn every_read_and_acknowledgement_outcome_is_typed() {
        for (state, request, expected_tag) in [
            (
                MockState::new(Ok(None), AcknowledgementOutcome::Applied),
                DiagnosticExportRequest::ReadSealed,
                1,
            ),
            (
                MockState::new(Err(()), AcknowledgementOutcome::Applied),
                DiagnosticExportRequest::ReadSealed,
                5,
            ),
            (
                MockState::new(Ok(None), AcknowledgementOutcome::Applied),
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                },
                3,
            ),
            (
                MockState::new(Ok(None), AcknowledgementOutcome::NoSealed),
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                },
                1,
            ),
            (
                MockState::new(Ok(None), AcknowledgementOutcome::Mismatch),
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                },
                4,
            ),
            (
                MockState::new(Ok(None), AcknowledgementOutcome::Unavailable),
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                },
                5,
            ),
        ] {
            let response = exchange(state.clone(), request).await;
            assert_eq!(response[1], expected_tag);
            if matches!(request, DiagnosticExportRequest::Acknowledge { .. }) {
                assert_eq!(
                    state.inner.lock().unwrap().acknowledged_binding,
                    Some((7, 3))
                );
            }
        }
    }

    #[tokio::test]
    async fn malformed_and_partial_requests_receive_no_response() {
        let state = MockState::new(Ok(Some(window())), AcknowledgementOutcome::Applied);
        let (server, mut client) = duplex(128);
        let handler = tokio::spawn(handle_connection(server, Arc::new(state.clone()), limits()));
        let mut invalid = encode_request(DiagnosticExportRequest::ReadSealed).unwrap();
        invalid[2] = 1;
        client.write_all(&invalid).await.unwrap();
        client.shutdown().await.unwrap();
        let mut probe = [0_u8; 1];
        assert_eq!(client.read(&mut probe).await.unwrap(), 0);
        handler.await.unwrap();
        assert_eq!(state.sealed_calls.load(Ordering::Acquire), 0);

        let state = MockState::new(Ok(Some(window())), AcknowledgementOutcome::Applied);
        let (server, mut client) = duplex(128);
        let started = tokio::time::Instant::now();
        let handler = tokio::spawn(handle_connection(server, Arc::new(state.clone()), limits()));
        client.write_all(&[1]).await.unwrap();
        assert!(closed_without_response(
            timeout(TEST_WAIT, client.read(&mut probe)).await.unwrap()
        ));
        assert!(started.elapsed() >= TEST_LIMIT);
        handler.await.unwrap();
        assert_eq!(state.sealed_calls.load(Ordering::Acquire), 0);
    }

    #[tokio::test]
    async fn stalled_response_is_bounded_after_state_is_released() {
        let state = MockState::new(Ok(Some(window())), AcknowledgementOutcome::Applied);
        let (server, mut client) = duplex(1);
        let handler = tokio::spawn(handle_connection(server, Arc::new(state.clone()), limits()));
        write_request(&mut client, DiagnosticExportRequest::ReadSealed).await;
        for _ in 0..100 {
            if state.sealed_calls.load(Ordering::Acquire) == 1 {
                break;
            }
            sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(state.sealed_calls.load(Ordering::Acquire), 1);
        assert_eq!(
            state.acknowledge(7, 3).await,
            AcknowledgementOutcome::Applied
        );
        timeout(TEST_WAIT, handler).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn applied_acknowledgement_with_stalled_response_remains_ambiguous() {
        let state = MockState::new(Ok(None), AcknowledgementOutcome::Applied);
        let (server, mut client) = duplex(1);
        let handler = tokio::spawn(handle_connection(server, Arc::new(state.clone()), limits()));
        write_request(
            &mut client,
            DiagnosticExportRequest::Acknowledge {
                instance_id: 7,
                window_id: 3,
            },
        )
        .await;
        for _ in 0..100 {
            if state.inner.lock().unwrap().acknowledged_binding == Some((7, 3)) {
                break;
            }
            sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(
            state.inner.lock().unwrap().acknowledged_binding,
            Some((7, 3))
        );
        timeout(TEST_WAIT, handler).await.unwrap().unwrap();
    }

    #[test]
    fn runtime_errors_map_without_raw_details() {
        assert_eq!(
            map_acknowledgement(Err(DiagnosticListenerError::Runtime(
                DiagnosticRuntimeError::Window(DiagnosticWindowError::NoSealedWindow)
            ))),
            AcknowledgementOutcome::NoSealed
        );
        assert_eq!(
            map_acknowledgement(Err(DiagnosticListenerError::Runtime(
                DiagnosticRuntimeError::Window(DiagnosticWindowError::AcknowledgementMismatch)
            ))),
            AcknowledgementOutcome::Mismatch
        );
        assert_eq!(
            map_acknowledgement(Err(DiagnosticListenerError::ClockOverflow)),
            AcknowledgementOutcome::Unavailable
        );
    }

    #[derive(Clone)]
    struct AllowAll;

    impl ConsumerAuthenticator for AllowAll {
        fn permits(&self, _stream: &UnixStream) -> bool {
            true
        }
    }

    #[derive(Clone)]
    struct DenyAll;

    impl ConsumerAuthenticator for DenyAll {
        fn permits(&self, _stream: &UnixStream) -> bool {
            false
        }
    }

    #[tokio::test]
    async fn unauthenticated_connection_receives_no_response_or_state_access() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("denied.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(MockState::new(
            Ok(Some(window())),
            AcknowledgementOutcome::Applied,
        ));
        let (shutdown_sender, shutdown) = oneshot::channel();
        let server = tokio::spawn(serve_with_parts(
            listener,
            Arc::clone(&state),
            Arc::new(DenyAll),
            limits(),
            async {
                let _ = shutdown.await;
            },
        ));
        let mut client = UnixStream::connect(&socket).await.unwrap();
        client
            .write_all(&encode_request(DiagnosticExportRequest::ReadSealed).unwrap())
            .await
            .unwrap();
        let mut probe = [0_u8; 1];
        assert!(closed_without_response(
            timeout(TEST_WAIT, client.read(&mut probe)).await.unwrap()
        ));
        assert_eq!(state.sealed_calls.load(Ordering::Acquire), 0);
        shutdown_sender.send(()).unwrap();
        timeout(TEST_WAIT, server).await.unwrap().unwrap().unwrap();
    }

    #[tokio::test]
    async fn shutdown_joins_bounded_connections_and_instance_is_single_use() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("export.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let state = Arc::new(MockState::new(
            Ok(Some(window())),
            AcknowledgementOutcome::Applied,
        ));
        let (shutdown_sender, shutdown) = oneshot::channel();
        let server = tokio::spawn(serve_with_parts(
            listener,
            Arc::clone(&state),
            Arc::new(AllowAll),
            limits(),
            async {
                let _ = shutdown.await;
            },
        ));
        let mut first = UnixStream::connect(&socket).await.unwrap();
        let mut second = UnixStream::connect(&socket).await.unwrap();
        first.write_all(&[1]).await.unwrap();
        second.write_all(&[1]).await.unwrap();
        sleep(Duration::from_millis(5)).await;
        let mut third = UnixStream::connect(&socket).await.unwrap();
        third
            .write_all(&encode_request(DiagnosticExportRequest::ReadSealed).unwrap())
            .await
            .unwrap();
        let mut probe = [0_u8; 1];
        assert!(closed_without_response(
            timeout(TEST_WAIT, third.read(&mut probe)).await.unwrap()
        ));
        shutdown_sender.send(()).unwrap();
        timeout(TEST_WAIT, server).await.unwrap().unwrap().unwrap();

        let runtime = Arc::new(MockState::new(Ok(None), AcknowledgementOutcome::Applied));
        let directory = tempfile::tempdir().unwrap();
        let first_listener = UnixListener::bind(directory.path().join("first.sock")).unwrap();
        let second_listener = UnixListener::bind(directory.path().join("second.sock")).unwrap();
        let owner = TestOwner {
            state: runtime,
            authenticator: Arc::new(AllowAll),
            limits: limits(),
            started: AtomicBool::new(false),
        };
        owner.serve(first_listener, async {}).await.unwrap();
        assert_eq!(
            owner.serve(second_listener, async {}).await,
            Err(DiagnosticExportListenerError::AlreadyStarted)
        );
    }

    struct TestOwner<S, A> {
        state: Arc<S>,
        authenticator: Arc<A>,
        limits: ListenerLimits,
        started: AtomicBool,
    }

    impl<S, A> TestOwner<S, A>
    where
        S: ExportState,
        A: ConsumerAuthenticator,
    {
        async fn serve<F>(
            &self,
            listener: UnixListener,
            shutdown: F,
        ) -> Result<(), DiagnosticExportListenerError>
        where
            F: Future<Output = ()> + Send,
        {
            if self
                .started
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                return Err(DiagnosticExportListenerError::AlreadyStarted);
            }
            serve_with_parts(
                listener,
                Arc::clone(&self.state),
                Arc::clone(&self.authenticator),
                self.limits,
                shutdown,
            )
            .await
        }
    }
}
