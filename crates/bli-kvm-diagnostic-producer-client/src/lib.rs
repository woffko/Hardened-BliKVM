#![forbid(unsafe_code)]

//! Fixed-path typed producer client for local diagnostic events.
//!
//! The client serializes one persistent Unix connection and waits for one exact
//! response after every event. It never retries automatically. Once a frame
//! write has begun, any write or response failure is conservatively reported as
//! delivery-ambiguous.

use std::{
    error::Error,
    fmt,
    future::Future,
    io::{self, ErrorKind},
    path::{Path, PathBuf},
    time::Duration,
};

use bli_kvm_diagnostic_protocol::{
    DIAGNOSTIC_EVENT_FRAME_BYTES, DIAGNOSTIC_RESPONSE_FRAME_BYTES, DiagnosticEvent,
    DiagnosticProducerRejection, DiagnosticProducerResponse, decode_response, encode_event,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::UnixStream,
    sync::Mutex,
    time::timeout,
};

pub const DIAGNOSTIC_COLLECTOR_SOCKET: &str = "/run/hardened-blikvm-diagnostics/collector.sock";
pub const DIAGNOSTIC_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
pub const DIAGNOSTIC_WRITE_TIMEOUT: Duration = Duration::from_secs(2);
pub const DIAGNOSTIC_RESPONSE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticNotSentReason {
    InvalidEvent,
    ConnectTimeout,
    ConnectFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticAmbiguousStage {
    WriteTimeout,
    WriteFailed,
    ResponseTimeout,
    ResponseClosed,
    ResponseReadFailed,
    InvalidResponse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSendError {
    NotSent { reason: DiagnosticNotSentReason },
    DeliveryAmbiguous { stage: DiagnosticAmbiguousStage },
}

impl fmt::Display for DiagnosticSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSent { .. } => formatter.write_str("diagnostic event was not sent"),
            Self::DeliveryAmbiguous { .. } => {
                formatter.write_str("diagnostic event delivery is ambiguous")
            }
        }
    }
}

impl Error for DiagnosticSendError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticSendOutcome {
    Accepted,
    Rejected { reason: DiagnosticProducerRejection },
}

pub struct DiagnosticProducerClient {
    socket_path: PathBuf,
    limits: ClientLimits,
    stream: Mutex<Option<UnixStream>>,
}

impl DiagnosticProducerClient {
    pub fn new() -> Self {
        Self::with_parts(
            PathBuf::from(DIAGNOSTIC_COLLECTOR_SOCKET),
            ClientLimits::PRODUCTION,
        )
    }

    pub async fn send(
        &self,
        event: DiagnosticEvent,
    ) -> Result<DiagnosticSendOutcome, DiagnosticSendError> {
        let frame = encode_event(event).map_err(|_| DiagnosticSendError::NotSent {
            reason: DiagnosticNotSentReason::InvalidEvent,
        })?;
        let mut state = self.stream.lock().await;
        let mut stream = match state.take() {
            Some(stream) => stream,
            None => connect(&self.socket_path, self.limits.connect_timeout).await?,
        };
        let response = exchange(
            &mut stream,
            &frame,
            self.limits.write_timeout,
            self.limits.response_timeout,
        )
        .await?;
        *state = Some(stream);
        Ok(match response {
            DiagnosticProducerResponse::Accepted => DiagnosticSendOutcome::Accepted,
            DiagnosticProducerResponse::Rejected { reason } => {
                DiagnosticSendOutcome::Rejected { reason }
            }
        })
    }

    fn with_parts(socket_path: PathBuf, limits: ClientLimits) -> Self {
        debug_assert!(socket_path.is_absolute());
        debug_assert!(limits.valid());
        Self {
            socket_path,
            limits,
            stream: Mutex::new(None),
        }
    }
}

impl Default for DiagnosticProducerClient {
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
        connect_timeout: DIAGNOSTIC_CONNECT_TIMEOUT,
        write_timeout: DIAGNOSTIC_WRITE_TIMEOUT,
        response_timeout: DIAGNOSTIC_RESPONSE_TIMEOUT,
    };

    const fn valid(self) -> bool {
        !self.connect_timeout.is_zero()
            && !self.write_timeout.is_zero()
            && !self.response_timeout.is_zero()
    }
}

async fn connect(path: &Path, limit: Duration) -> Result<UnixStream, DiagnosticSendError> {
    connect_future(UnixStream::connect(path), limit).await
}

async fn connect_future<F>(future: F, limit: Duration) -> Result<UnixStream, DiagnosticSendError>
where
    F: Future<Output = io::Result<UnixStream>>,
{
    match timeout(limit, future).await {
        Err(_) => Err(DiagnosticSendError::NotSent {
            reason: DiagnosticNotSentReason::ConnectTimeout,
        }),
        Ok(Err(_)) => Err(DiagnosticSendError::NotSent {
            reason: DiagnosticNotSentReason::ConnectFailed,
        }),
        Ok(Ok(stream)) => Ok(stream),
    }
}

