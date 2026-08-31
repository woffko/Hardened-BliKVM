//! Transient authenticated diagnostic listener runtime.
//!
//! The caller owns binding, permissions, and cleanup of the Unix socket. This
//! module owns only accepted streams, kernel-bound peer authentication, a fresh
//! process-local instance binding, monotonic policy time, and bounded frame I/O.

use std::{
    error::Error,
    fmt,
    future::Future,
    mem::size_of,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use bli_kvm_diagnostic_peer_policy::DiagnosticPeerAssignments;
use bli_kvm_diagnostic_protocol::{DIAGNOSTIC_EVENT_FRAME_BYTES, DIAGNOSTIC_RESPONSE_FRAME_BYTES};
use bli_kvm_diagnostic_window_policy::{
    DiagnosticWindowError, DiagnosticWindowSnapshot, WindowTickOutcome,
};
use rand_core::{OsRng, RngCore};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{Mutex, watch},
    task::JoinSet,
    time::{MissedTickBehavior, interval, timeout},
};

use crate::{
    AuthenticatedDiagnosticPeer, DiagnosticConnection, DiagnosticFrameOutcome,
    DiagnosticPeerAdapterError, DiagnosticRuntimeCore, DiagnosticRuntimeError, map_tokio_peer,
    resolve_current_assignments,
};

/// Maximum time allowed after the first byte of a producer frame arrives.
pub const DIAGNOSTIC_PARTIAL_FRAME_TIMEOUT: Duration = Duration::from_secs(5);
/// Maximum time allowed to write one mandatory producer response.
pub const DIAGNOSTIC_RESPONSE_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Cadence for checking time-based diagnostic-window rotation.
pub const DIAGNOSTIC_WINDOW_TICK_INTERVAL: Duration = Duration::from_secs(1);

const INSTANCE_ID_ATTEMPTS: usize = 8;

/// Coarse fail-closed listener errors without paths or numeric peer identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticListenerError {
    AccountResolution(DiagnosticPeerAdapterError),
    Runtime(DiagnosticRuntimeError),
    EntropyUnavailable,
    InvalidLimits,
    AlreadyStarted,
    AcceptFailed,
    ClockOverflow,
    ConnectionTaskFailed,
}

impl fmt::Display for DiagnosticListenerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::AccountResolution(_) => "diagnostic account resolution failed",
            Self::Runtime(_) => "diagnostic runtime policy failed",
            Self::EntropyUnavailable => "diagnostic instance entropy unavailable",
            Self::InvalidLimits => "diagnostic runtime limits are invalid",
            Self::AlreadyStarted => "diagnostic listener runtime already started",
            Self::AcceptFailed => "diagnostic listener accept failed",
            Self::ClockOverflow => "diagnostic monotonic clock overflowed",
            Self::ConnectionTaskFailed => "diagnostic connection task failed",
        };
        formatter.write_str(message)
    }
}

impl Error for DiagnosticListenerError {}

impl From<DiagnosticPeerAdapterError> for DiagnosticListenerError {
    fn from(error: DiagnosticPeerAdapterError) -> Self {
        Self::AccountResolution(error)
    }
}

