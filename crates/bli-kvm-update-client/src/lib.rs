use std::{
    path::{Component, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bli_kvm_update_protocol::{
    ApplicationUpdateStatus, MAX_UPDATE_MESSAGE_BYTES, UPDATE_SCHEMA_VERSION, UpdateProtocolError,
    UpdateReadErrorCode, UpdateReadRequest, UpdateReadResponse, UpdateRequestEnvelope,
    decode_response, encode_frame, encode_request,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    sync::Semaphore,
    time::timeout,
};

mod upload;
pub use upload::{UpdateUploadClient, UploadClientError};
mod activation;
pub use activation::{ActivationClientError, UpdateActivationClient};

#[derive(Debug, Error)]
pub enum UpdateClientError {
    #[error("update status socket path is invalid")]
    InvalidSocketPath,
    #[error("update status timeout is invalid")]
    InvalidTimeout,
    #[error("too many concurrent update status requests")]
    Busy,
    #[error("update status request timed out")]
    Timeout,
    #[error("update status I/O failed")]
    Io(#[from] std::io::Error),
    #[error("update status protocol failed")]
    Protocol(#[from] UpdateProtocolError),
    #[error("update status response did not match the request")]
    MismatchedResponse,
    #[error("update status worker rejected the request: {0:?}")]
    Rejected(UpdateReadErrorCode),
}

#[derive(Debug)]
pub struct UpdateStatusClient {
    socket_path: PathBuf,
    request_timeout: Duration,
    next_request_id: AtomicU64,
    concurrency: Semaphore,
}

impl UpdateStatusClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, UpdateClientError> {
        Self::with_timeout(socket_path, Duration::from_secs(5))
    }

    pub fn with_timeout(
        socket_path: impl Into<PathBuf>,
        request_timeout: Duration,
    ) -> Result<Self, UpdateClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute()
            || socket_path.file_name().is_none()
            || socket_path.as_os_str().as_encoded_bytes().len() >= 108
            || socket_path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return Err(UpdateClientError::InvalidSocketPath);
        }
        if request_timeout.is_zero() || request_timeout > Duration::from_secs(10) {
            return Err(UpdateClientError::InvalidTimeout);
        }
        Ok(Self {
            socket_path,
            request_timeout,
            next_request_id: AtomicU64::new(1),
            concurrency: Semaphore::new(4),
        })
    }

    pub async fn status(&self) -> Result<ApplicationUpdateStatus, UpdateClientError> {
        let _permit = self
            .concurrency
            .try_acquire()
            .map_err(|_| UpdateClientError::Busy)?;
        timeout(self.request_timeout, self.execute())
            .await
            .map_err(|_| UpdateClientError::Timeout)?
    }

    async fn execute(&self) -> Result<ApplicationUpdateStatus, UpdateClientError> {
        let id = self
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                Some(value.checked_add(1).unwrap_or(1))
            })
            .unwrap_or(1);
        let request = UpdateRequestEnvelope {
            schema_version: UPDATE_SCHEMA_VERSION,
            request_id: id,
            request: UpdateReadRequest::Status {},
        };
        let frame = encode_frame(&encode_request(&request)?)?;
        let mut stream = UnixStream::connect(&self.socket_path).await?;
        stream.write_all(&frame).await?;
        stream.shutdown().await?;
        let length = stream.read_u32().await? as usize;
        if length == 0 || length > MAX_UPDATE_MESSAGE_BYTES {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let mut payload = vec![0; length];
        stream.read_exact(&mut payload).await?;
        let mut extra = [0_u8; 1];
        if stream.read(&mut extra).await? != 0 {
            return Err(UpdateProtocolError::InvalidLength.into());
        }
        let response = decode_response(&payload)?;
        if response.request_id != id {
            return Err(UpdateClientError::MismatchedResponse);
        }
        match response.response {
            UpdateReadResponse::Status { status } => Ok(*status),
            UpdateReadResponse::Error { code } => Err(UpdateClientError::Rejected(code)),
        }
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_update_protocol::{
        ApplicationSlotVersion, UpdateResponseEnvelope, decode_request, encode_response,
    };
    use tokio::net::UnixListener;

    use super::*;

    fn status_value() -> ApplicationUpdateStatus {
        let slot = ApplicationSlotVersion {
            slot_id: format!("app-00000000000000000002-{}", "a".repeat(64)),
            version: "0.1.0-dev2".into(),
            sequence: 2,
            minimum_system_sequence: 1,
        };
        ApplicationUpdateStatus {
            generation: 10,
            floor_sequence: 2,
            system_sequence: 1,
            selected: slot.clone(),
            confirmed: slot,
            previous: None,
            pending: None,
            last_rollback: None,
        }
    }

    #[tokio::test]
    async fn exchanges_exact_correlated_status_then_eof() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("status.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let length = stream.read_u32().await.unwrap() as usize;
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).await.unwrap();
            let request = decode_request(&bytes).unwrap();
            let response = UpdateResponseEnvelope {
                schema_version: 1,
                request_id: request.request_id,
                response: UpdateReadResponse::Status {
                    status: Box::new(status_value()),
                },
            };
            stream
                .write_all(&encode_frame(&encode_response(&response).unwrap()).unwrap())
                .await
                .unwrap();
        });
        assert_eq!(
            UpdateStatusClient::new(path)
                .unwrap()
                .status()
                .await
                .unwrap(),
            status_value()
        );
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_response_before_body_allocation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("status.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            stream.read_to_end(&mut request).await.unwrap();
            stream.write_u32(u32::MAX).await.unwrap();
        });
        assert!(matches!(
            UpdateStatusClient::new(path).unwrap().status().await,
            Err(UpdateClientError::Protocol(
                UpdateProtocolError::InvalidLength
            ))
        ));
        task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_mismatched_ids_and_trailing_frames() {
        for trailing in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("status.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let task = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                stream.read_to_end(&mut input).await.unwrap();
                let request =
                    decode_request(bli_kvm_update_protocol::decode_frame(&input).unwrap()).unwrap();
                let response = UpdateResponseEnvelope {
                    schema_version: 1,
                    request_id: request.request_id + u64::from(!trailing),
                    response: UpdateReadResponse::Status {
                        status: Box::new(status_value()),
                    },
                };
                let mut frame = encode_frame(&encode_response(&response).unwrap()).unwrap();
                if trailing {
                    frame.push(0);
                }
                stream.write_all(&frame).await.unwrap();
            });
            let result = UpdateStatusClient::new(path).unwrap().status().await;
            if trailing {
                assert!(matches!(
                    result,
                    Err(UpdateClientError::Protocol(
                        UpdateProtocolError::InvalidLength
                    ))
                ));
            } else {
                assert!(matches!(result, Err(UpdateClientError::MismatchedResponse)));
            }
            task.await.unwrap();
        }
    }

    #[tokio::test]
    async fn times_out_a_silent_peer() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("status.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let task = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        assert!(matches!(
            UpdateStatusClient::with_timeout(path, Duration::from_millis(20))
                .unwrap()
                .status()
                .await,
            Err(UpdateClientError::Timeout)
        ));
        task.await.unwrap();
    }

    #[test]
    fn bounds_socket_paths_and_timeouts() {
        for path in ["relative.sock", "/", "/tmp/../status.sock"] {
            assert!(UpdateStatusClient::new(path).is_err());
        }
        assert!(UpdateStatusClient::with_timeout("/run/status.sock", Duration::ZERO).is_err());
        assert!(
            UpdateStatusClient::with_timeout("/run/status.sock", Duration::from_secs(11)).is_err()
        );
    }
}
