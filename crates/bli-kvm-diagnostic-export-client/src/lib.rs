#![forbid(unsafe_code)]

//! Fixed-path client for one ephemeral sealed diagnostic-window consumer.
//!
//! Every operation uses one fresh Unix connection and exactly one canonical
//! request/response exchange. The client has no persistence, retry loop,
//! arbitrary socket path, query, HTTP, or logging surface. Read failures are
//! safe for an explicit caller retry because reading does not mutate collector
//! state. Once an acknowledgement write begins, every transport or response
//! failure is conservatively delivery-ambiguous and is never retried here.

use std::{
    error::Error,
    fmt,
    future::Future,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::Duration,
};

use bli_kvm_diagnostic_export::DiagnosticBundle;
use bli_kvm_diagnostic_export_protocol::{
    DIAGNOSTIC_EXPORT_REQUEST_BYTES, DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES,
    DecodedDiagnosticExportResponse, DiagnosticExportRequest, MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES,
    decode_response, encode_request,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    time::timeout,
};

pub const DIAGNOSTIC_EXPORT_SOCKET: &str = "/run/hardened-blikvm-diagnostics/export.sock";
pub const DIAGNOSTIC_EXPORT_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DIAGNOSTIC_EXPORT_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DIAGNOSTIC_EXPORT_READ_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_DIAGNOSTIC_EXPORT_PAYLOAD_BYTES: usize =
    MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES - DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportNotSentReason {
    InvalidBinding,
    ConnectTimeout,
    ConnectFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportExchangeStage {
    WriteTimeout,
    WriteFailed,
    ResponseTimeout,
    ResponseClosed,
    ResponseReadFailed,
    ResponseTooLarge,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportReadError {
    NotSent {
        reason: DiagnosticExportNotSentReason,
    },
    Retryable {
        stage: DiagnosticExportExchangeStage,
    },
}

impl fmt::Display for DiagnosticExportReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSent { .. } => formatter.write_str("diagnostic read was not sent"),
            Self::Retryable { .. } => formatter.write_str("diagnostic read failed"),
        }
    }
}

impl Error for DiagnosticExportReadError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportAcknowledgementError {
    NotSent {
        reason: DiagnosticExportNotSentReason,
    },
    DeliveryAmbiguous {
        stage: DiagnosticExportExchangeStage,
    },
}

impl fmt::Display for DiagnosticExportAcknowledgementError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSent { .. } => formatter.write_str("diagnostic acknowledgement was not sent"),
            Self::DeliveryAmbiguous { .. } => {
                formatter.write_str("diagnostic acknowledgement delivery is ambiguous")
            }
        }
    }
}

impl Error for DiagnosticExportAcknowledgementError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagnosticExportBinding {
    instance_id: u64,
    window_id: u32,
}

impl DiagnosticExportBinding {
    pub const fn new(instance_id: u64, window_id: u32) -> Option<Self> {
        if instance_id == 0 || window_id == 0 {
            None
        } else {
            Some(Self {
                instance_id,
                window_id,
            })
        }
    }

    pub const fn instance_id(self) -> u64 {
        self.instance_id
    }

    pub const fn window_id(self) -> u32 {
        self.window_id
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct DiagnosticSealedSnapshot {
    binding: DiagnosticExportBinding,
    omitted_lifecycle_events: u8,
    bundle: DiagnosticBundle,
}

impl DiagnosticSealedSnapshot {
    pub const fn binding(&self) -> DiagnosticExportBinding {
        self.binding
    }

    pub const fn omitted_lifecycle_events(&self) -> u8 {
        self.omitted_lifecycle_events
    }

    pub const fn record_count(&self) -> u8 {
        self.bundle.record_count() as u8
    }

    pub fn as_bytes(&self) -> &[u8] {
        self.bundle.as_bytes()
    }

    pub const fn length(&self) -> usize {
        self.bundle.length()
    }

    pub const fn bundle(&self) -> &DiagnosticBundle {
        &self.bundle
    }
}

impl fmt::Debug for DiagnosticSealedSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticSealedSnapshot")
            .field("binding", &self.binding)
            .field("omitted_lifecycle_events", &self.omitted_lifecycle_events)
            .field("record_count", &self.bundle.record_count())
            .field("length", &self.bundle.length())
            .finish_non_exhaustive()
    }
}