async fn exchange<S>(
    stream: &mut S,
    frame: &[u8; DIAGNOSTIC_EVENT_FRAME_BYTES],
    write_timeout: Duration,
    response_timeout: Duration,
) -> Result<DiagnosticProducerResponse, DiagnosticSendError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match timeout(write_timeout, stream.write_all(frame)).await {
        Err(_) => {
            return Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::WriteTimeout,
            });
        }
        Ok(Err(_)) => {
            return Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::WriteFailed,
            });
        }
        Ok(Ok(())) => {}
    }

    let mut response = [0_u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES];
    match timeout(response_timeout, stream.read_exact(&mut response)).await {
        Err(_) => {
            return Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseTimeout,
            });
        }
        Ok(Err(error)) if error.kind() == ErrorKind::UnexpectedEof => {
            return Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseClosed,
            });
        }
        Ok(Err(_)) => {
            return Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseReadFailed,
            });
        }
        Ok(Ok(_)) => {}
    }
    decode_response(&response).map_err(|_| DiagnosticSendError::DeliveryAmbiguous {
        stage: DiagnosticAmbiguousStage::InvalidResponse,
    })
}

#[cfg(test)]
mod tests {
    use std::{
        future::pending,
        pin::Pin,
        sync::Arc,
        task::{Context, Poll},
    };

