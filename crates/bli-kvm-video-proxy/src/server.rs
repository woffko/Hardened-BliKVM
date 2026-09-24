use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    sync::Arc,
};

use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    task::JoinSet,
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
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    serve_until(config, async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
    })
    .await
}

async fn serve_until(
    config: VideoProxyConfig,
    stopping: impl std::future::Future<Output = ()>,
) -> Result<(), VideoProxyError> {
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.proxy_socket).is_ok() {
        return Err(VideoProxyError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.proxy_socket)?;
    let cleanup = SocketCleanup::new(config.proxy_socket.clone())?;
    fs::set_permissions(&config.proxy_socket, Permissions::from_mode(0o660))?;
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    tokio::pin!(stopping);
    let mut connections = JoinSet::new();
    let serving = loop {
        let permit = tokio::select! {
            biased;
            _ = &mut stopping => break Ok(()),
            result = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = result {
                    break Err(io::Error::other(format!("video proxy connection task failed: {error}")));
                }
                continue;
            }
            permit = semaphore.clone().acquire_owned() => match permit {
                Ok(permit) => permit,
                Err(_) => break Err(io::Error::other("video proxy semaphore closed")),
            },
        };
        let accepted = tokio::select! {
            biased;
            _ = &mut stopping => break Ok(()),
            accepted = listener.accept() => accepted,
        };
        let (stream, _) = match accepted {
            Ok(accepted) => accepted,
            Err(error) => break Err(error),
        };
        let config = config.clone();
        connections.spawn(async move {
            let _permit = permit;
            handle_connection(stream, &config).await
        });
    };
    drop(listener);
    // Proxy tasks are read-only: cancellation closes both socket halves and
    // upstream capture clients. Join before claiming a clean owner transition.
    connections.abort_all();
    let drained = timeout(std::time::Duration::from_secs(2), async {
        let mut failure = None;
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result
                && !error.is_cancelled()
            {
                failure = Some(io::Error::other(format!(
                    "video proxy drain failed: {error}"
                )));
            }
        }
        failure.map_or(Ok(()), Err)
    })
    .await
    .map_err(|_| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            "video proxy drain deadline expired",
        )
    })?;
    let cleaned = cleanup.finish();
    serving?;
    drained?;
    cleaned?;
    Ok(())
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

struct SocketCleanup {
    path: std::path::PathBuf,
    device: u64,
    inode: u64,
    armed: bool,
}

impl SocketCleanup {
    fn new(path: std::path::PathBuf) -> io::Result<Self> {
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_socket() {
            return Err(io::Error::other("video proxy socket identity unavailable"));
        }
        Ok(Self {
            path,
            device: metadata.dev(),
            inode: metadata.ino(),
            armed: true,
        })
    }

    fn remove_owned(&self) -> io::Result<()> {
        let metadata = fs::symlink_metadata(&self.path)?;
        if !metadata.file_type().is_socket()
            || metadata.dev() != self.device
            || metadata.ino() != self.inode
        {
            return Err(io::Error::other("video proxy socket identity changed"));
        }
        // The reviewed runtime directory excludes untrusted writers. This
        // check protects accidental replacement, not a privileged rename race.
        fs::remove_file(&self.path)
    }

