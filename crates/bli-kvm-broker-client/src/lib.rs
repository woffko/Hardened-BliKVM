use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[cfg(feature = "diagnostic-export")]
use bli_kvm_broker_protocol::BrokerDiagnosticSnapshot;
use bli_kvm_broker_protocol::{
    BROKER_SCHEMA_VERSION, BrokerErrorCode, BrokerProtocolError, BrokerReadRequest,
    BrokerReadResponse, BrokerRequestEnvelope, MAX_BROKER_MESSAGE_BYTES, decode_response,
    encode_frame, encode_request,
};
use bli_kvm_core::RuntimeInventory;
use thiserror::Error;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    time::timeout,
};

#[derive(Debug, Error)]
pub enum BrokerClientError {
    #[error("broker Unix socket path must be absolute")]
    RelativeSocketPath,
    #[error("broker request timeout must be greater than zero")]
    InvalidTimeout,
    #[error("broker request timed out")]
    Timeout,
    #[error("broker I/O failed")]
    Io(#[from] std::io::Error),
    #[error("broker protocol failed")]
    Protocol(#[from] BrokerProtocolError),
    #[error("broker response ID did not match the request")]
    MismatchedRequestId,
    #[error("broker response type did not match the request")]
    MismatchedResponse,
    #[error("broker returned {0:?}")]
    Broker(BrokerErrorCode),
}

#[derive(Debug)]
pub struct BrokerReadClient {
    socket_path: PathBuf,
    request_timeout: Duration,
    next_request_id: AtomicU64,
}

impl BrokerReadClient {
    pub fn new(socket_path: impl Into<PathBuf>) -> Result<Self, BrokerClientError> {
        Self::with_timeout(socket_path, Duration::from_secs(10))
    }

    pub fn with_timeout(
        socket_path: impl Into<PathBuf>,
        request_timeout: Duration,
    ) -> Result<Self, BrokerClientError> {
        let socket_path = socket_path.into();
        if !socket_path.is_absolute() {
            return Err(BrokerClientError::RelativeSocketPath);
        }
        if request_timeout.is_zero() {
            return Err(BrokerClientError::InvalidTimeout);
        }
        Ok(Self {
            socket_path,
            request_timeout,
            next_request_id: AtomicU64::new(1),
        })
    }

    pub async fn inventory(&self) -> Result<RuntimeInventory, BrokerClientError> {
        let response = timeout(
            self.request_timeout,
            self.execute(BrokerReadRequest::Inventory),
        )
        .await
        .map_err(|_| BrokerClientError::Timeout)??;
        match response {
            BrokerReadResponse::Inventory { inventory } => Ok(inventory),
            BrokerReadResponse::Error { code } => Err(BrokerClientError::Broker(code)),
            #[cfg(feature = "diagnostic-export")]
            BrokerReadResponse::Diagnostics { .. } => Err(BrokerClientError::MismatchedResponse),
        }
    }

    #[cfg(feature = "diagnostic-export")]
    /// Reads one diagnostic snapshot through the feature-gated local route.
    ///
    /// This method never acknowledges collector state and has no HTTP caller
    /// in the current server.
    pub async fn diagnostics(&self) -> Result<Option<BrokerDiagnosticSnapshot>, BrokerClientError> {
        let response = timeout(
            self.request_timeout,
            self.execute(BrokerReadRequest::Diagnostics),
        )
        .await
        .map_err(|_| BrokerClientError::Timeout)??;
        match response {
            BrokerReadResponse::Diagnostics { snapshot } => Ok(snapshot),
            BrokerReadResponse::Error { code } => Err(BrokerClientError::Broker(code)),
            BrokerReadResponse::Inventory { .. } => Err(BrokerClientError::MismatchedResponse),
        }
    }

    async fn execute(
        &self,
        request: BrokerReadRequest,
    ) -> Result<BrokerReadResponse, BrokerClientError> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let request = BrokerRequestEnvelope {
            schema_version: BROKER_SCHEMA_VERSION,
            request_id,
            request,
        };
        let payload = encode_request(&request)?;
        let frame = encode_frame(&payload)?;
        let mut stream = UnixStream::connect(&self.socket_path).await?;
        stream.write_all(&frame).await?;

        let mut prefix = [0_u8; 4];
        stream.read_exact(&mut prefix).await?;
        let length = u32::from_be_bytes(prefix) as usize;
        if length > MAX_BROKER_MESSAGE_BYTES {
            return Err(BrokerProtocolError::MessageTooLarge.into());
        }
        let mut payload = vec![0_u8; length];
        stream.read_exact(&mut payload).await?;
        let response = decode_response(&payload)?;
        if response.request_id != request_id {
            return Err(BrokerClientError::MismatchedRequestId);
        }
        Ok(response.response)
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "diagnostic-export")]
    use bli_kvm_broker_protocol::BrokerDiagnosticSnapshot;
    use bli_kvm_broker_protocol::{BrokerResponseEnvelope, encode_response};
    use bli_kvm_core::{
        ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RtcObservation,
        RuntimeVersions, ThermalObservation,
    };
    #[cfg(feature = "diagnostic-export")]
    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };
    use tokio::net::UnixListener;

    use super::*;

    #[tokio::test]
    async fn reads_one_sanitized_inventory_frame() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("broker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut request = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut request).await.unwrap();
            let request = bli_kvm_broker_protocol::decode_request(&request).unwrap();
            let inventory = RuntimeInventory {
                schema_version: RUNTIME_INVENTORY_SCHEMA_VERSION,
                platform_id: "bli-h616-pikvm".into(),
                backend: InventoryBackend::KvmdTransition,
                versions: RuntimeVersions::default(),
                video: ComponentObservation {
                    online: Some(true),
                    ..ComponentObservation::default()
                },
                hid: ComponentObservation::default(),
                power: ComponentObservation::default(),
                virtual_media: ComponentObservation::default(),
                thermal: ThermalObservation::default(),
                rtc: RtcObservation::default(),
                power_led: None,
                janus_enabled: Some(false),
            };
            let response = BrokerResponseEnvelope {
                schema_version: BROKER_SCHEMA_VERSION,
                request_id: request.request_id,
                response: BrokerReadResponse::Inventory { inventory },
            };
            let payload = encode_response(&response).unwrap();
            stream
                .write_all(&encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });

        let inventory = BrokerReadClient::new(&socket)
            .unwrap()
            .inventory()
            .await
            .unwrap();
        assert_eq!(inventory.video.online, Some(true));
        assert_eq!(inventory.janus_enabled, Some(false));
        server.await.unwrap();
    }

    #[cfg(feature = "diagnostic-export")]
    #[tokio::test]
    async fn reads_one_typed_diagnostic_snapshot_without_acknowledgement() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("broker.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0_u8; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut request = vec![0_u8; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut request).await.unwrap();
            let request = bli_kvm_broker_protocol::decode_request(&request).unwrap();
            assert_eq!(request.request, BrokerReadRequest::Diagnostics);

            let mut exporter = DiagnosticExporter::new();
            exporter
                .push(
                    DiagnosticRecord::new(
                        5,
                        DiagnosticComponent::ControlBroker,
                        DiagnosticEvent::Ready,
                    )
                    .unwrap(),
                )
                .unwrap();
            let snapshot =
                BrokerDiagnosticSnapshot::from_bundle(7, 3, 1, &exporter.export().unwrap())
                    .unwrap();
            let response = BrokerResponseEnvelope {
                schema_version: BROKER_SCHEMA_VERSION,
                request_id: request.request_id,
                response: BrokerReadResponse::Diagnostics {
                    snapshot: Some(snapshot),
                },
            };
            let payload = encode_response(&response).unwrap();
            stream
                .write_all(&encode_frame(&payload).unwrap())
                .await
                .unwrap();
        });

        let snapshot = BrokerReadClient::new(&socket)
            .unwrap()
            .diagnostics()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(snapshot.instance_id(), 7);
        assert_eq!(snapshot.window_id(), 3);
        assert_eq!(snapshot.record_count(), 1);
        assert!(snapshot.bundle().contains("event=ready"));
        server.await.unwrap();
    }

    #[test]
    fn rejects_relative_socket_paths() {
        assert!(matches!(
            BrokerReadClient::new("broker.sock"),
            Err(BrokerClientError::RelativeSocketPath)
        ));
    }
}
