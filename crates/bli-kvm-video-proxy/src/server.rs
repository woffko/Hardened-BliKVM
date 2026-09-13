use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    sync::Arc,
};

use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    time::timeout,
};

use crate::{VideoProxyConfig, VideoProxyConfigError};

const MAX_REQUEST_BYTES: usize = 4096;
const MAX_HEADERS: usize = 32;
const UPSTREAM_REQUEST: &[u8] = b"GET /stream HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nUser-Agent: Hardened-BliKVM-video-proxy/0.1\r\n\r\n";

#[derive(Debug, Error)]
pub enum VideoProxyError {
    #[error("video proxy configuration is invalid")]
    Config(#[from] VideoProxyConfigError),
    #[error("video proxy runtime directory is invalid")]
    InvalidRuntimeDirectory,
    #[error("video proxy socket already exists")]
    SocketAlreadyExists,
    #[error("video proxy I/O failed")]
    Io(#[from] io::Error),
}

pub async fn run(config: VideoProxyConfig) -> Result<(), VideoProxyError> {
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.proxy_socket).is_ok() {
        return Err(VideoProxyError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.proxy_socket)?;
    let _cleanup = SocketCleanup(config.proxy_socket.clone());
    fs::set_permissions(&config.proxy_socket, Permissions::from_mode(0o660))?;
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("video proxy semaphore closed"))?;
        let config = config.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, &config).await;
        });
    }
}

fn validate_runtime_directory(config: &VideoProxyConfig) -> Result<(), VideoProxyError> {
    let parent = config
        .proxy_socket
        .parent()
        .ok_or(VideoProxyError::InvalidRuntimeDirectory)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.file_type().is_socket() {
        return Err(VideoProxyError::InvalidRuntimeDirectory);
    }
    if metadata.permissions().mode() & 0o022 != 0 {
        return Err(VideoProxyError::InvalidRuntimeDirectory);
    }
    Ok(())
}

async fn handle_connection(
    mut downstream: UnixStream,
    config: &VideoProxyConfig,
) -> Result<(), io::Error> {
    timeout(config.request_timeout, read_fixed_request(&mut downstream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "video request timed out"))??;
    let mut upstream = timeout(
        config.request_timeout,
        UnixStream::connect(&config.upstream_socket),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "uStreamer connect timed out"))??;
    timeout(config.request_timeout, upstream.write_all(UPSTREAM_REQUEST))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "uStreamer request timed out"))??;

    let (mut downstream_read, mut downstream_write) = downstream.into_split();
    let mut downstream_probe = [0_u8; 1];
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = tokio::select! {
            biased;
            downstream = downstream_read.read(&mut downstream_probe) => {
                return match downstream? {
                    0 => Ok(()),
                    _ => Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "video request body is forbidden",
                    )),
                };
            }
            upstream = timeout(config.idle_timeout, upstream.read(&mut buffer)) => {
                upstream.map_err(|_| {
                    io::Error::new(io::ErrorKind::TimedOut, "uStreamer stream idle")
                })??
            }
        };
        if read == 0 {
            return Ok(());
        }
        timeout(
            config.request_timeout,
            downstream_write.write_all(&buffer[..read]),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "video client write timed out"))??;
    }
}

async fn read_fixed_request(stream: &mut UnixStream) -> Result<(), io::Error> {
    let mut request = Vec::with_capacity(1024);
    loop {
        let mut chunk = [0_u8; 1024];
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "video request ended before headers",
            ));
        }
        if request.len().saturating_add(read) > MAX_REQUEST_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "video request headers too large",
            ));
        }
        request.extend_from_slice(&chunk[..read]);
        if let Some(end) = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|position| position + 4)
        {
            if end != request.len() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "video request body is forbidden",
                ));
            }
            break;
        }
    }

    let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut parsed = httparse::Request::new(&mut headers);
    let complete = matches!(
        parsed.parse(&request),
        Ok(httparse::Status::Complete(length)) if length == request.len()
    );
    let mut host_headers = parsed
        .headers
        .iter()
        .filter(|header| header.name.eq_ignore_ascii_case("host"));
    if !complete
        || parsed.method != Some("GET")
        || parsed.path != Some("/mjpeg")
        || parsed.version != Some(1)
        || host_headers.next().map(|header| header.value) != Some(b"localhost")
        || host_headers.next().is_some()
        || parsed.headers.iter().any(|header| {
            header.name.eq_ignore_ascii_case("transfer-encoding")
                || header.name.eq_ignore_ascii_case("content-length")
        })
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "video request is not allowlisted",
        ));
    }
    Ok(())
}

