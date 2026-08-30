use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bli_kvm_control_protocol::{
    CONTROL_SCHEMA_VERSION, ControlProtocolError, ControlRequestEnvelope, ControlResponse,
    MAX_CONTROL_MESSAGE_BYTES, decode_response, encode_frame, encode_request,
};
use bli_kvm_core::PlatformControlRequest;
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::timeout,
};

#[derive(Debug, Error)]
pub enum ControlClientError {
    #[error("control broker Unix socket path must be absolute")]
    RelativeSocketPath,
    #[error("control broker request timeout must be greater than zero")]
    InvalidTimeout,
    #[error("control broker request timed out")]
    Timeout,
    #[error("control broker I/O failed")]
    Io(#[from] std::io::Error),
    #[error("control broker protocol failed")]
    Protocol(#[from] ControlProtocolError),
    #[error("control broker response did not match the request")]
    MismatchedResponse,
}

#[derive(Debug)]
pub struct ControlClient {
    socket_path: PathBuf,
    request_timeout: Duration,
    next_request_id: AtomicU64,
}

impl ControlClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, ControlClientError> {
        Self::with_timeout(socket_path, Duration::from_secs(5))
    }

    pub fn with_timeout(
        socket_path: impl Into<PathBuf>,
        request_timeout: Duration,
    ) -> Result<Self, ControlClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(ControlClientError::RelativeSocketPath);
        }
        if request_timeout.is_zero() {
            return Err(ControlClientError::InvalidTimeout);
        }
        Ok(Self {
            socket_path,
            request_timeout,
            next_request_id: AtomicU64::new(1),
        })
    }

    pub async fn execute(
        &self,
        operation_id: impl Into<String>,
        request: PlatformControlRequest,
    ) -> Result<ControlResponse, ControlClientError> {
        timeout(
            self.request_timeout,
            self.execute_inner(operation_id.into(), request),
        )
        .await
        .map_err(|_| ControlClientError::Timeout)?
    }

    async fn execute_inner(
        &self,
        operation_id: String,
        request: PlatformControlRequest,
    ) -> Result<ControlResponse, ControlClientError> {
        let request_id = self
            .next_request_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                Some(current.checked_add(1).unwrap_or(1))
            })
            .expect("request ID update closure is infallible");
        let envelope = ControlRequestEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id,
            operation_id: operation_id.clone(),
            request,
        };
        let payload = encode_request(&envelope)?;
        let mut stream = UnixStream::connect(&self.socket_path).await?;
        stream.write_all(&encode_frame(&payload)?).await?;
        stream.shutdown().await?;

        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > MAX_CONTROL_MESSAGE_BYTES {
            return Err(ControlProtocolError::MessageTooLarge.into());
        }
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload).await?;
        let response = decode_response(&payload)?;
        if response.request_id != request_id || response.operation_id != operation_id {
            return Err(ControlClientError::MismatchedResponse);
        }
        Ok(response.response)
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_control_protocol::{
        ControlRejectionCode, ControlResponseEnvelope, encode_response,
    };
    use bli_kvm_core::{AtxControlAction, PlatformControlRequest};
    use tokio::net::UnixListener;

    use super::*;

    #[tokio::test]
    async fn correlates_one_typed_denial_response() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut payload = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut payload).await.unwrap();
            let request = bli_kvm_control_protocol::decode_request(&payload).unwrap();
            let response = ControlResponseEnvelope {
                schema_version: CONTROL_SCHEMA_VERSION,
                request_id: request.request_id,
                operation_id: request.operation_id,
                response: ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled,
                },
            };
            let payload = encode_response(&response).unwrap();
            stream
                .write_all(&encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });

        let response = ControlClient::new(&socket)
            .unwrap()
            .execute(
                "0123456789abcdef0123456789abcdef",
                PlatformControlRequest::Atx {
                    action: AtxControlAction::PowerShort,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );
        server.await.unwrap();
    }

    #[test]
    fn rejects_relative_paths_and_zero_timeouts() {
        assert!(matches!(
            ControlClient::new("control.sock"),
            Err(ControlClientError::RelativeSocketPath)
        ));
        assert!(matches!(
            ControlClient::with_timeout("/run/control.sock", Duration::ZERO),
            Err(ControlClientError::InvalidTimeout)
        ));
    }
}