// Keeping the bounded snapshot inline avoids a heap-allocation failure at the
// trust boundary; the maximum size is fixed by the protocol at 8 KiB.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiagnosticExportReadOutcome {
    NoSealed,
    Sealed(DiagnosticSealedSnapshot),
    CollectorUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportAcknowledgementOutcome {
    Acknowledged,
    NoSealed,
    Mismatch,
    CollectorUnavailable,
}

pub struct DiagnosticExportClient {
    socket_path: PathBuf,
    limits: ClientLimits,
}

impl DiagnosticExportClient {
    pub fn new() -> Self {
        Self::with_parts(
            PathBuf::from(DIAGNOSTIC_EXPORT_SOCKET),
            ClientLimits::PRODUCTION,
        )
    }

    /// Reads the current sealed window without mutating collector state.
    ///
    /// This method never retries. A [`DiagnosticExportReadError::Retryable`]
    /// result or cancellation is safe for a later explicit read because the
    /// request cannot acknowledge or clear a window.
    pub async fn read_sealed(
        &self,
    ) -> Result<DiagnosticExportReadOutcome, DiagnosticExportReadError> {
        let request = encode_request(DiagnosticExportRequest::ReadSealed).map_err(|_| {
            DiagnosticExportReadError::NotSent {
                reason: DiagnosticExportNotSentReason::InvalidBinding,
            }
        })?;
        let mut stream = connect(&self.socket_path, self.limits.connect_timeout)
            .await
            .map_err(|reason| DiagnosticExportReadError::NotSent { reason })?;
        let response = exchange(&mut stream, &request, self.limits)
            .await
            .map_err(|stage| DiagnosticExportReadError::Retryable { stage })?;
        decode_read_response(&response)
            .map_err(|stage| DiagnosticExportReadError::Retryable { stage })
    }

    /// Acknowledges one exact collector-instance/window binding.
    ///
    /// This method never retries and is not cancellation-safe after it has
    /// started polling: cancellation may occur after the collector applied the
    /// acknowledgement. Every delivery-ambiguous error or cancelled call must
    /// be reconciled with a later explicit [`Self::read_sealed`] call; callers
    /// must not automatically repeat the acknowledgement.
    pub async fn acknowledge(
        &self,
        binding: DiagnosticExportBinding,
    ) -> Result<DiagnosticExportAcknowledgementOutcome, DiagnosticExportAcknowledgementError> {
        let request = encode_request(DiagnosticExportRequest::Acknowledge {
            instance_id: binding.instance_id,
            window_id: binding.window_id,
        })
        .map_err(|_| DiagnosticExportAcknowledgementError::NotSent {
            reason: DiagnosticExportNotSentReason::InvalidBinding,
        })?;
        let mut stream = connect(&self.socket_path, self.limits.connect_timeout)
            .await
            .map_err(|reason| DiagnosticExportAcknowledgementError::NotSent { reason })?;
        let response = exchange(&mut stream, &request, self.limits)
            .await
            .map_err(|stage| DiagnosticExportAcknowledgementError::DeliveryAmbiguous { stage })?;
        decode_acknowledgement_response(&response, binding)
            .map_err(|stage| DiagnosticExportAcknowledgementError::DeliveryAmbiguous { stage })
    }

    fn with_parts(socket_path: PathBuf, limits: ClientLimits) -> Self {
        debug_assert!(socket_path.is_absolute());
        debug_assert!(limits.valid());
        Self {
            socket_path,
            limits,
        }
    }
}

impl Default for DiagnosticExportClient {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy)]
struct ClientLimits {
    connect_timeout: Duration,
    write_timeout: Duration,
    response_timeout: Duration,
}

impl ClientLimits {
    const PRODUCTION: Self = Self {
        connect_timeout: DIAGNOSTIC_EXPORT_CONNECT_TIMEOUT,
        write_timeout: DIAGNOSTIC_EXPORT_WRITE_TIMEOUT,
        response_timeout: DIAGNOSTIC_EXPORT_READ_TIMEOUT,
    };

    const fn valid(self) -> bool {
        !self.connect_timeout.is_zero()
            && !self.write_timeout.is_zero()
            && !self.response_timeout.is_zero()
    }
}

struct ResponseFrame {
    bytes: [u8; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES],
    length: usize,
}

