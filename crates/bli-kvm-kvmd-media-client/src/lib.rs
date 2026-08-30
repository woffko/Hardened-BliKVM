//! Fixed-route virtual-media transition client for KVMD 3.291.
//!
//! Image names can only come from a validated private opaque-ID catalog. This
//! crate exposes no upload, download, remove, reset, generic URI, or raw path.

use std::{path::PathBuf, time::Duration};

use bli_kvm_core::{VirtualMediaCommand, VirtualMediaControlMode};
use bli_kvm_kvmd_client::KvmdCredentials;
use bli_kvm_media_catalog::{MediaCatalog, MediaCatalogError};
use bytes::Bytes;
use http::{Method, Request, StatusCode, header};
use http_body_util::{BodyExt, Empty};
use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use thiserror::Error;
use tokio::{net::UnixStream, time::timeout};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum KvmdMediaClientError {
    #[error("KVMD media socket path must be absolute")]
    RelativeSocketPath,
    #[error("KVMD media timeout must be greater than zero")]
    InvalidTimeout,
    #[error("KVMD media response size limit must be greater than zero")]
    InvalidResponseLimit,
    #[error("KVMD media catalog rejected the selection")]
    Catalog(#[from] MediaCatalogError),
    #[error("KVMD media request timed out")]
    Timeout,
    #[error("KVMD media Unix socket I/O failed")]
    Io(#[from] std::io::Error),
    #[error("KVMD media HTTP request construction failed")]
    Http(#[from] http::Error),
    #[error("KVMD media HTTP protocol failed")]
    Hyper(#[from] hyper::Error),
    #[error("KVMD media returned HTTP status {0}")]
    Status(StatusCode),
    #[error("KVMD media response exceeded the configured size limit")]
    ResponseTooLarge,
    #[error("KVMD media response content type is invalid")]
    InvalidContentType,
    #[error("KVMD media returned invalid JSON")]
    InvalidJson(#[from] serde_json::Error),
    #[error("KVMD rejected the media command")]
    ApiRejected,
}

#[derive(Debug, Clone)]
pub struct KvmdMediaClient {
    socket_path: PathBuf,
    timeout: Duration,
    max_response_bytes: usize,
}

#[derive(Debug, Deserialize)]
struct KvmdMutationEnvelope {
    ok: bool,
}

impl KvmdMediaClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, KvmdMediaClientError> {
        Self::with_limits(socket_path, DEFAULT_TIMEOUT, DEFAULT_MAX_RESPONSE_BYTES)
    }

    pub fn with_limits(
        socket_path: impl Into<PathBuf>,
        timeout: Duration,
        max_response_bytes: usize,
    ) -> Result<Self, KvmdMediaClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(KvmdMediaClientError::RelativeSocketPath);
        }
        if timeout.is_zero() {
            return Err(KvmdMediaClientError::InvalidTimeout);
        }
        if max_response_bytes == 0 {
            return Err(KvmdMediaClientError::InvalidResponseLimit);
        }
        Ok(Self {
            socket_path,
            timeout,
            max_response_bytes,
        })
    }

    pub async fn execute(
        &self,
        command: &VirtualMediaCommand,
        catalog: &MediaCatalog,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdMediaClientError> {
        let uri = match command {
            VirtualMediaCommand::Select {
                image_id,
                mode,
                writable,
            } => {
                let selection = catalog.resolve(*image_id, *mode, *writable)?;
                selection_uri(
                    selection.image_name(),
                    selection.mode(),
                    selection.writable(),
                )
            }
            VirtualMediaCommand::SetConnected { connected } => {
                format!("/msd/set_connected?connected={}", u8::from(*connected))
            }
        };
        timeout(self.timeout, self.post(&uri, credentials))
            .await
            .map_err(|_| KvmdMediaClientError::Timeout)?
    }

    async fn post(
        &self,
        uri: &str,
        credentials: &KvmdCredentials,
    ) -> Result<(), KvmdMediaClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        let _connection_task = AbortTask(tokio::spawn(async move {
            let _ = connection.await;
        }));
        let mut request = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::ACCEPT, "application/json")
            .header(header::HOST, "localhost")
            .header(header::USER_AGENT, "Hardened-BliKVM-media-broker/0.1")
            .body(Empty::<Bytes>::new())?;
        credentials.apply_request_headers(request.headers_mut());
        let response = sender.send_request(request).await?;
        drop(sender);
        if response.status() != StatusCode::OK {
            return Err(KvmdMediaClientError::Status(response.status()));
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
            return Err(KvmdMediaClientError::InvalidContentType);
        }
        if response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<usize>().ok())
            .is_some_and(|length| length > self.max_response_bytes)
        {
            return Err(KvmdMediaClientError::ResponseTooLarge);
        }
        let mut body = response.into_body();
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            if let Ok(data) = frame?.into_data() {
                if bytes.len().saturating_add(data.len()) > self.max_response_bytes {
                    return Err(KvmdMediaClientError::ResponseTooLarge);
                }
                bytes.extend_from_slice(&data);
            }
        }
        let response: KvmdMutationEnvelope = serde_json::from_slice(&bytes)?;
        if !response.ok {
            return Err(KvmdMediaClientError::ApiRejected);
        }
        Ok(())
    }
}