impl From<DiagnosticRuntimeError> for DiagnosticListenerError {
    fn from(error: DiagnosticRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

/// Single-use owner of authenticated diagnostic connections and window state.
///
/// It intentionally does not implement `Clone` or `Debug`. Snapshot access
/// returns only the bounded semantic export model.
pub struct DiagnosticCollectorRuntime {
    assignments: DiagnosticPeerAssignments,
    shared: Arc<SharedRuntime>,
    limits: RuntimeLimits,
    started: AtomicBool,
}

impl DiagnosticCollectorRuntime {
    /// Resolves the fixed account set, verifies the collector eUID, and creates
    /// a fresh nonzero instance binding from operating-system entropy.
    pub fn from_current_process() -> Result<Self, DiagnosticListenerError> {
        let assignments = resolve_current_assignments()?;
        let instance_id = generate_instance_id()?;
        Self::with_parts(assignments, instance_id, RuntimeLimits::PRODUCTION)
    }

    /// Serves one caller-owned pre-bound Unix listener until shutdown.
    ///
    /// The caller remains responsible for the socket path, access mode, and
    /// cleanup. A runtime instance can enter this loop only once. On shutdown,
    /// every admitted connection is joined after bounded in-flight response
    /// handling and lifecycle disconnect accounting.
    pub async fn serve_until<F>(
        &self,
        listener: UnixListener,
        shutdown: F,
    ) -> Result<(), DiagnosticListenerError>
    where
        F: Future<Output = ()> + Send,
    {
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(DiagnosticListenerError::AlreadyStarted);
        }

        let (shutdown_sender, _) = watch::channel(false);
        let mut connections = JoinSet::new();
        let mut ticker = interval(self.limits.tick_interval);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        tokio::pin!(shutdown);
        let mut result = Ok(());

        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => break,
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(joined) = joined {
                        if let Err(error) = connection_task_result(joined) {
                            result = Err(error);
                            break;
                        }
                    }
                }
                _ = ticker.tick() => {
                    if let Err(error) = self.shared.tick().await {
                        result = Err(error);
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let (stream, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(_) => {
                            result = Err(DiagnosticListenerError::AcceptFailed);
                            break;
                        }
                    };
                    if let Err(error) = self
                        .admit(stream, &shutdown_sender, &mut connections)
                        .await
                    {
                        result = Err(error);
                        break;
                    }
                }
            }
        }

        let _ = shutdown_sender.send(true);
        while let Some(joined) = connections.join_next().await {
            if result.is_ok() {
                if let Err(error) = connection_task_result(joined) {
                    result = Err(error);
                }
            }
        }
        result
    }

    /// Returns the current bounded live diagnostic snapshot.
    pub async fn live_snapshot(&self) -> Result<DiagnosticWindowSnapshot, DiagnosticListenerError> {
        self.shared.live_snapshot().await
    }

    /// Returns a clone of the bounded sealed snapshot awaiting acknowledgement.
    pub async fn sealed_window(
        &self,
    ) -> Result<Option<DiagnosticWindowSnapshot>, DiagnosticListenerError> {
        self.shared.sealed_window().await
    }

    /// Acknowledges only the exact current instance and sealed window identifiers.
    pub async fn acknowledge_sealed(
        &self,
        instance_id: u64,
        window_id: u32,
    ) -> Result<(), DiagnosticListenerError> {
        self.shared.acknowledge_sealed(instance_id, window_id).await
    }

    async fn admit(
        &self,
        stream: UnixStream,
        shutdown_sender: &watch::Sender<bool>,
        connections: &mut JoinSet<Result<(), DiagnosticListenerError>>,
    ) -> Result<(), DiagnosticListenerError> {
        let peer = match map_tokio_peer(&self.assignments, &stream) {
            Ok(peer) => peer,
            Err(_) => return Ok(()),
        };
        let connection = match self.shared.accept(peer).await? {
            Some(connection) => connection,
            None => return Ok(()),
        };
        let shared = Arc::clone(&self.shared);
        let shutdown = shutdown_sender.subscribe();
        let limits = self.limits;
        connections.spawn(async move {
            handle_connection(stream, shared, connection, limits, shutdown).await
        });
        Ok(())
    }

    fn with_parts(
        assignments: DiagnosticPeerAssignments,
        instance_id: u64,
        limits: RuntimeLimits,
    ) -> Result<Self, DiagnosticListenerError> {
        limits.validate()?;
        Ok(Self {
            assignments,
            shared: Arc::new(SharedRuntime {
                core: Mutex::new(DiagnosticRuntimeCore::new(instance_id, 0)?),
                clock: MonotonicClock::new(),
            }),
            limits,
            started: AtomicBool::new(false),
        })
    }
}

struct SharedRuntime {
    core: Mutex<DiagnosticRuntimeCore>,
    clock: MonotonicClock,
}

