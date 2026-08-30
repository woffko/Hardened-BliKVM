//! Fixed-route HID transition client for KVMD 3.291's internal Unix socket.
//!
//! The client has no generic URI method. Construction alone does not enable a
//! broker backend or the public HID capability.

use std::{path::PathBuf, time::Duration};

use bli_kvm_hid_adapter::KvmdHidEvent;
use bli_kvm_kvmd_client::KvmdCredentials;
use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use thiserror::Error;
use tokio::{net::UnixStream, time::timeout};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;
pub const MAX_HID_BATCH_EVENTS: usize = 32;

#[derive(Debug, Error)]
pub enum KvmdHidClientError {
    #[error("KVMD HID socket path must be absolute")]
    RelativeSocketPath,
    #[error("KVMD HID response size limit must be greater than zero")]
    InvalidResponseLimit,
    #[error("KVMD HID timeout must be greater than zero")]
    InvalidTimeout,
    #[error("KVMD HID request timed out")]
    Timeout,
    #[error("KVMD HID batch exceeded the event limit")]
    BatchTooLarge,
    #[error("KVMD HID Unix socket I/O failed")]
    Io(#[from] std::io::Error),
    #[error("KVMD HID HTTP request construction failed")]
    Http(#[from] http::Error),
    #[error("KVMD HID HTTP protocol failed")]
    Hyper(#[from] hyper::Error),
    #[error("KVMD HID returned HTTP status {0}")]
    Status(StatusCode),
    #[error("KVMD HID response exceeded the configured size limit")]
    ResponseTooLarge,
    #[error("KVMD HID response content type is invalid")]
    InvalidContentType,
    #[error("KVMD HID returned invalid JSON")]
    InvalidJson(#[from] serde_json::Error),
    #[error("KVMD rejected the HID event")]
    ApiRejected,
}

#[derive(Debug, Clone)]
pub struct KvmdHidClient {
    socket_path: PathBuf,
    timeout: Duration,
    max_response_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct KvmdMutationEnvelope {
    ok: bool,
}

impl KvmdHidClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, KvmdHidClientError> {
        Self::with_limits(socket_path, DEFAULT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES)
    }

    pub fn with_limits(
        socket_path: impl Into<PathBuf>,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, KvmdHidClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(KvmdHidClientError::RelativeSocketPath);
        }
        if timeout.is_zero() {
            return Err(KvmdHidClientError::InvalidTimeout);
        }
        if max_response_bytes == 0 {
            return Err(KvmdHidClientError::InvalidResponseLimit);
        }
        Ok(Self {
            socket_path,
            timeout,
            max_response_bytes,
        })
    }

    pub async fn execute(
        &self,
        event: &KvmdHidEvent,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdHidClientError> {
        timeout(self.timeout, self.execute_inner(event, credentials))
            .await
            .map_err(|_| KvmdHidClientError::Timeout)?
    }

    pub async fn execute_batch(
        &self,
        events: &[KvmdHidEvent],
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdHidClientError> {
        if events.len() > MAX_HID_BATCH_EVENTS {
            return Err(KvmdHidClientError::BatchTooLarge);
        }
        for event in events {
            self.execute(event, credentials).await?;
        }
        Ok(())
    }

    async fn execute_inner(
        &self,
        event: &KvmdHidEvent,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdHidClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        let _connection_task = AbortTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(event_uri(event))
            .header(header::ACCEPT, "application/json")
            .header(header::HOST, "localhost")
            .header(header::USER_AGENT, "Hardened-BliKVM-hid-broker/0.1")
            .body(Empty::<Bytes>::new())?;
        credentials.apply_request_headers(request.headers_mut());
        let response = sender.send_request(request).await?;
        drop(sender);

        if response.status() != StatusCode::OK {
            return Err(KvmdHidClientError::Status(response.status()));
        }
        if !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value
                    .split(';')
                    .next()
                    .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("application/json"))
            })
        {
            return Err(KvmdHidClientError::InvalidContentType);
        }
        if response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.max_response_bytes)
        {
            return Err(KvmdHidClientError::ResponseTooLarge);
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame?.into_data() {
                if bytes.len().saturating_add(data.len()) > self.max_response_bytes {
                    return Err(KvmdHidClientError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        let response: KvmdMutationEnvelope = serde_json::from_slice(&bytes)?;
        if !response.ok {
            return Err(KvmdHidClientError::ApiRejected);
        }
        Ok(())
    }
}

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn event_uri(event: &KvmdHidEvent) -> String {
    match event {
        KvmdHidEvent::Key { key, pressed } => format!(
            "/hid/events/send_key?key={}&state={}",
            key.as_str(),
            u8::from(*pressed)
        ),
        KvmdHidEvent::MouseButton { button, pressed } => format!(
            "/hid/events/send_mouse_button?button={}&state={}",
            button.as_str(),
            u8::from(*pressed)
        ),
        KvmdHidEvent::MouseMove { x, y } => {
            format!("/hid/events/send_mouse_move?to_x={x}&to_y={y}")
        }
        KvmdHidEvent::MouseRelative { x, y } => {
            format!("/hid/events/send_mouse_relative?delta_x={x}&delta_y={y}")
        }
        KvmdHidEvent::MouseWheel { x, y } => {
            format!("/hid/events/send_mouse_wheel?delta_x={x}&delta_y={y}")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use bli_kvm_hid_adapter::{KvmdKeyCode, KvmdMouseButton};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    async fn serve_requests(socket: &Path, count: usize) -> tokio::task::JoinHandle<Vec<String>> {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}")
                    .await
                    .unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        })
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

    #[tokio::test]
    async fn sends_only_the_five_fixed_internal_hid_routes() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_requests(&socket, 5).await;
        let client = KvmdHidClient::new(&socket).unwrap();
        let events = [
            KvmdHidEvent::Key {
                key: KvmdKeyCode::from_usb_usage(4).unwrap(),
                pressed: true,
            },
            KvmdHidEvent::MouseButton {
                button: KvmdMouseButton::Right,
                pressed: false,
            },
            KvmdHidEvent::MouseMove {
                x: i16::MIN,
                y: i16::MAX,
            },
            KvmdHidEvent::MouseRelative { x: -1, y: 2 },
            KvmdHidEvent::MouseWheel { x: 0, y: -3 },
        ];
        client.execute_batch(&events, &credentials()).await.unwrap();
        let requests = server.await.unwrap();
        let expected = [
            "/hid/events/send_key?key=KeyA&state=1",
            "/hid/events/send_mouse_button?button=right&state=0",
            "/hid/events/send_mouse_move?to_x=-32768&to_y=32767",
            "/hid/events/send_mouse_relative?delta_x=-1&delta_y=2",
            "/hid/events/send_mouse_wheel?delta_x=0&delta_y=-3",
        ];
        for (request, path) in requests.iter().zip(expected) {
            assert!(request.starts_with(&format!("POST {path} HTTP/1.1\r\n")));
            assert!(request.contains("host: localhost\r\n"));
            assert!(request.contains("x-kvmd-user: test-user\r\n"));
            assert!(request.contains("x-kvmd-passwd: not-a-real-password\r\n"));
            assert!(!request.contains("/api/"));
        }
    }

    #[test]
    fn rejects_relative_paths_and_zero_limits() {
        assert!(matches!(
            KvmdHidClient::new("kvmd.sock"),
            Err(KvmdHidClientError::RelativeSocketPath)
        ));
        assert!(matches!(
            KvmdHidClient::with_limits("/run/kvmd.sock", Duration::ZERO, 1),
            Err(KvmdHidClientError::InvalidTimeout)
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_batches_before_connecting() {
        let client = KvmdHidClient::new("/run/missing-kvmd.sock").unwrap();
        let event = KvmdHidEvent::MouseRelative { x: 0, y: 0 };
        let events = vec![event; MAX_HID_BATCH_EVENTS + 1];
        assert!(matches!(
            client.execute_batch(&events, &credentials()).await,
            Err(KvmdHidClientError::BatchTooLarge)
        ));
    }

    #[tokio::test]
    async fn rejects_non_json_backend_responses() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0_u8; 2048];
            let _ = stream.read(&mut request).await.unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}",
                )
                .await
                .unwrap();
        });
        let client = KvmdHidClient::new(&socket).unwrap();
        assert!(matches!(
            client
                .execute(&KvmdHidEvent::MouseRelative { x: 0, y: 0 }, &credentials())
                .await,
            Err(KvmdHidClientError::InvalidContentType)
        ));
        server.await.unwrap();
    }
}