impl ResponseFrame {
    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }
}

async fn connect(
    path: &Path,
    limit: Duration,
) -> Result<UnixStream, DiagnosticExportNotSentReason> {
    connect_future(UnixStream::connect(path), limit).await
}

async fn connect_future<F>(
    future: F,
    limit: Duration,
) -> Result<UnixStream, DiagnosticExportNotSentReason>
where
    F: Future<Output = io::Result<UnixStream>>,
{
    match timeout(limit, future).await {
        Err(_) => Err(DiagnosticExportNotSentReason::ConnectTimeout),
        Ok(Err(_)) => Err(DiagnosticExportNotSentReason::ConnectFailed),
        Ok(Ok(stream)) => Ok(stream),
    }
}

async fn exchange<S>(
    stream: &mut S,
    request: &[u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES],
    limits: ClientLimits,
) -> Result<ResponseFrame, DiagnosticExportExchangeStage>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match timeout(limits.write_timeout, stream.write_all(request)).await {
        Err(_) => return Err(DiagnosticExportExchangeStage::WriteTimeout),
        Ok(Err(_)) => return Err(DiagnosticExportExchangeStage::WriteFailed),
        Ok(Ok(())) => {}
    }

    match timeout(limits.response_timeout, read_response(stream)).await {
        Err(_) => Err(DiagnosticExportExchangeStage::ResponseTimeout),
        Ok(result) => result,
    }
}

async fn read_response<S>(stream: &mut S) -> Result<ResponseFrame, DiagnosticExportExchangeStage>
where
    S: AsyncRead + Unpin,
{
    let mut frame = ResponseFrame {
        bytes: [0; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES],
        length: 0,
    };
    let mut overflow = [0_u8; 1];
    loop {
        let checking_overflow = frame.length == frame.bytes.len();
        let target: &mut [u8] = if checking_overflow {
            &mut overflow
        } else {
            &mut frame.bytes[frame.length..]
        };
        match stream.read(target).await {
            Ok(0) if frame.length == 0 => {
                return Err(DiagnosticExportExchangeStage::ResponseClosed);
            }
            Ok(0) => return Ok(frame),
            Ok(_) if checking_overflow => {
                return Err(DiagnosticExportExchangeStage::ResponseTooLarge);
            }
            Ok(read) => frame.length += read,
            Err(error) if error.kind() == ErrorKind::UnexpectedEof && frame.length == 0 => {
                return Err(DiagnosticExportExchangeStage::ResponseClosed);
            }
            Err(_) => return Err(DiagnosticExportExchangeStage::ResponseReadFailed),
        }
    }
}

fn decode_read_response(
    frame: &ResponseFrame,
) -> Result<DiagnosticExportReadOutcome, DiagnosticExportExchangeStage> {
    match decode_response(frame.as_bytes())
        .map_err(|_| DiagnosticExportExchangeStage::InvalidResponse)?
    {
        DecodedDiagnosticExportResponse::NoSealed => Ok(DiagnosticExportReadOutcome::NoSealed),
        DecodedDiagnosticExportResponse::Sealed {
            instance_id,
            window_id,
            omitted_lifecycle_events,
            record_count,
            payload,
        } => {
            let binding = DiagnosticExportBinding::new(instance_id, window_id)
                .ok_or(DiagnosticExportExchangeStage::InvalidResponse)?;
            let bundle = DiagnosticBundle::decode(payload)
                .map_err(|_| DiagnosticExportExchangeStage::InvalidResponse)?;
            if bundle.record_count() != usize::from(record_count) {
                return Err(DiagnosticExportExchangeStage::InvalidResponse);
            }
            Ok(DiagnosticExportReadOutcome::Sealed(
                DiagnosticSealedSnapshot {
                    binding,
                    omitted_lifecycle_events,
                    bundle,
                },
            ))
        }
        DecodedDiagnosticExportResponse::CollectorUnavailable => {
            Ok(DiagnosticExportReadOutcome::CollectorUnavailable)
        }
        DecodedDiagnosticExportResponse::Acknowledged { .. }
        | DecodedDiagnosticExportResponse::AcknowledgementMismatch { .. } => {
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        }
    }
}

