use std::{
    fs::{self, Permissions},
    io,
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    sync::Arc,
};

use bli_kvm_broker_protocol::{
    BROKER_SCHEMA_VERSION, BrokerErrorCode, BrokerReadRequest, BrokerReadResponse,
    BrokerRequestEnvelope, BrokerResponseEnvelope, MAX_BROKER_MESSAGE_BYTES, decode_request,
    encode_frame, encode_response,
};
use bli_kvm_kvmd_client::{KvmdClientError, KvmdCredentials, KvmdReadClient};
use bli_kvm_platform_bli::{
    BliPlatformObservations, read_h616_external_rtc, read_h616_temperature_millidegrees,
    sanitize_kvmd_inventory_with_observations,
};
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::Semaphore,
    time::timeout,
};

use crate::{BrokerConfig, BrokerConfigError};

#[derive(Debug, Error)]
pub enum BrokerServerError {
    #[error("broker configuration is invalid")]
    Config(#[from] BrokerConfigError),
    #[error("broker runtime directory is invalid")]
    InvalidRuntimeDirectory,
    #[error("broker socket path already exists")]
    SocketAlreadyExists,
    #[error("broker I/O failed")]
    Io(#[from] io::Error),
}

pub async fn run(
    config: BrokerConfig,
    credentials: KvmdCredentials,
) -> Result<(), BrokerServerError> {
    config.validate()?;
    validate_runtime_directory(&config)?;
    if fs::symlink_metadata(&config.broker_socket).is_ok() {
        return Err(BrokerServerError::SocketAlreadyExists);
    }
    let listener = UnixListener::bind(&config.broker_socket)?;
    fs::set_permissions(&config.broker_socket, Permissions::from_mode(0o660))?;

    let client = Arc::new(
        KvmdReadClient::new(&config.kvmd_socket)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?,
    );
    let credentials = Arc::new(credentials);
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    loop {
        let (stream, _) = listener.accept().await?;
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| io::Error::other("broker semaphore closed"))?;
        let client = client.clone();
        let credentials = credentials.clone();
        let request_timeout = config.request_timeout;
        tokio::spawn(async move {
            let _permit = permit;
            let _ = handle_connection(stream, client, credentials, request_timeout).await;
        });
    }
}

fn validate_runtime_directory(config: &BrokerConfig) -> Result<(), BrokerServerError> {
    let parent = config
        .broker_socket
        .parent()
        .ok_or(BrokerServerError::InvalidRuntimeDirectory)?;
    let metadata = fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.mode() & 0o002 != 0
    {
        return Err(BrokerServerError::InvalidRuntimeDirectory);
    }
    Ok(())
}

async fn handle_connection(
    mut stream: UnixStream,
    client: Arc<KvmdReadClient>,
    credentials: Arc<KvmdCredentials>,
    request_timeout: std::time::Duration,
) -> Result<(), io::Error> {
    let request = timeout(request_timeout, read_request(&mut stream))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker request timed out"))??;
    let response = process_request(request, &client, &credentials).await;
    let payload = encode_response(&response)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker response invalid"))?;
    let frame = encode_frame(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker response too large"))?;
    timeout(request_timeout, stream.write_all(&frame))
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "broker response timed out"))??;
    Ok(())
}

async fn read_request(stream: &mut UnixStream) -> Result<BrokerRequestEnvelope, io::Error> {
    let mut prefix = [0_u8; 4];
    stream.read_exact(&mut prefix).await?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_BROKER_MESSAGE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "broker request too large",
        ));
    }
    let mut payload = vec![0_u8; length];
    stream.read_exact(&mut payload).await?;
    decode_request(&payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "broker request invalid"))
}

async fn process_request(
    request: BrokerRequestEnvelope,
    client: &KvmdReadClient,
    credentials: &KvmdCredentials,
) -> BrokerResponseEnvelope {
    let response = match request.request {
        BrokerReadRequest::Inventory => match client.inventory(credentials).await {
            Ok(source) => {
                let thermal = match read_h616_temperature_millidegrees() {
                    Ok(value) => Some(value),
                    Err(error) => {
                        eprintln!("H616 thermal observation failed with {}", error.class());
                        None
                    }
                };
                let external_rtc = match read_h616_external_rtc() {
                    Ok(value) => Some(value),
                    Err(error) => {
                        eprintln!(
                            "H616 external RTC observation failed with {}",
                            error.class()
                        );
                        None
                    }
                };
                BrokerReadResponse::Inventory {
                    inventory: sanitize_kvmd_inventory_with_observations(
                        source,
                        BliPlatformObservations {
                            temperature_millidegrees_celsius: thermal,
                            external_rtc,
                        },
                    ),
                }
            }
            Err(error) => {
                let code = map_client_error(&error.source);
                eprintln!(
                    "KVMD inventory request failed with {code:?} at {} with {}",
                    error.endpoint.as_str(),
                    client_error_class(&error.source),
                );
                BrokerReadResponse::Error { code }
            }
        },
    };
    BrokerResponseEnvelope {
        schema_version: BROKER_SCHEMA_VERSION,
        request_id: request.request_id,
        response,
    }
}