struct SocketCleanup(std::path::PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{Duration, sleep};

    use super::*;

    #[tokio::test]
    async fn proxies_only_the_fixed_mjpeg_route_to_fixed_upstream() {
        let directory = tempfile::tempdir().unwrap();
        let upstream_path = directory.path().join("upstream.sock");
        let proxy_path = directory.path().join("proxy.sock");
        let upstream = UnixListener::bind(&upstream_path).unwrap();
        let config = VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap();
        let proxy = tokio::spawn(run(config));
        for _ in 0..50 {
            if proxy_path.exists() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }

        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = vec![0_u8; UPSTREAM_REQUEST.len()];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(request, UPSTREAM_REQUEST);
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nContent-Length: 4\r\n\r\nJPEG")
                .await
                .unwrap();
        });
        let mut client = UnixStream::connect(&proxy_path).await.unwrap();
        client
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        assert!(response.ends_with(b"JPEG"));
        upstream_task.await.unwrap();
        proxy.abort();
        let _ = proxy.await;
    }

    #[tokio::test]
    async fn downstream_disconnect_cancels_a_silent_upstream() {
        let directory = tempfile::tempdir().unwrap();
        let upstream_path = directory.path().join("upstream.sock");
        let upstream_listener = UnixListener::bind(&upstream_path).unwrap();
        let config =
            VideoProxyConfig::new(directory.path().join("proxy.sock"), &upstream_path).unwrap();
        let (proxy_downstream, mut client_downstream) = UnixStream::pair().unwrap();
        let handler =
            tokio::spawn(async move { handle_connection(proxy_downstream, &config).await });

        client_downstream
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let (mut upstream, _) = upstream_listener.accept().await.unwrap();
        let mut request = vec![0_u8; UPSTREAM_REQUEST.len()];
        upstream.read_exact(&mut request).await.unwrap();
        assert_eq!(request, UPSTREAM_REQUEST);

        drop(client_downstream);
        let result = tokio::time::timeout(Duration::from_millis(500), handler)
            .await
            .expect("handler retained a disconnected downstream permit")
            .unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn rejects_non_allowlisted_request_without_upstream_access() {
        let directory = tempfile::tempdir().unwrap();
        let upstream_path = directory.path().join("upstream.sock");
        let proxy_path = directory.path().join("proxy.sock");
        let upstream = UnixListener::bind(&upstream_path).unwrap();
        let proxy = tokio::spawn(run(
            VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap()
        ));
        for _ in 0..50 {
            if proxy_path.exists() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        for request in [
            b"GET /other HTTP/1.1\r\nHost: localhost\r\n\r\n".as_slice(),
            b"GET /mjpeg HTTP/1.1\r\n\r\n".as_slice(),
            b"GET /mjpeg HTTP/1.1\r\nHost: other\r\n\r\n".as_slice(),
            b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\nHost: localhost\r\n\r\n".as_slice(),
            b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n".as_slice(),
        ] {
            let mut client = UnixStream::connect(&proxy_path).await.unwrap();
            client.write_all(request).await.unwrap();
            let mut response = Vec::new();
            client.read_to_end(&mut response).await.unwrap();
            assert!(response.is_empty());
        }
        assert!(
            timeout(Duration::from_millis(50), upstream.accept())
                .await
                .is_err()
        );
        proxy.abort();
        let _ = proxy.await;
    }

    #[tokio::test]
    async fn sustains_a_bounded_multi_frame_soak() {
        const FRAMES: usize = 2000;
        let directory = tempfile::tempdir().unwrap();
        let upstream_path = directory.path().join("upstream.sock");
        let proxy_path = directory.path().join("proxy.sock");
        let upstream = UnixListener::bind(&upstream_path).unwrap();
        let proxy = tokio::spawn(run(
            VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap()
        ));
        for _ in 0..50 {
            if proxy_path.exists() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }

        let upstream_task = tokio::spawn(async move {
            let (mut stream, _) = upstream.accept().await.unwrap();
            let mut request = vec![0_u8; UPSTREAM_REQUEST.len()];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(request, UPSTREAM_REQUEST);
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n",
                )
                .await
                .unwrap();
            for _ in 0..FRAMES {
                stream
                    .write_all(
                        b"--frame\r\nContent-Type: image/jpeg\r\nContent-Length: 4\r\n\r\nJPEG\r\n",
                    )
                    .await
                    .unwrap();
            }
        });

        let mut client = UnixStream::connect(&proxy_path).await.unwrap();
        client
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        upstream_task.await.unwrap();
        let markers = response
            .windows(b"--frame".len())
            .filter(|window| *window == b"--frame")
            .count();
        assert_eq!(markers, FRAMES);
        assert!(response.ends_with(b"JPEG\r\n"));
        proxy.abort();
        let _ = proxy.await;
    }

    #[test]
    fn rejects_a_group_or_world_writable_runtime_directory() {
        let directory = tempfile::tempdir().unwrap();
        let proxy_path = directory.path().join("proxy.sock");
        let upstream_path = directory.path().join("upstream.sock");
        let config = VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap();

        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            validate_runtime_directory(&config),
            Err(VideoProxyError::InvalidRuntimeDirectory)
        ));

        fs::set_permissions(directory.path(), Permissions::from_mode(0o750)).unwrap();
        assert!(validate_runtime_directory(&config).is_ok());
    }
}