fn decode_acknowledgement_response(
    frame: &ResponseFrame,
    expected: DiagnosticExportBinding,
) -> Result<DiagnosticExportAcknowledgementOutcome, DiagnosticExportExchangeStage> {
    match decode_response(frame.as_bytes())
        .map_err(|_| DiagnosticExportExchangeStage::InvalidResponse)?
    {
        DecodedDiagnosticExportResponse::NoSealed => {
            Ok(DiagnosticExportAcknowledgementOutcome::NoSealed)
        }
        DecodedDiagnosticExportResponse::Acknowledged {
            instance_id,
            window_id,
        } if (instance_id, window_id) == (expected.instance_id, expected.window_id) => {
            Ok(DiagnosticExportAcknowledgementOutcome::Acknowledged)
        }
        DecodedDiagnosticExportResponse::AcknowledgementMismatch {
            instance_id,
            window_id,
        } if (instance_id, window_id) == (expected.instance_id, expected.window_id) => {
            Ok(DiagnosticExportAcknowledgementOutcome::Mismatch)
        }
        DecodedDiagnosticExportResponse::CollectorUnavailable => {
            Ok(DiagnosticExportAcknowledgementOutcome::CollectorUnavailable)
        }
        DecodedDiagnosticExportResponse::Sealed { .. }
        | DecodedDiagnosticExportResponse::Acknowledged { .. }
        | DecodedDiagnosticExportResponse::AcknowledgementMismatch { .. } => {
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
    };

    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord, RejectReason,
    };
    use bli_kvm_diagnostic_export_protocol::{
        DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION, DiagnosticExportResponse, decode_request,
        encode_response,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, ReadBuf, duplex},
        net::UnixListener,
        sync::oneshot,
        time::{sleep, timeout},
    };

    use super::*;

    const TEST_LIMIT: Duration = Duration::from_millis(30);
    const TEST_WAIT: Duration = Duration::from_millis(500);
    const ONE_RECORD_BUNDLE: &[u8] = b"schema=1 record_count=1\n\
offset_ms=9 severity=info component=control_broker event=ready\n";

    fn limits() -> ClientLimits {
        ClientLimits {
            connect_timeout: TEST_LIMIT,
            write_timeout: TEST_LIMIT,
            response_timeout: TEST_LIMIT,
        }
    }

    fn test_client(path: PathBuf) -> DiagnosticExportClient {
        DiagnosticExportClient::with_parts(path, limits())
    }

    fn encoded(response: DiagnosticExportResponse<'_>) -> Vec<u8> {
        encode_response(response).unwrap().as_bytes().to_vec()
    }

    fn sealed_response(
        instance_id: u64,
        window_id: u32,
        omitted_lifecycle_events: u8,
        record_count: u8,
        payload: &[u8],
    ) -> Vec<u8> {
        assert!(!payload.is_empty());
        assert!(payload.len() <= MAX_DIAGNOSTIC_EXPORT_PAYLOAD_BYTES);
        let mut response = vec![0_u8; DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES + payload.len()];
        response[0] = DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION;
        response[1] = 2;
        response[2] = omitted_lifecycle_events;
        response[3] = record_count;
        response[4..12].copy_from_slice(&instance_id.to_be_bytes());
        response[12..16].copy_from_slice(&window_id.to_be_bytes());
        response[16..18].copy_from_slice(&(payload.len() as u16).to_be_bytes());
        response[DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES..].copy_from_slice(payload);
        response
    }

    async fn receive_request(stream: &mut UnixStream) -> DiagnosticExportRequest {
        let mut request = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
        stream.read_exact(&mut request).await.unwrap();
        decode_request(&request).unwrap()
    }

    #[tokio::test]
    async fn every_read_uses_a_fresh_connection_and_maps_all_read_outcomes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let responses = [
                encoded(DiagnosticExportResponse::NoSealed),
                sealed_response(7, 3, 2, 1, ONE_RECORD_BUNDLE),
                encoded(DiagnosticExportResponse::CollectorUnavailable),
            ];
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(
                    receive_request(&mut stream).await,
                    DiagnosticExportRequest::ReadSealed
                );
                stream.write_all(&response).await.unwrap();
            }
        });
        let client = test_client(path);
        assert_eq!(
            client.read_sealed().await,
            Ok(DiagnosticExportReadOutcome::NoSealed)
        );
        let DiagnosticExportReadOutcome::Sealed(snapshot) = client.read_sealed().await.unwrap()
        else {
            panic!("expected sealed snapshot");
        };
        assert_eq!(
            snapshot.binding(),
            DiagnosticExportBinding::new(7, 3).unwrap()
        );
        assert_eq!(snapshot.omitted_lifecycle_events(), 2);
        assert_eq!(snapshot.record_count(), 1);
        assert_eq!(snapshot.as_bytes(), ONE_RECORD_BUNDLE);
        assert_eq!(snapshot.length(), snapshot.as_bytes().len());
        assert!(!format!("{snapshot:?}").contains("schema="));
        assert_eq!(
            client.read_sealed().await,
            Ok(DiagnosticExportReadOutcome::CollectorUnavailable)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn acknowledgement_maps_all_outcomes_and_requires_exact_echo() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let binding = DiagnosticExportBinding::new(7, 3).unwrap();
        let server = tokio::spawn(async move {
            let responses = [
                encoded(DiagnosticExportResponse::Acknowledged {
                    instance_id: 7,
                    window_id: 3,
                }),
                encoded(DiagnosticExportResponse::AcknowledgementMismatch {
                    instance_id: 7,
                    window_id: 3,
                }),
                encoded(DiagnosticExportResponse::NoSealed),
                encoded(DiagnosticExportResponse::CollectorUnavailable),
                encoded(DiagnosticExportResponse::Acknowledged {
                    instance_id: 8,
                    window_id: 3,
                }),
            ];
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(
                    receive_request(&mut stream).await,
                    DiagnosticExportRequest::Acknowledge {
                        instance_id: 7,
                        window_id: 3,
                    }
                );
                stream.write_all(&response).await.unwrap();
            }
        });
        let client = test_client(path);
        for expected in [
            DiagnosticExportAcknowledgementOutcome::Acknowledged,
            DiagnosticExportAcknowledgementOutcome::Mismatch,
            DiagnosticExportAcknowledgementOutcome::NoSealed,
            DiagnosticExportAcknowledgementOutcome::CollectorUnavailable,
        ] {
            assert_eq!(client.acknowledge(binding).await, Ok(expected));
        }
        assert_eq!(
            client.acknowledge(binding).await,
            Err(DiagnosticExportAcknowledgementError::DeliveryAmbiguous {
                stage: DiagnosticExportExchangeStage::InvalidResponse,
            },)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn maximum_typed_window_is_owned_and_noncanonical_capacity_is_rejected() {
        assert!(DiagnosticExportBinding::new(0, 3).is_none());
        assert!(DiagnosticExportBinding::new(7, 0).is_none());
        let mut exporter = DiagnosticExporter::new();
        for offset in 0..32 {
            exporter
                .push(
                    DiagnosticRecord::new(
                        offset,
                        DiagnosticComponent::SystemActionStore,
                        DiagnosticEvent::RequestRejected {
                            reason: RejectReason::CapabilityDisabled,
                        },
                    )
                    .unwrap(),
                )
                .unwrap();
        }
        let bundle = exporter.export().unwrap();
        let response = sealed_response(9, 4, u8::MAX, 32, bundle.as_bytes());
        let frame = ResponseFrame {
            bytes: response_frame(&response).bytes,
            length: response.len(),
        };
        let DiagnosticExportReadOutcome::Sealed(snapshot) = decode_read_response(&frame).unwrap()
        else {
            panic!("expected sealed snapshot");
        };
        assert_eq!(snapshot.record_count(), 32);
        assert_eq!(snapshot.bundle(), &bundle);
        assert_eq!(snapshot.binding().instance_id(), 9);
        assert_eq!(snapshot.binding().window_id(), 4);

        let arbitrary = vec![b'x'; MAX_DIAGNOSTIC_EXPORT_PAYLOAD_BYTES];
        let frame = response_frame(&sealed_response(9, 4, 0, 1, &arbitrary));
        assert_eq!(
            decode_read_response(&frame),
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn connect_failures_are_definitely_not_sent() {
        let directory = tempfile::tempdir().unwrap();
        let client = test_client(directory.path().join("missing.sock"));
        assert_eq!(
            client.read_sealed().await,
            Err(DiagnosticExportReadError::NotSent {
                reason: DiagnosticExportNotSentReason::ConnectFailed,
            })
        );
        assert_eq!(
            client
                .acknowledge(DiagnosticExportBinding::new(7, 3).unwrap())
                .await,
            Err(DiagnosticExportAcknowledgementError::NotSent {
                reason: DiagnosticExportNotSentReason::ConnectFailed,
            })
        );
        assert!(matches!(
            connect_future(pending::<io::Result<UnixStream>>(), TEST_LIMIT).await,
            Err(DiagnosticExportNotSentReason::ConnectTimeout)
        ));
    }

    #[tokio::test]
    async fn write_timeout_and_failure_are_phase_specific() {
        let request = encode_request(DiagnosticExportRequest::ReadSealed).unwrap();
        let (mut client, _server) = duplex(1);
        assert!(matches!(
            exchange(&mut client, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::WriteTimeout)
        ));

        let (mut client, server) = duplex(64);
        drop(server);
        assert!(matches!(
            exchange(&mut client, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::WriteFailed)
        ));
    }

    #[tokio::test]
    async fn response_timeout_eof_read_error_and_oversize_fail_closed() {
        let request = encode_request(DiagnosticExportRequest::ReadSealed).unwrap();
        let (mut client, mut server) = duplex(64);
        let silent = tokio::spawn(async move {
            let mut received = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
            server.read_exact(&mut received).await.unwrap();
            sleep(TEST_LIMIT * 3).await;
        });
        assert!(matches!(
            exchange(&mut client, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::ResponseTimeout)
        ));
        silent.abort();

        let (mut client, mut server) = duplex(64);
        let closed = tokio::spawn(async move {
            let mut received = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
            server.read_exact(&mut received).await.unwrap();
        });
        assert!(matches!(
            exchange(&mut client, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::ResponseClosed)
        ));
        closed.await.unwrap();

        assert!(matches!(
            exchange(&mut ReadFailureIo, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::ResponseReadFailed)
        ));

        let (mut client, mut server) = duplex(MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES * 2);
        let oversized = tokio::spawn(async move {
            let mut received = [0_u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES];
            server.read_exact(&mut received).await.unwrap();
            server
                .write_all(&vec![0_u8; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES + 1])
                .await
                .unwrap();
        });
        assert!(matches!(
            exchange(&mut client, &request, limits()).await,
            Err(DiagnosticExportExchangeStage::ResponseTooLarge)
        ));
        oversized.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_partial_and_wrong_operation_responses_are_rejected() {
        let mut malformed = encoded(DiagnosticExportResponse::NoSealed);
        malformed[18] = 1;
        let frame = response_frame(&malformed);
        assert_eq!(
            decode_read_response(&frame),
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        );
        let partial = response_frame(&malformed[..malformed.len() - 1]);
        assert_eq!(
            decode_read_response(&partial),
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        );
        let acknowledgement = response_frame(&encoded(DiagnosticExportResponse::Acknowledged {
            instance_id: 7,
            window_id: 3,
        }));
        assert_eq!(
            decode_read_response(&acknowledgement),
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        );
        let sealed = response_frame(&sealed_response(7, 3, 0, 1, b"typed"));
        assert_eq!(
            decode_acknowledgement_response(&sealed, DiagnosticExportBinding::new(7, 3).unwrap()),
            Err(DiagnosticExportExchangeStage::InvalidResponse)
        );
    }

    #[tokio::test]
    async fn partial_socket_responses_keep_read_retryable_and_ack_ambiguous() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let partial = encoded(DiagnosticExportResponse::NoSealed)
            [..DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES - 1]
            .to_vec();
        let server = tokio::spawn(async move {
            for expected in [
                DiagnosticExportRequest::ReadSealed,
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                },
            ] {
                let (mut stream, _) = listener.accept().await.unwrap();
                assert_eq!(receive_request(&mut stream).await, expected);
                stream.write_all(&partial).await.unwrap();
            }
        });
        let client = test_client(path);
        assert_eq!(
            client.read_sealed().await,
            Err(DiagnosticExportReadError::Retryable {
                stage: DiagnosticExportExchangeStage::InvalidResponse,
            })
        );
        assert_eq!(
            client
                .acknowledge(DiagnosticExportBinding::new(7, 3).unwrap())
                .await,
            Err(DiagnosticExportAcknowledgementError::DeliveryAmbiguous {
                stage: DiagnosticExportExchangeStage::InvalidResponse,
            },)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn acknowledgement_eof_is_ambiguous_and_is_never_retried() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert!(matches!(
                receive_request(&mut stream).await,
                DiagnosticExportRequest::Acknowledge { .. }
            ));
            drop(stream);
            assert!(timeout(TEST_LIMIT * 3, listener.accept()).await.is_err());
        });
        let client = test_client(path);
        assert_eq!(
            client
                .acknowledge(DiagnosticExportBinding::new(7, 3).unwrap())
                .await,
            Err(DiagnosticExportAcknowledgementError::DeliveryAmbiguous {
                stage: DiagnosticExportExchangeStage::ResponseClosed,
            },)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn read_eof_is_retryable_but_is_never_retried_implicitly() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                receive_request(&mut stream).await,
                DiagnosticExportRequest::ReadSealed
            );
            drop(stream);
            assert!(timeout(TEST_LIMIT * 3, listener.accept()).await.is_err());
        });
        let client = test_client(path);
        assert_eq!(
            client.read_sealed().await,
            Err(DiagnosticExportReadError::Retryable {
                stage: DiagnosticExportExchangeStage::ResponseClosed,
            })
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_drops_the_connection_before_an_explicit_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (request_seen_sender, request_seen) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            assert_eq!(
                receive_request(&mut first).await,
                DiagnosticExportRequest::ReadSealed
            );
            request_seen_sender.send(()).unwrap();
            let mut eof = [0_u8; 1];
            assert_eq!(first.read(&mut eof).await.unwrap(), 0);
            let (mut second, _) = listener.accept().await.unwrap();
            assert_eq!(
                receive_request(&mut second).await,
                DiagnosticExportRequest::ReadSealed
            );
            second
                .write_all(&encoded(DiagnosticExportResponse::NoSealed))
                .await
                .unwrap();
        });
        let client = Arc::new(test_client(path));
        let reading = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.read_sealed().await })
        };
        request_seen.await.unwrap();
        reading.abort();
        let _ = reading.await;
        assert_eq!(
            client.read_sealed().await,
            Ok(DiagnosticExportReadOutcome::NoSealed)
        );
        timeout(TEST_WAIT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn acknowledgement_cancellation_drops_one_sent_connection_without_retry() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("export.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (request_seen_sender, request_seen) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            assert_eq!(
                receive_request(&mut stream).await,
                DiagnosticExportRequest::Acknowledge {
                    instance_id: 7,
                    window_id: 3,
                }
            );
            request_seen_sender.send(()).unwrap();
            let mut eof = [0_u8; 1];
            assert_eq!(stream.read(&mut eof).await.unwrap(), 0);
            assert!(timeout(TEST_LIMIT * 3, listener.accept()).await.is_err());
        });
        let client = Arc::new(test_client(path));
        let acknowledging = {
            let client = Arc::clone(&client);
            tokio::spawn(async move {
                client
                    .acknowledge(DiagnosticExportBinding::new(7, 3).unwrap())
                    .await
            })
        };
        request_seen.await.unwrap();
        acknowledging.abort();
        let _ = acknowledging.await;
        timeout(TEST_WAIT, server).await.unwrap().unwrap();
    }

    #[test]
    fn public_defaults_are_fixed_bounded_and_do_not_expose_payload_in_debug() {
        let client = DiagnosticExportClient::new();
        assert_eq!(client.socket_path, PathBuf::from(DIAGNOSTIC_EXPORT_SOCKET));
        assert!(ClientLimits::PRODUCTION.valid());
        assert_eq!(MAX_DIAGNOSTIC_EXPORT_PAYLOAD_BYTES, 8_192);
    }

    fn response_frame(bytes: &[u8]) -> ResponseFrame {
        let mut frame = ResponseFrame {
            bytes: [0; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES],
            length: bytes.len(),
        };
        frame.bytes[..bytes.len()].copy_from_slice(bytes);
        frame
    }

    struct ReadFailureIo;

    impl AsyncRead for ReadFailureIo {
        fn poll_read(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            _buffer: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::other("test read failure")))
        }
    }

    impl AsyncWrite for ReadFailureIo {
        fn poll_write(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
            buffer: &[u8],
        ) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(buffer.len()))
        }

        fn poll_flush(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }

        fn poll_shutdown(self: Pin<&mut Self>, _context: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
}