fn client_error_class(error: &KvmdClientError) -> String {
    match error {
        KvmdClientError::Status(status) => format!("status_{}", status.as_u16()),
        KvmdClientError::Io(_) => "io".into(),
        KvmdClientError::Timeout => "timeout".into(),
        KvmdClientError::Hyper(_) => "hyper".into(),
        KvmdClientError::ResponseTooLarge => "response_too_large".into(),
        KvmdClientError::InvalidJson(_) => "invalid_json".into(),
        KvmdClientError::ApiRejected => "api_rejected".into(),
        KvmdClientError::MissingResult => "missing_result".into(),
        KvmdClientError::RelativeSocketPath => "relative_socket_path".into(),
        KvmdClientError::InvalidResponseLimit => "invalid_response_limit".into(),
        KvmdClientError::Http(_) => "http_build".into(),
    }
}

fn map_client_error(error: &KvmdClientError) -> BrokerErrorCode {
    match error {
        KvmdClientError::Status(status) if matches!(status.as_u16(), 401 | 403) => {
            BrokerErrorCode::BackendUnauthorized
        }
        KvmdClientError::Io(_) | KvmdClientError::Timeout => BrokerErrorCode::BackendUnavailable,
        KvmdClientError::Status(_)
        | KvmdClientError::Hyper(_)
        | KvmdClientError::ResponseTooLarge
        | KvmdClientError::InvalidJson(_)
        | KvmdClientError::ApiRejected
        | KvmdClientError::MissingResult => BrokerErrorCode::InvalidBackendResponse,
        KvmdClientError::RelativeSocketPath
        | KvmdClientError::InvalidResponseLimit
        | KvmdClientError::Http(_) => BrokerErrorCode::Internal,
    }
}

#[cfg(test)]
mod tests {
    use std::{path::Path, time::Duration};

    use bli_kvm_broker_protocol::{BrokerReadResponse, decode_response, encode_frame};
    use tokio::time::sleep;

    use super::*;

    #[test]
    fn maps_backend_errors_without_messages_or_payloads() {
        assert_eq!(
            map_client_error(&KvmdClientError::Status(http::StatusCode::UNAUTHORIZED)),
            BrokerErrorCode::BackendUnauthorized
        );
        assert_eq!(
            map_client_error(&KvmdClientError::ResponseTooLarge),
            BrokerErrorCode::InvalidBackendResponse
        );
        assert_eq!(
            client_error_class(&KvmdClientError::Status(http::StatusCode::BAD_REQUEST)),
            "status_400"
        );
    }

    async fn mock_kvmd(socket: &Path) -> tokio::task::JoinHandle<()> {
        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            for _ in 0..5 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if read == 0 || request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let path = request.split_whitespace().nth(1).unwrap();
                let body = match path {
                    "/info" => {
                        r#"{"ok":true,"result":{"system":{"kvmd":{"version":"3.291"},"streamer":{"version":"6.11"}},"extras":{"janus":{"enabled":false}}}}"#
                    }
                    "/atx" => {
                        r#"{"ok":true,"result":{"enabled":true,"busy":false,"leds":{"power":true}}}"#
                    }
                    "/hid" => {
                        r#"{"ok":true,"result":{"online":true,"busy":false,"mouse":{"online":true}}}"#
                    }
                    "/msd" => {
                        r#"{"ok":true,"result":{"enabled":true,"online":true,"busy":false,"drive":{"connected":false}}}"#
                    }
                    "/streamer" => {
                        r#"{"ok":true,"result":{"streamer":{"source":{"online":true}}}}"#
                    }
                    _ => panic!("unexpected read-only KVMD path"),
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        })
    }

    #[tokio::test]
    async fn broker_exposes_only_sanitized_inventory_end_to_end() {
        let directory = tempfile::tempdir().unwrap();
        let kvmd_socket = directory.path().join("kvmd.sock");
        let broker_socket = directory.path().join("broker.sock");
        let backend = mock_kvmd(&kvmd_socket).await;
        let config = BrokerConfig::new(&broker_socket, &kvmd_socket, directory.path()).unwrap();
        let broker = tokio::spawn(run(
            config,
            KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
        ));

        for _ in 0..100 {
            if broker_socket.exists() {
                break;
            }
            sleep(Duration::from_millis(5)).await;
        }
        assert!(broker_socket.exists());

        let payload = br#"{"schemaVersion":1,"requestId":42,"request":{"type":"inventory"}}"#;
        let mut stream = UnixStream::connect(&broker_socket).await.unwrap();
        stream
            .write_all(&encode_frame(payload).unwrap())
            .await
            .unwrap();
        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await.unwrap();
        let mut response = vec![0_u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut response).await.unwrap();
        let response = decode_response(&response).unwrap();
        assert_eq!(response.request_id, 42);
        let inventory = match response.response {
            BrokerReadResponse::Inventory { inventory } => inventory,
            BrokerReadResponse::Error { code } => {
                panic!("expected inventory response, received {code:?}")
            }
        };
        assert_eq!(inventory.video.online, Some(true));
        assert_eq!(inventory.hid.online, Some(true));
        assert_eq!(inventory.power_led, Some(true));
        assert_eq!(inventory.virtual_media.connected, Some(false));
        assert_eq!(inventory.janus_enabled, Some(false));

        backend.await.unwrap();
        broker.abort();
    }
}