impl SharedRuntime {
    async fn accept(
        &self,
        peer: AuthenticatedDiagnosticPeer,
    ) -> Result<Option<DiagnosticConnection>, DiagnosticListenerError> {
        let mut core = self.core.lock().await;
        let now_ms = self.clock.now_ms()?;
        match core.accept(peer, now_ms) {
            Ok(connection) => Ok(Some(connection)),
            Err(DiagnosticRuntimeError::Window(
                DiagnosticWindowError::SourceAlreadyConnected
                | DiagnosticWindowError::Backpressure
                | DiagnosticWindowError::CollectorUnavailable,
            )) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    async fn handle_frame(
        &self,
        connection: &mut DiagnosticConnection,
        frame: &[u8; DIAGNOSTIC_EVENT_FRAME_BYTES],
    ) -> Result<DiagnosticFrameOutcome, DiagnosticListenerError> {
        let mut core = self.core.lock().await;
        let now_ms = self.clock.now_ms()?;
        core.handle_frame(connection, now_ms, frame)
            .map_err(Into::into)
    }

    async fn disconnect(
        &self,
        connection: &mut DiagnosticConnection,
    ) -> Result<(), DiagnosticListenerError> {
        let mut core = self.core.lock().await;
        let now_ms = self.clock.now_ms()?;
        match core.disconnect(connection, now_ms) {
            Ok(_) | Err(DiagnosticRuntimeError::ConnectionClosed) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    async fn tick(&self) -> Result<WindowTickOutcome, DiagnosticListenerError> {
        let mut core = self.core.lock().await;
        let now_ms = self.clock.now_ms()?;
        core.tick(now_ms).map_err(Into::into)
    }

    async fn live_snapshot(&self) -> Result<DiagnosticWindowSnapshot, DiagnosticListenerError> {
        self.core.lock().await.live_snapshot().map_err(Into::into)
    }

    async fn sealed_window(
        &self,
    ) -> Result<Option<DiagnosticWindowSnapshot>, DiagnosticListenerError> {
        Ok(self.core.lock().await.sealed_window().cloned())
    }

    async fn acknowledge_sealed(
        &self,
        instance_id: u64,
        window_id: u32,
    ) -> Result<(), DiagnosticListenerError> {
        self.core
            .lock()
            .await
            .acknowledge_sealed(instance_id, window_id)
            .map_err(Into::into)
    }
}

struct MonotonicClock {
    origin: Instant,
}

impl MonotonicClock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }

    fn now_ms(&self) -> Result<u64, DiagnosticListenerError> {
        u64::try_from(self.origin.elapsed().as_millis())
            .map_err(|_| DiagnosticListenerError::ClockOverflow)
    }
}

#[derive(Clone, Copy)]
struct RuntimeLimits {
    partial_frame_timeout: Duration,
    response_write_timeout: Duration,
    tick_interval: Duration,
}

impl RuntimeLimits {
    const PRODUCTION: Self = Self {
        partial_frame_timeout: DIAGNOSTIC_PARTIAL_FRAME_TIMEOUT,
        response_write_timeout: DIAGNOSTIC_RESPONSE_WRITE_TIMEOUT,
        tick_interval: DIAGNOSTIC_WINDOW_TICK_INTERVAL,
    };

    fn validate(self) -> Result<(), DiagnosticListenerError> {
        if self.partial_frame_timeout.is_zero()
            || self.response_write_timeout.is_zero()
            || self.tick_interval.is_zero()
        {
            return Err(DiagnosticListenerError::InvalidLimits);
        }
        Ok(())
    }
}

enum FrameReadOutcome {
    Frame([u8; DIAGNOSTIC_EVENT_FRAME_BYTES]),
    Closed,
}

async fn handle_connection<S>(
    mut stream: S,
    shared: Arc<SharedRuntime>,
    mut connection: DiagnosticConnection,
    limits: RuntimeLimits,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), DiagnosticListenerError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    loop {
        let frame = match read_frame(&mut stream, limits.partial_frame_timeout, &mut shutdown).await
        {
            FrameReadOutcome::Frame(frame) => frame,
            FrameReadOutcome::Closed => {
                shared.disconnect(&mut connection).await?;
                return Ok(());
            }
        };
        match shared.handle_frame(&mut connection, &frame).await? {
            DiagnosticFrameOutcome::Response(response) => {
                if !write_response(&mut stream, &response, limits.response_write_timeout).await {
                    shared.disconnect(&mut connection).await?;
                    return Ok(());
                }
            }
            DiagnosticFrameOutcome::Terminated { .. } => return Ok(()),
        }
    }
}

async fn read_frame<S>(
    stream: &mut S,
    partial_frame_timeout: Duration,
    shutdown: &mut watch::Receiver<bool>,
) -> FrameReadOutcome
where
    S: AsyncRead + Unpin,
{
    if *shutdown.borrow() {
        return FrameReadOutcome::Closed;
    }
    let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
    let first = tokio::select! {
        biased;
        _ = shutdown.changed() => return FrameReadOutcome::Closed,
        result = stream.read(&mut frame[..1]) => result.ok(),
    };
    if first != Some(1) {
        return FrameReadOutcome::Closed;
    }
    let remainder = tokio::select! {
        biased;
        _ = shutdown.changed() => return FrameReadOutcome::Closed,
        result = timeout(partial_frame_timeout, stream.read_exact(&mut frame[1..])) => result,
    };
    if !matches!(remainder, Ok(Ok(_))) {
        return FrameReadOutcome::Closed;
    }
    FrameReadOutcome::Frame(frame)
}

async fn write_response<S>(
    stream: &mut S,
    response: &[u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES],
    response_timeout: Duration,
) -> bool
where
    S: AsyncWrite + Unpin,
{
    matches!(
        timeout(response_timeout, stream.write_all(response)).await,
        Ok(Ok(()))
    )
}

fn generate_instance_id() -> Result<u64, DiagnosticListenerError> {
    let mut bytes = [0_u8; size_of::<u64>()];
    let mut rng = OsRng;
    for _ in 0..INSTANCE_ID_ATTEMPTS {
        rng.try_fill_bytes(&mut bytes)
            .map_err(|_| DiagnosticListenerError::EntropyUnavailable)?;
        if let Some(instance_id) = instance_id_from_bytes(bytes) {
            return Ok(instance_id);
        }
    }
    Err(DiagnosticListenerError::EntropyUnavailable)
}

fn instance_id_from_bytes(bytes: [u8; size_of::<u64>()]) -> Option<u64> {
    match u64::from_ne_bytes(bytes) {
        0 => None,
        instance_id => Some(instance_id),
    }
}

fn connection_task_result(
    joined: Result<Result<(), DiagnosticListenerError>, tokio::task::JoinError>,
) -> Result<(), DiagnosticListenerError> {
    match joined {
        Ok(result) => result,
        Err(_) => Err(DiagnosticListenerError::ConnectionTaskFailed),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use bli_kvm_diagnostic_peer_policy::DiagnosticSource;
    use bli_kvm_diagnostic_protocol::{
        DiagnosticEvent, DiagnosticProducerResponse, encode_event, encode_response,
    };
    use nix::unistd::{getegid, geteuid};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, duplex},
        net::{UnixListener, UnixStream},
        sync::{oneshot, watch},
        time::{sleep, timeout},
    };