    use bli_kvm_diagnostic_protocol::{DiagnosticProducerRejection, RejectReason, encode_response};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt, ReadBuf, duplex},
        net::UnixListener,
        sync::oneshot,
        time::{sleep, timeout},
    };

    use super::*;

    const TEST_LIMIT: Duration = Duration::from_millis(30);
    const TEST_WAIT: Duration = Duration::from_millis(500);

    fn test_client(path: PathBuf) -> DiagnosticProducerClient {
        DiagnosticProducerClient::with_parts(
            path,
            ClientLimits {
                connect_timeout: TEST_LIMIT,
                write_timeout: TEST_LIMIT,
                response_timeout: TEST_LIMIT,
            },
        )
    }

    #[tokio::test]
    async fn reuses_one_connection_for_two_exact_accepted_events() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for expected in [DiagnosticEvent::Ready, DiagnosticEvent::Unavailable] {
                let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
                stream.read_exact(&mut frame).await.unwrap();
                assert_eq!(frame, encode_event(expected).unwrap());
                stream
                    .write_all(&encode_response(DiagnosticProducerResponse::Accepted))
                    .await
                    .unwrap();
            }
        });
        let client = test_client(path);
        assert_eq!(
            client.send(DiagnosticEvent::Ready).await,
            Ok(DiagnosticSendOutcome::Accepted)
        );
        assert_eq!(
            client.send(DiagnosticEvent::Unavailable).await,
            Ok(DiagnosticSendOutcome::Accepted)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn preserves_every_typed_rejection_without_reconnecting() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let reasons = [
            DiagnosticProducerRejection::EventNotAllowed,
            DiagnosticProducerRejection::Backpressure,
            DiagnosticProducerRejection::CollectorUnavailable,
        ];
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for reason in reasons {
                let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
                stream.read_exact(&mut frame).await.unwrap();
                stream
                    .write_all(&encode_response(DiagnosticProducerResponse::Rejected {
                        reason,
                    }))
                    .await
                    .unwrap();
            }
        });
        let client = test_client(path);
        for reason in reasons {
            assert_eq!(
                client.send(DiagnosticEvent::Ready).await,
                Ok(DiagnosticSendOutcome::Rejected { reason })
            );
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_event_and_connect_failure_are_definitely_not_sent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.sock");
        let client = test_client(path);
        assert_eq!(
            client.send(DiagnosticEvent::Started).await,
            Err(DiagnosticSendError::NotSent {
                reason: DiagnosticNotSentReason::InvalidEvent
            })
        );
        assert_eq!(
            client.send(DiagnosticEvent::Ready).await,
            Err(DiagnosticSendError::NotSent {
                reason: DiagnosticNotSentReason::ConnectFailed
            })
        );
    }

    #[tokio::test]
    async fn pending_connect_is_classified_before_any_write() {
        assert!(matches!(
            connect_future(pending::<io::Result<UnixStream>>(), TEST_LIMIT).await,
            Err(DiagnosticSendError::NotSent {
                reason: DiagnosticNotSentReason::ConnectTimeout
            })
        ));
    }

    #[tokio::test]
    async fn write_timeout_is_delivery_ambiguous() {
        let (mut producer, _collector) = duplex(1);
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        assert_eq!(
            exchange(&mut producer, &frame, TEST_LIMIT, TEST_LIMIT).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::WriteTimeout
            })
        );
    }

    #[tokio::test]
    async fn write_and_non_eof_read_errors_are_ambiguous() {
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        let (mut producer, collector) = duplex(64);
        drop(collector);
        assert_eq!(
            exchange(&mut producer, &frame, TEST_LIMIT, TEST_LIMIT).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::WriteFailed
            })
        );

        assert_eq!(
            exchange(&mut ReadFailureIo, &frame, TEST_LIMIT, TEST_LIMIT).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseReadFailed
            })
        );
    }

    #[tokio::test]
    async fn response_timeout_and_eof_are_delivery_ambiguous() {
        let frame = encode_event(DiagnosticEvent::Ready).unwrap();
        let (mut producer, mut collector) = duplex(64);
        let silent = tokio::spawn(async move {
            let mut received = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
            collector.read_exact(&mut received).await.unwrap();
            sleep(TEST_LIMIT * 3).await;
        });
        assert_eq!(
            exchange(&mut producer, &frame, TEST_LIMIT, TEST_LIMIT).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseTimeout
            })
        );
        silent.abort();

        let (mut producer, mut collector) = duplex(64);
        let closed = tokio::spawn(async move {
            let mut received = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
            collector.read_exact(&mut received).await.unwrap();
        });
        assert_eq!(
            exchange(&mut producer, &frame, TEST_LIMIT, TEST_LIMIT).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::ResponseClosed
            })
        );
        closed.await.unwrap();
    }

    #[tokio::test]
    async fn malformed_response_is_ambiguous_and_next_explicit_send_reconnects() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
            first.read_exact(&mut frame).await.unwrap();
            first.write_all(&[0, 0, 0, 0]).await.unwrap();
            drop(first);
            let (mut second, _) = listener.accept().await.unwrap();
            second.read_exact(&mut frame).await.unwrap();
            second
                .write_all(&encode_response(DiagnosticProducerResponse::Accepted))
                .await
                .unwrap();
        });
        let client = test_client(path);
        assert_eq!(
            client.send(DiagnosticEvent::Ready).await,
            Err(DiagnosticSendError::DeliveryAmbiguous {
                stage: DiagnosticAmbiguousStage::InvalidResponse
            })
        );
        assert_eq!(
            client.send(DiagnosticEvent::Unavailable).await,
            Ok(DiagnosticSendOutcome::Accepted)
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cancellation_drops_in_flight_stream_before_next_explicit_send() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let (frame_seen_sender, frame_seen) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut first, _) = listener.accept().await.unwrap();
            let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
            first.read_exact(&mut frame).await.unwrap();
            frame_seen_sender.send(()).unwrap();
            let mut eof = [0_u8; 1];
            assert_eq!(first.read(&mut eof).await.unwrap(), 0);
            let (mut second, _) = listener.accept().await.unwrap();
            second.read_exact(&mut frame).await.unwrap();
            second
                .write_all(&encode_response(DiagnosticProducerResponse::Accepted))
                .await
                .unwrap();
        });
        let client = Arc::new(test_client(path));
        let sending = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.send(DiagnosticEvent::Ready).await })
        };
        frame_seen.await.unwrap();
        sending.abort();
        let _ = sending.await;
        assert_eq!(
            client.send(DiagnosticEvent::Unavailable).await,
            Ok(DiagnosticSendOutcome::Accepted)
        );
        timeout(TEST_WAIT, server).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn concurrent_callers_are_serialized_into_whole_frames() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("collector.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            for _ in 0..2 {
                let mut frame = [0_u8; DIAGNOSTIC_EVENT_FRAME_BYTES];
                stream.read_exact(&mut frame).await.unwrap();
                assert!(
                    frame == encode_event(DiagnosticEvent::Ready).unwrap()
                        || frame == encode_event(DiagnosticEvent::Unavailable).unwrap()
                );
                stream
                    .write_all(&encode_response(DiagnosticProducerResponse::Accepted))
                    .await
                    .unwrap();
            }
        });
        let client = Arc::new(test_client(path));
        let first = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.send(DiagnosticEvent::Ready).await })
        };
        let second = {
            let client = Arc::clone(&client);
            tokio::spawn(async move { client.send(DiagnosticEvent::Unavailable).await })
        };
        assert_eq!(first.await.unwrap(), Ok(DiagnosticSendOutcome::Accepted));
        assert_eq!(second.await.unwrap(), Ok(DiagnosticSendOutcome::Accepted));
        server.await.unwrap();
    }

    #[test]
    fn public_defaults_are_fixed_and_nonzero() {
        let client = DiagnosticProducerClient::new();
        assert_eq!(
            client.socket_path,
            PathBuf::from(DIAGNOSTIC_COLLECTOR_SOCKET)
        );
        assert!(ClientLimits::PRODUCTION.valid());
        assert_eq!(
            encode_event(DiagnosticEvent::RequestRejected {
                reason: RejectReason::Unauthorized
            })
            .unwrap()
            .len(),
            DIAGNOSTIC_EVENT_FRAME_BYTES
        );
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
