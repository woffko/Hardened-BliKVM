//! Stage-only streaming client. Dropping the future closes IPC; a lost receipt
//! after publication is ambiguous and callers must retry the same archive.
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bli_kvm_update_protocol::{
    MAX_UPDATE_MESSAGE_BYTES, UPDATE_SCHEMA_VERSION, UpdateProtocolError, encode_frame,
    upload::{
        self, UploadErrorCode, UploadReceipt, UploadRequest, UploadRequestEnvelope, UploadResponse,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Semaphore,
    time::timeout,
};

#[derive(Debug, thiserror::Error)]
pub enum UploadClientError {
    #[error("invalid upload configuration")]
    Configuration,
    #[error("upload busy")]
    Busy,
    #[error("upload timed out; publication may require reconciliation")]
    Timeout,
    #[error("upload I/O failed; publication may require reconciliation")]
    Io(#[from] std::io::Error),
    #[error("invalid upload protocol")]
    Protocol(#[from] UpdateProtocolError),
    #[error("upload length mismatch")]
    LengthMismatch,
    #[error("upload response mismatch")]
    MismatchedResponse,
    #[error("upload rejected: {0:?}")]
    Rejected(UploadErrorCode),
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_update_protocol::{
        ApplicationSlotVersion,
        upload::{UploadDisposition, UploadResponseEnvelope},
    };
    use tokio::net::UnixListener;

    async fn request(socket: &mut UnixStream) -> UploadRequestEnvelope {
        let size = socket.read_u32().await.unwrap() as usize;
        let mut bytes = vec![0; size];
        socket.read_exact(&mut bytes).await.unwrap();
        upload::decode_request(&bytes).unwrap()
    }

    fn receipt() -> UploadReceipt {
        UploadReceipt {
            slot: ApplicationSlotVersion {
                slot_id: format!("app-{:020}-{}", u64::MAX, "a".repeat(64)),
                version: "1.0.0".into(),
                sequence: u64::MAX,
                minimum_system_sequence: 1,
            },
            disposition: UploadDisposition::AlreadyPresent,
            manifest_sha256: "a".repeat(64),
            payload_bytes: 1,
        }
    }

    #[tokio::test]
    async fn exact_body_and_correlated_validated_receipt() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("upload.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let worker = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let metadata = request(&mut socket).await;
            assert_eq!(metadata.request, UploadRequest::Stage { archive_bytes: 3 });
            let mut body = Vec::new();
            socket.read_to_end(&mut body).await.unwrap();
            assert_eq!(body, b"abc");
            let response = UploadResponseEnvelope {
                schema_version: 1,
                request_id: metadata.request_id,
                response: UploadResponse::Staged { receipt: receipt() },
            };
            socket
                .write_all(&encode_frame(&upload::encode_response(&response).unwrap()).unwrap())
                .await
                .unwrap();
        });
        assert_eq!(
            UpdateUploadClient::new(path)
                .unwrap()
                .stage(3, &b"abc"[..])
                .await
                .unwrap(),
            receipt()
        );
        worker.await.unwrap();
    }

    #[tokio::test]
    async fn invalid_body_never_delivers_all_declared_bytes() {
        for body in [&b"ab"[..], &b"abcd"[..]] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("upload.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let worker = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                request(&mut socket).await;
                let mut bytes = Vec::new();
                socket.read_to_end(&mut bytes).await.unwrap();
                assert!(
                    bytes.len() < 3,
                    "disconnect must not impersonate complete body EOF"
                );
            });
            assert!(matches!(
                UpdateUploadClient::new(path).unwrap().stage(3, body).await,
                Err(UploadClientError::LengthMismatch)
            ));
            worker.await.unwrap();
        }
    }

    #[tokio::test]
    async fn invalid_length_does_not_read_or_connect() {
        struct NeverRead;
        impl AsyncRead for NeverRead {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                panic!("invalid declaration consumed body")
            }
        }
        for size in [0, upload::MAX_UPLOAD_ARCHIVE_BYTES + 1] {
            assert!(matches!(
                UpdateUploadClient::new("/nonexistent/upload.sock")
                    .unwrap()
                    .stage(size, NeverRead)
                    .await,
                Err(UploadClientError::Protocol(
                    UpdateProtocolError::InvalidLength
                ))
            ));
        }
    }

    #[tokio::test]
    async fn single_inflight_upload_and_cancellation_release_admission() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("upload.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let client = std::sync::Arc::new(UpdateUploadClient::new(path).unwrap());
        let (_writer, reader) = tokio::io::duplex(16);
        let first = tokio::spawn({
            let client = client.clone();
            async move { client.stage(3, reader).await }
        });
        let (mut socket, _) = listener.accept().await.unwrap();
        request(&mut socket).await;
        assert!(matches!(
            client.stage(1, &b"x"[..]).await,
            Err(UploadClientError::Busy)
        ));
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        let mut bytes = Vec::new();
        socket.read_to_end(&mut bytes).await.unwrap();
        assert!(bytes.is_empty());
        // Reaching declaration validation proves the permit was released.
        assert!(matches!(
            client.stage(0, &b""[..]).await,
            Err(UploadClientError::Protocol(_))
        ));
    }

    #[tokio::test]
    async fn rejects_oversized_mismatched_and_surplus_responses() {
        for mode in ["oversized", "mismatch", "surplus"] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("upload.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let worker = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let metadata = request(&mut socket).await;
                socket.read_to_end(&mut Vec::new()).await.unwrap();
                if mode == "oversized" {
                    socket.write_u32(u32::MAX).await.unwrap();
                    return;
                }
                let response = UploadResponseEnvelope {
                    schema_version: 1,
                    request_id: metadata.request_id + u64::from(mode == "mismatch"),
                    response: UploadResponse::Staged { receipt: receipt() },
                };
                let mut bytes = encode_frame(&upload::encode_response(&response).unwrap()).unwrap();
                if mode == "surplus" {
                    bytes.push(0);
                }
                socket.write_all(&bytes).await.unwrap();
            });
            let error = UpdateUploadClient::new(path)
                .unwrap()
                .stage(1, &b"x"[..])
                .await
                .unwrap_err();
            if mode == "mismatch" {
                assert!(matches!(error, UploadClientError::MismatchedResponse));
            } else {
                assert!(matches!(
                    error,
                    UploadClientError::Protocol(UpdateProtocolError::InvalidLength)
                ));
            }
            worker.await.unwrap();
        }
    }
}