    fn finish(mut self) -> io::Result<()> {
        self.armed = false;
        self.remove_owned()
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.remove_owned()
        {
            // Early-error/abort fallback only; normal shutdown checks finish.
            eprintln!("video proxy socket cleanup failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::{Duration, sleep};

    use super::*;

    #[tokio::test]
    async fn socket_cleanup_requires_its_original_inode_and_reports_failure() {
        for replacement in ["file", "socket", "symlink", "missing"] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("proxy.sock");
            let _listener = UnixListener::bind(&path).unwrap();
            let guard = SocketCleanup::new(path.clone()).unwrap();
            // Retain the old inode so reuse cannot make the fixture ambiguous.
            fs::rename(&path, directory.path().join("original.sock")).unwrap();
            let replacement_socket = match replacement {
                "file" => {
                    fs::write(&path, b"preserve").unwrap();
                    None
                }
                "socket" => Some(UnixListener::bind(&path).unwrap()),
                "symlink" => {
                    std::os::unix::fs::symlink(directory.path().join("original.sock"), &path)
                        .unwrap();
                    None
                }
                _ => None,
            };
            assert!(
                guard.finish().is_err(),
                "{replacement} replacement reported clean"
            );
            if replacement != "missing" {
                assert!(fs::symlink_metadata(&path).is_ok());
            }
            assert!(directory.path().join("original.sock").exists());
            drop(replacement_socket);
        }
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("proxy.sock");
        let _listener = UnixListener::bind(&path).unwrap();
        SocketCleanup::new(path.clone()).unwrap().finish().unwrap();
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn shutdown_joins_silent_stream_and_waiting_admission() {
        let directory = tempfile::tempdir().unwrap();
        let upstream_path = directory.path().join("upstream.sock");
        let proxy_path = directory.path().join("proxy.sock");
        let upstream_listener = UnixListener::bind(&upstream_path).unwrap();
        let mut config = VideoProxyConfig::new(&proxy_path, &upstream_path).unwrap();
        config.max_connections = 1;
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let proxy = tokio::spawn(serve_until(config, async {
            let _ = stopping.await;
        }));
        for _ in 0..50 {
            if proxy_path.exists() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        let mut client = UnixStream::connect(&proxy_path).await.unwrap();
        client
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let (mut upstream, _) = timeout(Duration::from_secs(1), upstream_listener.accept())
            .await
            .unwrap()
            .unwrap();
        let mut request = vec![0; UPSTREAM_REQUEST.len()];
        upstream.read_exact(&mut request).await.unwrap();
        assert_eq!(request, UPSTREAM_REQUEST);
        // The active stream owns the sole permit. Stop must interrupt permit
        // acquisition instead of waiting for another upstream byte or client.
        stop.send(()).unwrap();
        timeout(Duration::from_secs(3), proxy)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(!proxy_path.exists());
        let mut byte = [0_u8; 1];
        assert_eq!(
            timeout(Duration::from_secs(1), upstream.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        assert_eq!(
            timeout(Duration::from_secs(1), client.read(&mut byte))
                .await
                .unwrap()
                .unwrap(),
            0
        );
    }

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

    #[tokio::test]
    async fn two_clients_stream_independently() {
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
            let (mut first, _) = upstream.accept().await.unwrap();
            let mut request = vec![0_u8; UPSTREAM_REQUEST.len()];
            first.read_exact(&mut request).await.unwrap();
            assert_eq!(request, UPSTREAM_REQUEST);
            let (mut second, _) = upstream.accept().await.unwrap();
            let mut request = vec![0_u8; UPSTREAM_REQUEST.len()];
            second.read_exact(&mut request).await.unwrap();
            assert_eq!(request, UPSTREAM_REQUEST);
            first
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nContent-Length: 4\r\n\r\nAAAA")
                .await
                .unwrap();
            second
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nContent-Length: 4\r\n\r\nBBBB")
                .await
                .unwrap();
        });

        let mut client_a = UnixStream::connect(&proxy_path).await.unwrap();
        client_a
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut client_b = UnixStream::connect(&proxy_path).await.unwrap();
        client_b
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();

        let mut response_a = Vec::new();
        client_a.read_to_end(&mut response_a).await.unwrap();
        let mut response_b = Vec::new();
        client_b.read_to_end(&mut response_b).await.unwrap();
        upstream_task.await.unwrap();
        assert!(response_a.ends_with(b"AAAA"));
        assert!(!response_a.windows(4).any(|window| window == b"BBBB"));
        assert!(response_b.ends_with(b"BBBB"));
        assert!(!response_b.windows(4).any(|window| window == b"AAAA"));
        proxy.abort();
        let _ = proxy.await;
    }

    #[tokio::test]
    async fn upstream_disconnect_ends_downstream_cleanly() {
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
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\nContent-Length: 4\r\n\r\nJPEG",
                )
                .await
                .unwrap();
            // Mid-stream upstream disconnect.
            drop(stream);
        });

        let mut client = UnixStream::connect(&proxy_path).await.unwrap();
        client
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut response = Vec::new();
        client.read_to_end(&mut response).await.unwrap();
        upstream_task.await.unwrap();
        assert!(response.ends_with(b"JPEG"));
        proxy.abort();
        let _ = proxy.await;
    }

    #[tokio::test]
    async fn forwards_the_first_frame_without_buffering_the_stream() {
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
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: multipart/x-mixed-replace; boundary=frame\r\n\r\n--frame\r\n\r\nJPEG",
                )
                .await
                .unwrap();
            // Hold the stream open so a buffering proxy would block.
            sleep(Duration::from_millis(400)).await;
            let _ = stream.write_all(b"\r\n--frame\r\n\r\nLATE").await;
        });

        let mut client = UnixStream::connect(&proxy_path).await.unwrap();
        client
            .write_all(b"GET /mjpeg HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        let mut buffer = [0_u8; 512];
        let read = tokio::time::timeout(Duration::from_millis(200), client.read(&mut buffer))
            .await
            .expect("proxy buffered the first frame instead of streaming it")
            .unwrap();
        assert!(read > 0);
        assert!(buffer[..read].windows(4).any(|window| window == b"JPEG"));
        drop(client);
        upstream_task.await.unwrap();
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