    use super::*;

    const TEST_TIMEOUT: Duration = Duration::from_millis(25);
    const TEST_WAIT: Duration = Duration::from_millis(250);

    #[test]
    fn instance_id_is_native_endian_nonzero_and_limits_are_bounded() {
        assert_eq!(instance_id_from_bytes([0; 8]), None);
        assert_eq!(instance_id_from_bytes(7_u64.to_ne_bytes()), Some(7));
        assert!(generate_instance_id().unwrap() != 0);
        assert!(RuntimeLimits::PRODUCTION.validate().is_ok());
    }

    #[tokio::test]
    async fn long_running_idle_is_allowed_but_partial_frame_times_out() {
        let runtime = test_runtime();
        let connection = runtime
            .shared
            .accept(peer(DiagnosticSource::Server))
            .await
            .unwrap()
            .unwrap();
        let (server, mut client) = duplex(64);
        let (_shutdown_sender, shutdown) = watch::channel(false);
        let shared = Arc::clone(&runtime.shared);
        let handler = tokio::spawn(handle_connection(
            server,
            shared,
            connection,
            test_limits(),
            shutdown,
        ));

        sleep(TEST_TIMEOUT * 2).await;
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        client.write_all(&frame).await.unwrap();
        let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
        client.read_exact(&mut response).await.unwrap();
        assert_eq!(
            response,
            encode_response(DiagnosticProducerResponse::Accepted)
        );

        client.write_all(&frame[..1]).await.unwrap();
        timeout(TEST_WAIT, handler)
            .await
            .expect("partial frame retained the connection")
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            3
        );
    }

    #[tokio::test]
    async fn response_write_timeout_records_disconnect() {
        let runtime = test_runtime();
        let connection = runtime
            .shared
            .accept(peer(DiagnosticSource::Server))
            .await
            .unwrap()
            .unwrap();
        let (server, mut client) = duplex(1);
        let (_shutdown_sender, shutdown) = watch::channel(false);
        let handler = tokio::spawn(handle_connection(
            server,
            Arc::clone(&runtime.shared),
            connection,
            test_limits(),
            shutdown,
        ));
        client
            .write_all(&encode_event(DiagnosticEvent::Ready).unwrap())
            .await
            .unwrap();
        timeout(TEST_WAIT, handler)
            .await
            .expect("blocked response retained the connection")
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            3
        );
    }

    #[tokio::test]
    async fn shutdown_after_a_full_frame_preserves_the_mandatory_response() {
        let runtime = test_runtime();
        let connection = runtime
            .shared
            .accept(peer(DiagnosticSource::Server))
            .await
            .unwrap()
            .unwrap();
        let (server, mut client) = duplex(1);
        let (shutdown_sender, shutdown) = watch::channel(false);
        let handler = tokio::spawn(handle_connection(
            server,
            Arc::clone(&runtime.shared),
            connection,
            test_limits(),
            shutdown,
        ));
        client
            .write_all(&encode_event(DiagnosticEvent::Ready).unwrap())
            .await
            .unwrap();
        let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
        timeout(TEST_WAIT, client.read_exact(&mut response[..1]))
            .await
            .expect("committed response did not start")
            .unwrap();
        shutdown_sender.send(true).unwrap();
        timeout(TEST_WAIT, client.read_exact(&mut response[1..]))
            .await
            .expect("shutdown discarded an already committed response")
            .unwrap();
        assert_eq!(
            response,
            encode_response(DiagnosticProducerResponse::Accepted)
        );
        timeout(TEST_WAIT, handler)
            .await
            .expect("shutdown retained the completed connection")
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            3
        );
    }

    #[tokio::test]
    async fn malformed_frame_has_no_response_and_records_disconnect() {
        let runtime = test_runtime();
        let connection = runtime
            .shared
            .accept(peer(DiagnosticSource::Server))
            .await
            .unwrap()
            .unwrap();
        let (server, mut client) = duplex(64);
        let (_shutdown_sender, shutdown) = watch::channel(false);
        let handler = tokio::spawn(handle_connection(
            server,
            Arc::clone(&runtime.shared),
            connection,
            test_limits(),
            shutdown,
        ));
        client
            .write_all(&[0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES])
            .await
            .unwrap();
        let mut response = Vec::new();
        timeout(TEST_WAIT, client.read_to_end(&mut response))
            .await
            .expect("malformed frame retained the stream")
            .unwrap();
        assert!(response.is_empty());
        handler.await.unwrap().unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            2
        );
    }

    #[tokio::test]
    async fn concurrent_sources_share_one_monotonic_policy_order() {
        let runtime = test_runtime();
        let server_connection = runtime
            .shared
            .accept(peer(DiagnosticSource::Server))
            .await
            .unwrap()
            .unwrap();
        let broker_connection = runtime
            .shared
            .accept(peer(DiagnosticSource::PlatformBroker))
            .await
            .unwrap()
            .unwrap();
        let (server_io, mut server_client) = duplex(256);
        let (broker_io, mut broker_client) = duplex(256);
        let (shutdown_sender, shutdown) = watch::channel(false);
        let server_handler = tokio::spawn(handle_connection(
            server_io,
            Arc::clone(&runtime.shared),
            server_connection,
            test_limits(),
            shutdown.clone(),
        ));
        let broker_handler = tokio::spawn(handle_connection(
            broker_io,
            Arc::clone(&runtime.shared),
            broker_connection,
            test_limits(),
            shutdown,
        ));
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        let expected = encode_response(DiagnosticProducerResponse::Accepted);
        let server_client_task = async {
            for _ in 0..6 {
                server_client.write_all(&frame).await.unwrap();
                let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
                server_client.read_exact(&mut response).await.unwrap();
                assert_eq!(response, expected);
            }
        };
        let broker_client_task = async {
            for _ in 0..6 {
                broker_client.write_all(&frame).await.unwrap();
                let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
                broker_client.read_exact(&mut response).await.unwrap();
                assert_eq!(response, expected);
            }
        };
        tokio::join!(server_client_task, broker_client_task);
        shutdown_sender.send(true).unwrap();
        timeout(TEST_WAIT, server_handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        timeout(TEST_WAIT, broker_handler)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            16
        );
    }

    #[tokio::test]
    async fn real_listener_rejects_duplicate_role_and_cleans_up_on_shutdown() {
        let Some(assignments) = current_process_assignments() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("diagnostics.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let runtime = Arc::new(
            DiagnosticCollectorRuntime::with_parts(assignments, 7, test_limits()).unwrap(),
        );
        let (shutdown_sender, shutdown) = oneshot::channel();
        let serving = Arc::clone(&runtime);
        let server = tokio::spawn(async move {
            serving
                .serve_until(listener, async {
                    let _ = shutdown.await;
                })
                .await
        });

        let mut first = UnixStream::connect(&socket).await.unwrap();
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        first.write_all(&frame).await.unwrap();
        let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
        first.read_exact(&mut response).await.unwrap();
        assert_eq!(
            response,
            encode_response(DiagnosticProducerResponse::Accepted)
        );

        let mut duplicate = UnixStream::connect(&socket).await.unwrap();
        let mut probe = [0_u8; 1];
        assert_eq!(
            timeout(TEST_WAIT, duplicate.read(&mut probe))
                .await
                .expect("duplicate peer was not rejected")
                .unwrap(),
            0
        );
        first.write_all(&frame).await.unwrap();
        first.read_exact(&mut response).await.unwrap();
        assert_eq!(
            response,
            encode_response(DiagnosticProducerResponse::Accepted)
        );

        shutdown_sender.send(()).unwrap();
        assert_eq!(
            timeout(TEST_WAIT, first.read(&mut probe))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        server.await.unwrap().unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            4
        );
    }

    #[tokio::test]
    async fn real_listener_rejects_unknown_uid_before_starting_a_session() {
        let Some(assignments) = assignments_without_current_process() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("diagnostics-unknown.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let runtime = Arc::new(
            DiagnosticCollectorRuntime::with_parts(assignments, 7, test_limits()).unwrap(),
        );
        let (shutdown_sender, shutdown) = oneshot::channel();
        let serving = Arc::clone(&runtime);
        let server = tokio::spawn(async move {
            serving
                .serve_until(listener, async {
                    let _ = shutdown.await;
                })
                .await
        });

        let mut client = UnixStream::connect(&socket).await.unwrap();
        let mut probe = [0_u8; 1];
        assert_eq!(
            timeout(TEST_WAIT, client.read(&mut probe))
                .await
                .expect("unknown peer was not rejected")
                .unwrap(),
            0
        );
        shutdown_sender.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert_eq!(
            runtime
                .live_snapshot()
                .await
                .unwrap()
                .bundle()
                .record_count(),
            0
        );
    }

    #[tokio::test]
    async fn real_listener_reaps_reconnects_and_exactly_acks_sealed_windows() {
        let Some(assignments) = current_process_assignments() else {
            return;
        };
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("diagnostics-churn.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let runtime = Arc::new(
            DiagnosticCollectorRuntime::with_parts(assignments, 7, test_limits()).unwrap(),
        );
        let (shutdown_sender, shutdown) = oneshot::channel();
        let serving = Arc::clone(&runtime);
        let server = tokio::spawn(async move {
            serving
                .serve_until(listener, async {
                    let _ = shutdown.await;
                })
                .await
        });
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        let expected = encode_response(DiagnosticProducerResponse::Accepted);

        for _ in 0..40 {
            let mut client = UnixStream::connect(&socket).await.unwrap();
            client.write_all(&frame).await.unwrap();
            let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
            client.read_exact(&mut response).await.unwrap();
            assert_eq!(response, expected);
            client.shutdown().await.unwrap();
            let mut probe = [0_u8; 1];
            assert_eq!(
                timeout(TEST_WAIT, client.read(&mut probe))
                    .await
                    .expect("EOF did not release the diagnostic role")
                    .unwrap(),
                0
            );
            if let Some(sealed) = runtime.sealed_window().await.unwrap() {
                runtime
                    .acknowledge_sealed(sealed.instance_id(), sealed.window_id())
                    .await
                    .unwrap();
            }
        }

        shutdown_sender.send(()).unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn one_runtime_instance_cannot_restart_on_another_listener() {
        let runtime = test_runtime();
        let directory = tempfile::tempdir().unwrap();
        let first = UnixListener::bind(directory.path().join("first.sock")).unwrap();
        runtime.serve_until(first, async {}).await.unwrap();
        let second = UnixListener::bind(directory.path().join("second.sock")).unwrap();
        assert_eq!(
            runtime.serve_until(second, async {}).await,
            Err(DiagnosticListenerError::AlreadyStarted)
        );
    }

    fn test_runtime() -> DiagnosticCollectorRuntime {
        DiagnosticCollectorRuntime::with_parts(test_assignments(), 7, test_limits()).unwrap()
    }

    const fn test_limits() -> RuntimeLimits {
        RuntimeLimits {
            partial_frame_timeout: TEST_TIMEOUT,
            response_write_timeout: TEST_TIMEOUT,
            tick_interval: Duration::from_millis(10),
        }
    }

    fn test_assignments() -> DiagnosticPeerAssignments {
        DiagnosticPeerAssignments::new(100, 101, 102, 103, 104, 105, 200).unwrap()
    }

    fn peer(source: DiagnosticSource) -> AuthenticatedDiagnosticPeer {
        AuthenticatedDiagnosticPeer { source }
    }

    fn current_process_assignments() -> Option<DiagnosticPeerAssignments> {
        let uid = geteuid().as_raw();
        let gid = getegid().as_raw();
        if uid == 0 || gid == 0 {
            return None;
        }
        let mut values = six_uids_excluding(uid);
        values[1] = uid;
        DiagnosticPeerAssignments::new(
            values[0], values[1], values[2], values[3], values[4], values[5], gid,
        )
        .ok()
    }

    fn assignments_without_current_process() -> Option<DiagnosticPeerAssignments> {
        let uid = geteuid().as_raw();
        let gid = getegid().as_raw();
        if uid == 0 || gid == 0 {
            return None;
        }
        let values = six_uids_excluding(uid);
        DiagnosticPeerAssignments::new(
            values[0], values[1], values[2], values[3], values[4], values[5], gid,
        )
        .ok()
    }

    fn six_uids_excluding(excluded: u32) -> [u32; 6] {
        let mut values = [0_u32; 6];
        let mut candidate = 1_u32;
        let mut index = 0;
        while index < values.len() {
            while candidate == excluded || values[..index].contains(&candidate) {
                candidate += 1;
            }
            values[index] = candidate;
            candidate += 1;
            index += 1;
        }
        values
    }
}