#[derive(Debug)]
pub struct UpdateUploadClient {
    path: PathBuf,
    next_id: AtomicU64,
    concurrency: Semaphore,
}

impl UpdateUploadClient {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, UploadClientError> {
        let path = path.into();
        // Share the existing bounded, absolute Unix-path policy.
        super::UpdateStatusClient::new(path.clone())
            .map_err(|_| UploadClientError::Configuration)?;
        Ok(Self {
            path,
            next_id: AtomicU64::new(1),
            concurrency: Semaphore::new(1),
        })
    }

    pub async fn stage<R: AsyncRead + Unpin>(
        &self,
        archive_bytes: u64,
        reader: R,
    ) -> Result<UploadReceipt, UploadClientError> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| UploadClientError::Busy)?;
        // Independent hard total bound: progress does not extend it. Includes
        // verification and response time, bounded by the worker's 180 s life.
        timeout(
            Duration::from_secs(175),
            self.execute(archive_bytes, reader),
        )
        .await
        .map_err(|_| UploadClientError::Timeout)?
    }

    async fn execute<R: AsyncRead + Unpin>(
        &self,
        archive_bytes: u64,
        mut reader: R,
    ) -> Result<UploadReceipt, UploadClientError> {
        let id = self
            .next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
                Some(v.checked_add(1).unwrap_or(1))
            })
            .unwrap_or(1);
        let metadata = upload::encode_request(&UploadRequestEnvelope {
            schema_version: UPDATE_SCHEMA_VERSION,
            request_id: id,
            request: UploadRequest::Stage { archive_bytes },
        })?;
        let mut socket = timeout(Duration::from_secs(2), UnixStream::connect(&self.path))
            .await
            .map_err(|_| UploadClientError::Timeout)??;
        timeout(
            Duration::from_secs(2),
            socket.write_all(&encode_frame(&metadata)?),
        )
        .await
        .map_err(|_| UploadClientError::Timeout)??;
        timeout(Duration::from_secs(115), async {
            // Hold the final byte until upstream EOF is validated. Otherwise
            // dropping IPC on a surplus/error would deliver an apparently
            // valid exact-length body plus EOF and could publish it.
            let mut remaining = archive_bytes - 1;
            let mut buffer = [0_u8; 64 * 1024];
            while remaining > 0 {
                let capacity = remaining.min(buffer.len() as u64) as usize;
                let count = timeout(Duration::from_secs(4), reader.read(&mut buffer[..capacity]))
                    .await
                    .map_err(|_| UploadClientError::Timeout)??;
                if count == 0 {
                    return Err(UploadClientError::LengthMismatch);
                }
                timeout(Duration::from_secs(4), socket.write_all(&buffer[..count]))
                    .await
                    .map_err(|_| UploadClientError::Timeout)??;
                remaining -= count as u64;
            }
            let mut last = [0_u8; 1];
            if timeout(Duration::from_secs(4), reader.read(&mut last))
                .await
                .map_err(|_| UploadClientError::Timeout)??
                != 1
            {
                return Err(UploadClientError::LengthMismatch);
            }
            if timeout(Duration::from_secs(4), reader.read(&mut buffer[..1]))
                .await
                .map_err(|_| UploadClientError::Timeout)??
                != 0
            {
                return Err(UploadClientError::LengthMismatch);
            }
            timeout(Duration::from_secs(4), socket.write_all(&last))
                .await
                .map_err(|_| UploadClientError::Timeout)??;
            socket.shutdown().await?;
            Ok::<_, UploadClientError>(())
        })
        .await
        .map_err(|_| UploadClientError::Timeout)??;
        let length = socket.read_u32().await? as usize;
        if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let mut payload = vec![0; length];
        socket.read_exact(&mut payload).await?;
        if socket.read(&mut [0_u8; 1]).await? != 0 {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let response = upload::decode_response(&payload)?;
        if response.request_id != id {
            return Err(UploadClientError::MismatchedResponse);
        }
        match response.response {
            UploadResponse::Staged { receipt } => Ok(receipt),
            UploadResponse::Error { code } => Err(UploadClientError::Rejected(code)),
        }
    }
}