fn selection_uri(name: &str, mode: VirtualMediaControlMode, writable: bool) -> String {
    format!(
        "/msd/set_params?image={name}&cdrom={}&rw={}",
        u8::from(mode == VirtualMediaControlMode::CdRom),
        u8::from(writable)
    )
}

struct AbortTask(tokio::task::JoinHandle<()>);

impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixListener,
    };

    use super::*;

    fn catalog(directory: &Path) -> MediaCatalog {
        let path = directory.join("catalog.json");
        fs::write(
            &path,
            r#"{"schemaVersion":1,"entries":[{"imageId":7,"imageName":"rescue.iso","size":4096,"sha256":"1111111111111111111111111111111111111111111111111111111111111111","cdromAllowed":true,"removableAllowed":false,"writableAllowed":false}]}"#,
        )
        .unwrap();
        MediaCatalog::open(path).unwrap()
    }

    fn credentials() -> KvmdCredentials {
        KvmdCredentials::new("test-user", "not-a-real-password").unwrap()
    }

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

    #[tokio::test]
    async fn sends_only_catalog_selection_and_connection_routes() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = serve_requests(&socket, 2).await;
        let client = KvmdMediaClient::new(&socket).unwrap();
        let catalog = catalog(directory.path());
        client
            .execute(
                &VirtualMediaCommand::Select {
                    image_id: 7,
                    mode: VirtualMediaControlMode::CdRom,
                    writable: false,
                },
                &catalog,
                &credentials(),
            )
            .await
            .unwrap();
        client
            .execute(
                &VirtualMediaCommand::SetConnected { connected: false },
                &catalog,
                &credentials(),
            )
            .await
            .unwrap();
        let requests = server.await.unwrap();
        assert!(
            requests[0]
                .starts_with("POST /msd/set_params?image=rescue.iso&cdrom=1&rw=0 HTTP/1.1\r\n")
        );
        assert!(requests[1].starts_with("POST /msd/set_connected?connected=0 HTTP/1.1\r\n"));
        for request in requests {
            assert!(!request.contains("/msd/write"));
            assert!(!request.contains("/msd/remove"));
            assert!(!request.contains("/msd/reset"));
            assert!(!request.contains("/api/"));
        }
    }

    #[tokio::test]
    async fn missing_catalog_id_fails_before_backend_connect() {
        let directory = tempfile::tempdir().unwrap();
        let client = KvmdMediaClient::new(directory.path().join("missing.sock")).unwrap();
        assert!(matches!(
            client
                .execute(
                    &VirtualMediaCommand::Select {
                        image_id: 8,
                        mode: VirtualMediaControlMode::CdRom,
                        writable: false,
                    },
                    &catalog(directory.path()),
                    &credentials()
                )
                .await,
            Err(KvmdMediaClientError::Catalog(MediaCatalogError::NotFound))
        ));
    }
}
