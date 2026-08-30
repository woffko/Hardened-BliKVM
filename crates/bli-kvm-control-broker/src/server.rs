use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, PermissionsExt},
    sync::Arc,
};

use bli_kvm_control_protocol::{
    CONTROL_SCHEMA_VERSION, ControlRejectionCode, ControlRequestEnvelope, ControlResponse,
    ControlResponseEnvelope, MAX_CONTROL_MESSAGE_BYTES, decode_request, encode_frame,
    encode_response,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    time::timeout,
};

use crate::{ControlBrokerConfig, ControlBrokerConfigError};

#[derive(Debug, Error)]
pub enum ControlBrokerError {
    #[error("control broker configuration is invalid")]
    Config(#[from] ControlBrokerConfigError),
    #[error("control broker runtime directory is invalid")]
    InvalidRuntimeDirectory,
    #[error("control broker socket already exists")]
    SocketAlreadyExists,
    #[error("control broker I/O failed")]
    Io(#[from] io::Error),
}

pub async fn run(config: ControlBrokerConfig) -> Result<(), ControlBrokerError> {
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.socket_path).is_ok() {
        return Err(ControlBrokerError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.socket_path)?;
    let _cleanup = SocketCleanup(config.socket_path.clone());
    fs::set_permissions(&config.socket_path, Permissions::from_mode(0o660))?;
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("control broker semaphore closed"))?;
        let request_timeout = config.request_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, request_timeout).await;
        });
    }
}

fn validate_runtime_directory(config: &ControlBrokerConfig) -> Result<(), ControlBrokerError> {
    let parent = config
        .socket_path
        .parent()
        .ok_or(ControlBrokerError::InvalidRuntimeDirectory)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(ControlBrokerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

async fn handle_connection(
    mut stream: UnixStream,
    request_timeout: std::time::Duration,
) -> Result<(), io::Error> {
    let request = timeout(request_timeout, read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control request timed out"))??;
    let response = deny_request(request);
    let payload = encode_response(&response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control response invalid"))?;
    timeout(
        request_timeout,
        stream.write_all(&encode_frame(&payload).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "control response too large")
        })?),
    )
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "control response timed out"))??;
    Ok(())
}

async fn read_request(stream: &mut UnixStream) -> Result<ControlRequestEnvelope, io::Error> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_CONTROL_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control request too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    decode_request(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "control request invalid"))
}

fn deny_request(request: ControlRequestEnvelope) -> ControlResponseEnvelope {
    ControlResponseEnvelope {
        schema_version: CONTROL_SCHEMA_VERSION,
        request_id: request.request_id,
        operation_id: request.operation_id,
        response: ControlResponse::Rejected {
            code: ControlRejectionCode::CapabilityDisabled,
        },
    }
}

struct SocketCleanup(std::path::PathBuf);

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use bli_kvm_control_client::ControlClient;
    use bli_kvm_core::{AtxControlAction, HidReport, PlatformControlRequest, VirtualMediaCommand};
    use tokio::time::sleep;

    use super::*;

    #[tokio::test]
    async fn all_valid_control_families_are_denied_without_a_backend() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("control.sock");
        let broker = tokio::spawn(run(ControlBrokerConfig::new(&socket).unwrap()));
        for _ in 0..100 {
            if socket.exists() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        let client = ControlClient::new(&socket).unwrap();
        let requests = [
            PlatformControlRequest::Hid {
                report: HidReport::Keyboard {
                    data: [0, 0, 4, 0, 0, 0, 0, 0],
                },
            },
            PlatformControlRequest::Atx {
                action: AtxControlAction::PowerShort,
            },
            PlatformControlRequest::VirtualMedia {
                command: VirtualMediaCommand::SetConnected { connected: false },
            },
        ];
        for (index, request) in requests.into_iter().enumerate() {
            let operation_id = format!("{value:032x}", value = index + 1);
            assert_eq!(
                client.execute(operation_id, request).await.unwrap(),
                ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled
                }
            );
        }
        broker.abort();
        let _ = broker.await;
    }

    #[test]
    fn rejects_group_writable_runtime_directories() {
        let directory = tempfile::tempdir().unwrap();
        let config = ControlBrokerConfig::new(directory.path().join("control.sock")).unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            validate_runtime_directory(&config),
            Err(ControlBrokerError::InvalidRuntimeDirectory)
        ));
    }
}
