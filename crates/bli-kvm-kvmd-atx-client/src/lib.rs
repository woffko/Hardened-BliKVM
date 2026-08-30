//! Fixed-route ATX transition client for KVMD 3.291's internal Unix socket.
//!
//! This crate has no generic URI method and is not connected to the public
//! server or the denied-by-default control broker.

use std::{path::PathBuf, time::Duration};

use bli_kvm_core::AtxControlAction;
use bli_kvm_kvmd_client::KvmdCredentials;
use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use thiserror::Error;
use tokio::{net::UnixStream, time::timeout};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum KvmdAtxClientError {
    #[error("KVMD ATX socket path must be absolute")]
    RelativeSocketPath,
    #[error("KVMD ATX timeout must be greater than zero")]
    InvalidTimeout,
    #[error("KVMD ATX response size limit must be greater than zero")]
    InvalidResponseLimit,
    #[error("KVMD ATX request timed out")]
    Timeout,
    #[error("KVMD ATX Unix socket I/O failed")]
    Io(#[from] std::io::Error),
    #[error("KVMD ATX HTTP request construction failed")]
    Http(#[from] http::Error),
    #[error("KVMD ATX HTTP protocol failed")]
    Hyper(#[from] hyper::Error),
    #[error("KVMD ATX returned HTTP status {0}")]
    Status(StatusCode),
    #[error("KVMD ATX response exceeded the configured size limit")]
    ResponseTooLarge,
    #[error("KVMD ATX response content type is invalid")]
    InvalidContentType,
    #[error("KVMD ATX returned invalid JSON")]
    InvalidJson(#[from] serde_json::Error),
    #[error("KVMD rejected the ATX action")]
    ApiRejected,
}

#[derive(Debug, Clone)]
pub struct KvmdAtxClient {
    socket_path: PathBuf,
    timeout: Duration,
    max_response_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct KvmdMutationEnvelope {
    ok: bool,
}

impl KvmdAtxClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, KvmdAtxClientError> {
        Self::with_limits(socket_path, DEFAULT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES)
    }

    pub fn with_limits(
        socket_path: impl Into<PathBuf>,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, KvmdAtxClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(KvmdAtxClientError::RelativeSocketPath);
        }
        if timeout.is_zero() {
            return Err(KvmdAtxClientError::InvalidTimeout);
        }
        if max_response_bytes == 0 {
            return Err(KvmdAtxClientError::InvalidResponseLimit);
        }
        Ok(Self {
            socket_path,
            timeout,
            max_response_bytes,
        })
    }

    pub async fn execute(
        &self,
        action: AtxControlAction,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdAtxClientError> {
        timeout(self.timeout, self.execute_inner(action, credentials))
            .await
            .map_err(|_| KvmdAtxClientError::Timeout)?
    }

    async fn execute_inner(
        &self,
        action: AtxControlAction,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdAtxClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        let _connection_task = AbortTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(action_uri(action))
            .header(header::ACCEPT, "application/json")
            .header(header::HOST, "localhost")
            .header(header::USER_AGENT, "Hardened-BliKVM-atx-broker/0.1")
            .body(Empty::<Bytes>::new())?;
        credentials.apply_request_headers(request.headers_mut());
        let response = sender.send_request(request).await?;
        drop(sender);

        if response.status() != StatusCode::OK {
            return Err(KvmdAtxClientError::Status(response.status()));
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
            return Err(KvmdAtxClientError::InvalidContentType);
        }
        if response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.max_response_bytes)
        {
            return Err(KvmdAtxClientError::ResponseTooLarge);
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame?.into_data() {
                if bytes.len().saturating_add(data.len()) > self.max_response_bytes {
                    return Err(KvmdAtxClientError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        let response: KvmdMutationEnvelope = serde_json::from_slice(&bytes)?;
        if !response.ok {
            return Err(KvmdAtxClientError::ApiRejected);
        }
        Ok(())
    }
}

fn action_uri(action: AtxControlAction) -> &'static str {
    match action {
        AtxControlAction::PowerShort => "/atx/click?button=power&wait=1",
        AtxControlAction::PowerLong => "/atx/click?button=power_long&wait=1",
        AtxControlAction::Reset => "/atx/click?button=reset&wait=1",
    }
}

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

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
    async fn sends_only_three_fixed_waiting_atx_routes() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_requests(&socket, 3).await;
        let client = KvmdAtxClient::new(&socket).unwrap();
        for action in [
            AtxControlAction::PowerShort,
            AtxControlAction::PowerLong,
            AtxControlAction::Reset,
        ] {
            client.execute(action, &credentials()).await.unwrap();
        }
        let requests = server.await.unwrap();
        let expected = [
            "/atx/click?button=power&wait=1",
            "/atx/click?button=power_long&wait=1",
            "/atx/click?button=reset&wait=1",
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
            KvmdAtxClient::new("kvmd.sock"),
            Err(KvmdAtxClientError::RelativeSocketPath)
        ));
        assert!(matches!(
            KvmdAtxClient::with_limits("/run/kvmd.sock", Duration::ZERO, 1),
            Err(KvmdAtxClientError::InvalidTimeout)
        ));
    }
}
