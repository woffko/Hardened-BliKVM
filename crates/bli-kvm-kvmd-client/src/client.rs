use std::{path::PathBuf, time::Duration};

use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::{net::UnixStream, time::timeout};

use crate::{
    KvmdAtxState, KvmdCredentials, KvmdHidState, KvmdInfoState, KvmdInventory, KvmdMsdState,
    KvmdStreamerState,
};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum KvmdClientError {
    #[error("KVMD Unix socket path must be absolute")]
    RelativeSocketPath,
    #[error("KVMD response size limit must be greater than zero")]
    InvalidResponseLimit,
    #[error("KVMD request timed out")]
    Timeout,
    #[error("KVMD Unix socket I/O failed")]
    Io(#[from] std::io::Error),
    #[error("KVMD HTTP request construction failed")]
    Http(#[from] http::Error),
    #[error("KVMD HTTP protocol failed")]
    Hyper(#[from] hyper::Error),
    #[error("KVMD returned HTTP status {0}")]
    Status(StatusCode),
    #[error("KVMD response exceeded the configured size limit")]
    ResponseTooLarge,
    #[error("KVMD returned invalid JSON")]
    InvalidJson(#[from] serde_json::Error),
    #[error("KVMD rejected the API request")]
    ApiRejected,
    #[error("KVMD response did not contain a result")]
    MissingResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KvmdReadEndpoint {
    Info,
    Atx,
    Hid,
    Msd,
    Streamer,
}

impl KvmdReadEndpoint {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Atx => "atx",
            Self::Hid => "hid",
            Self::Msd => "msd",
            Self::Streamer => "streamer",
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("KVMD {endpoint:?} inventory endpoint failed")]
pub struct KvmdInventoryError {
    pub endpoint: KvmdReadEndpoint,
    #[source]
    pub source: KvmdClientError,
}

impl KvmdInventoryError {
    fn new(endpoint: KvmdReadEndpoint, source: KvmdClientError) -> Self {
        Self { endpoint, source }
    }
}

#[derive(Debug, Clone)]
pub struct KvmdReadClient {
    socket_path: PathBuf,
    timeout: Duration,
    max_response_bytes: usize,
}

#[derive(Deserialize)]
struct KvmdEnvelope<T> {
    ok: bool,
    result: Option<T>,
}

impl KvmdReadClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, KvmdClientError> {
        Self::with_limits(socket_path, DEFAULT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES)
    }

    pub fn with_limits(
        socket_path: impl Into<PathBuf>,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, KvmdClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(KvmdClientError::RelativeSocketPath);
        }
        if max_response_bytes == 0 {
            return Err(KvmdClientError::InvalidResponseLimit);
        }
        Ok(Self {
            socket_path,
            timeout,
            max_response_bytes,
        })
    }

    // These are KVMD's internal Unix-socket routes. The public Nginx boundary
    // rewrites `/api/<route>` to `/<route>` before proxying to this socket.
    pub async fn info(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdInfoState, KvmdClientError> {
        self.get_json("/info", credentials).await
    }

    pub async fn atx(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdAtxState, KvmdClientError> {
        self.get_json("/atx", credentials).await
    }

    pub async fn hid(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdHidState, KvmdClientError> {
        self.get_json("/hid", credentials).await
    }

    pub async fn msd(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdMsdState, KvmdClientError> {
        self.get_json("/msd", credentials).await
    }

    pub async fn streamer(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdStreamerState, KvmdClientError> {
        self.get_json("/streamer", credentials).await
    }

    pub async fn inventory(
        &self,
        credentials: &KvmdCredentials,
    ) -> Result<KvmdInventory, KvmdInventoryError> {
        let (info, atx, hid, msd, streamer) = tokio::try_join!(
            async {
                self.info(credentials)
                    .await
                    .map_err(|source| KvmdInventoryError::new(KvmdReadEndpoint::Info, source))
            },
            async {
                self.atx(credentials)
                    .await
                    .map_err(|source| KvmdInventoryError::new(KvmdReadEndpoint::Atx, source))
            },
            async {
                self.hid(credentials)
                    .await
                    .map_err(|source| KvmdInventoryError::new(KvmdReadEndpoint::Hid, source))
            },
            async {
                self.msd(credentials)
                    .await
                    .map_err(|source| KvmdInventoryError::new(KvmdReadEndpoint::Msd, source))
            },
            async {
                self.streamer(credentials)
                    .await
                    .map_err(|source| KvmdInventoryError::new(KvmdReadEndpoint::Streamer, source))
            },
        )?;
        Ok(KvmdInventory {
            info,
            atx,
            hid,
            msd,
            streamer,
        })
    }

    async fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        path: &'static str,
        credentials: &KvmdCredentials,
    ) -> Result<T, KvmdClientError> {
        timeout(self.timeout, self.get_json_inner(path, credentials))
            .await
            .map_err(|_| KvmdClientError::Timeout)?
    }

    async fn get_json_inner<T: for<'de> Deserialize<'de>>(
        &self,
        path: &'static str,
        credentials: &KvmdCredentials,
    ) -> Result<T, KvmdClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        let connection_task = tokio::spawn(connection);
        let mut request = Request::builder()
            .method(Method::GET)
            .uri(path)
            .header(header::ACCEPT, "application/json")
            .header(header::HOST, "localhost")
            .header(header::USER_AGENT, "Hardened-BliKVM-broker/0.1")
            .body(Empty::<Bytes>::new())?;
        credentials.apply_request_headers(request.headers_mut());
        let response = sender.send_request(request).await?;
        drop(sender);

        if response.status() != StatusCode::OK {
            connection_task.abort();
            return Err(KvmdClientError::Status(response.status()));
        }
        if response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.max_response_bytes)
        {
            connection_task.abort();
            return Err(KvmdClientError::ResponseTooLarge);
        }

        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame?.into_data() {
                if bytes.len().saturating_add(data.len()) > self.max_response_bytes {
                    connection_task.abort();
                    return Err(KvmdClientError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        connection_task.abort();

        let envelope: KvmdEnvelope<T> = serde_json::from_slice(&bytes)?;
        if !envelope.ok {
            return Err(KvmdClientError::ApiRejected);
        }
        envelope.result.ok_or(KvmdClientError::MissingResult)
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

    async fn serve_once(
        socket: &Path,
        status: &str,
        body: &str,
    ) -> tokio::task::JoinHandle<String> {
        let listener = UnixListener::bind(socket).unwrap();
        let status = status.to_owned();
        let body = body.to_owned();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        })
    }

    fn fake_credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-operator", "not-a-real-password").unwrap()
    }

    #[tokio::test]
    async fn reads_typed_state_over_the_unix_socket() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_once(
            &socket,
            "200 OK",
            r#"{"ok":true,"result":{"enabled":true,"busy":false,"leds":{"power":true}}}"#,
        )
        .await;
        let client = KvmdReadClient::new(&socket).unwrap();
        let state = client.atx(&fake_credentials()).await.unwrap();
        assert_eq!(state.enabled, Some(true));
        assert_eq!(state.leds.power, Some(true));

        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /atx http/1.1\r\n"));
        assert!(request.contains("host: localhost\r\n"));
        assert!(request.contains("x-kvmd-user: test-operator\r\n"));
        assert!(request.contains("x-kvmd-passwd: not-a-real-password\r\n"));
    }

    #[tokio::test]
    async fn rejects_an_oversized_response_before_parsing() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_once(
            &socket,
            "200 OK",
            r#"{"ok":true,"result":{"padding":"too large"}}"#,
        )
        .await;
        let client = KvmdReadClient::with_limits(&socket, Duration::from_secs(1), 16).unwrap();
        assert!(matches!(
            client.info(&fake_credentials()).await,
            Err(KvmdClientError::ResponseTooLarge)
        ));
        let _ = server.await.unwrap();
    }

    #[test]
    fn rejects_relative_socket_paths_and_zero_limits() {
        assert!(matches!(
            KvmdReadClient::new("relative.sock"),
            Err(KvmdClientError::RelativeSocketPath)
        ));
        assert!(matches!(
            KvmdReadClient::with_limits("/run/kvmd/kvmd.sock", Duration::from_secs(1), 0),
            Err(KvmdClientError::InvalidResponseLimit)
        ));
    }
}
