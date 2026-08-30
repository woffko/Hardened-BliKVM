//! Authenticated-server client for the isolated local MJPEG proxy.

use std::{
    path::PathBuf,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use http::{HeaderValue, Method, Request, StatusCode, header};
use http_body::{Body as HttpBody, Frame, SizeHint};
use http_body_util::Empty;
use hyper::{body::Incoming, client::conn::http1};
use hyper_util::rt::TokioIo;
use thiserror::Error;
use tokio::{net::UnixStream, task::JoinHandle, time::timeout};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum VideoClientError {
    #[error("video proxy socket path must be absolute")]
    RelativeSocketPath,
    #[error("video proxy request timeout must be greater than zero")]
    InvalidTimeout,
    #[error("video proxy request timed out")]
    Timeout,
    #[error("video proxy I/O failed")]
    Io(#[from] std::io::Error),
    #[error("video proxy HTTP request failed")]
    Http(#[from] http::Error),
    #[error("video proxy HTTP protocol failed")]
    Hyper(#[from] hyper::Error),
    #[error("video proxy returned an invalid status")]
    Status,
    #[error("video proxy returned an invalid content type")]
    ContentType,
}

pub struct MjpegStream {
    pub content_type: HeaderValue,
    pub body: VideoBody,
}

pub struct VideoBody {
    inner: Pin<Box<Incoming>>,
    connection_task: JoinHandle<()>,
}

impl VideoBody {
    fn new(inner: Incoming, connection_task: JoinHandle<()>) -> Self {
        Self {
            inner: Box::pin(inner),
            connection_task,
        }
    }
}

impl HttpBody for VideoBody {
    type Data = Bytes;
    type Error = hyper::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.get_mut().inner.as_mut().poll_frame(context)
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}

impl Drop for VideoBody {
    fn drop(&mut self) {
        self.connection_task.abort();
    }
}

#[derive(Debug, Clone)]
pub struct VideoReadClient {
    socket_path: PathBuf,
    request_timeout: Duration,
}

impl VideoReadClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, VideoClientError> {
        Self::with_timeout(socket_path, DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(
        socket_path: impl Into<PathBuf>,
        request_timeout: Duration,
    ) -> Result<Self, VideoClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(VideoClientError::RelativeSocketPath);
        }
        if request_timeout.is_zero() {
            return Err(VideoClientError::InvalidTimeout);
        }
        Ok(Self {
            socket_path,
            request_timeout,
        })
    }

    pub async fn mjpeg(&self) -> Result<MjpegStream, VideoClientError> {
        timeout(self.request_timeout, self.mjpeg_inner())
            .await
            .map_err(|_| VideoClientError::Timeout)?
    }

    async fn mjpeg_inner(&self) -> Result<MjpegStream, VideoClientError> {
        let stream = UnixStream::connect(&self.socket_path).await?;
        let (mut sender, connection) = http1::handshake(TokioIo::new(stream)).await?;
        let connection_task = tokio::spawn(async move {
            let _ = connection.await;
        });
        let request = Request::builder()
            .method(Method::GET)
            .uri("/mjpeg")
            .header(header::HOST, "localhost")
            .header(header::ACCEPT, "multipart/x-mixed-replace")
            .header(header::USER_AGENT, "Hardened-BliKVM-server/0.1")
            .body(Empty::<Bytes>::new())?;
        let response = sender.send_request(request).await?;
        drop(sender);
        if response.status() != StatusCode::OK {
            connection_task.abort();
            return Err(VideoClientError::Status);
        }
        let content_type = response
            .headers()
            .get(header::CONTENT_TYPE)
            .filter(|value| valid_mjpeg_content_type(value))
            .cloned()
            .ok_or_else(|| {
                connection_task.abort();
                VideoClientError::ContentType
            })?;
        Ok(MjpegStream {
            content_type,
            body: VideoBody::new(response.into_body(), connection_task),
        })
    }
}

fn valid_mjpeg_content_type(value: &HeaderValue) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    let mut parts = value.split(';');
    if !parts.next().is_some_and(|mime| {
        mime.trim()
            .eq_ignore_ascii_case("multipart/x-mixed-replace")
    }) {
        return false;
    }
    parts.any(|parameter| {
        let Some((name, value)) = parameter.split_once('=') else {
            return false;
        };
        name.trim().eq_ignore_ascii_case("boundary") && !value.trim().trim_matches('"').is_empty()
    })
}

#[cfg(test)]
mod tests {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::*;

    #[tokio::test]
    async fn receives_allowlisted_multipart_stream() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("video.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
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
            let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
            assert!(request.starts_with("get /mjpeg http/1.1\r\n"));
            assert!(request.contains("host: localhost\r\n"));
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nContent-Length: 4\r\n\r\nJPEG")
                .await
                .unwrap();
        });
        let stream = VideoReadClient::new(&socket)
            .unwrap()
            .mjpeg()
            .await
            .unwrap();
        assert_eq!(
            stream.content_type,
            "multipart/x-mixed-replace; boundary=frame"
        );
        let bytes = http_body_util::BodyExt::collect(stream.body)
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(bytes, "JPEG");
        server.await.unwrap();
    }

    #[test]
    fn rejects_relative_paths_and_zero_timeout() {
        assert!(matches!(
            VideoReadClient::new("video.sock"),
            Err(VideoClientError::RelativeSocketPath)
        ));
        assert!(matches!(
            VideoReadClient::with_timeout("/run/video.sock", Duration::ZERO),
            Err(VideoClientError::InvalidTimeout)
        ));
    }

    #[test]
    fn content_type_requires_exact_mime_and_a_nonempty_boundary() {
        for valid in [
            "multipart/x-mixed-replace; boundary=frame",
            "Multipart/X-Mixed-Replace;boundary=\"frame\"",
        ] {
            assert!(valid_mjpeg_content_type(&HeaderValue::from_static(valid)));
        }
        for invalid in [
            "multipart/x-mixed-replacedanger; boundary=frame",
            "multipart/x-mixed-replace",
            "multipart/x-mixed-replace; boundary=\"\"",
            "image/jpeg; boundary=frame",
        ] {
            assert!(!valid_mjpeg_content_type(&HeaderValue::from_static(
                invalid
            )));
        }
    }
}
