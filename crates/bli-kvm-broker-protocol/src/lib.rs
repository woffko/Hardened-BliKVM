//! Versioned and bounded platform-broker messages.
//!
//! Schema v1 is intentionally read-only. Adding a mutation is a protocol and
//! security review, not a generic string or passthrough request.

#[cfg(feature = "diagnostic-export")]
use std::fmt;

use bli_kvm_core::RuntimeInventory;
#[cfg(feature = "diagnostic-export")]
use bli_kvm_diagnostic_export::{DiagnosticBundle, MAX_EXPORT_BYTES, MAX_RECORDS};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const BROKER_SCHEMA_VERSION: u16 = 1;
#[cfg(feature = "diagnostic-export")]
pub const BROKER_DIAGNOSTIC_SCHEMA_VERSION: u16 = 2;
pub const MAX_BROKER_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerReadRequest {
    Inventory,
    #[cfg(feature = "diagnostic-export")]
    Diagnostics,
}

#[cfg(feature = "diagnostic-export")]
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Canonical typed diagnostic bundle relayed only across the local broker socket.
///
/// `Debug` deliberately omits the bundle. Construction and decoding both
/// revalidate the closed diagnostic grammar and the envelope record count.
pub struct BrokerDiagnosticSnapshot {
    instance_id: u64,
    window_id: u32,
    omitted_lifecycle_events: u8,
    record_count: u8,
    bundle: String,
}

#[cfg(feature = "diagnostic-export")]
impl BrokerDiagnosticSnapshot {
    /// Builds one local-broker snapshot from an already typed bundle.
    pub fn from_bundle(
        instance_id: u64,
        window_id: u32,
        omitted_lifecycle_events: u8,
        bundle: &DiagnosticBundle,
    ) -> Result<Self, BrokerProtocolError> {
        let snapshot = Self {
            instance_id,
            window_id,
            omitted_lifecycle_events,
            record_count: u8::try_from(bundle.record_count())
                .map_err(|_| BrokerProtocolError::InvalidDiagnostics)?,
            bundle: core::str::from_utf8(bundle.as_bytes())
                .map_err(|_| BrokerProtocolError::InvalidDiagnostics)?
                .to_owned(),
        };
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub const fn instance_id(&self) -> u64 {
        self.instance_id
    }

    pub const fn window_id(&self) -> u32 {
        self.window_id
    }

    pub const fn omitted_lifecycle_events(&self) -> u8 {
        self.omitted_lifecycle_events
    }

    pub const fn record_count(&self) -> u8 {
        self.record_count
    }

    pub fn bundle(&self) -> &str {
        &self.bundle
    }

    fn validate(&self) -> Result<(), BrokerProtocolError> {
        if self.instance_id == 0
            || self.window_id == 0
            || self.record_count == 0
            || usize::from(self.record_count) > MAX_RECORDS
            || self.bundle.len() > MAX_EXPORT_BYTES
        {
            return Err(BrokerProtocolError::InvalidDiagnostics);
        }
        let bundle = DiagnosticBundle::decode(self.bundle.as_bytes())
            .map_err(|_| BrokerProtocolError::InvalidDiagnostics)?;
        if bundle.record_count() != usize::from(self.record_count) {
            return Err(BrokerProtocolError::InvalidDiagnostics);
        }
        Ok(())
    }
}

#[cfg(feature = "diagnostic-export")]
impl fmt::Debug for BrokerDiagnosticSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BrokerDiagnosticSnapshot")
            .field("instance_id", &self.instance_id)
            .field("window_id", &self.window_id)
            .field("omitted_lifecycle_events", &self.omitted_lifecycle_events)
            .field("record_count", &self.record_count)
            .field("bundle_length", &self.bundle.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokerRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub request: BrokerReadRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerErrorCode {
    #[cfg(feature = "diagnostic-export")]
    Unauthorized,
    BackendUnavailable,
    BackendUnauthorized,
    InvalidBackendResponse,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerReadResponse {
    Inventory {
        inventory: RuntimeInventory,
    },
    #[cfg(feature = "diagnostic-export")]
    Diagnostics {
        snapshot: Option<BrokerDiagnosticSnapshot>,
    },
    Error {
        code: BrokerErrorCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BrokerResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub response: BrokerReadResponse,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BrokerProtocolError {
    #[error("broker message exceeded the size limit")]
    MessageTooLarge,
    #[error("broker message is invalid JSON")]
    InvalidJson,
    #[error("unsupported broker schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("runtime inventory is invalid")]
    InvalidInventory,
    #[cfg(feature = "diagnostic-export")]
    #[error("diagnostic snapshot is invalid")]
    InvalidDiagnostics,
    #[error("broker frame is invalid")]
    InvalidFrame,
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, BrokerProtocolError> {
    ensure_bounded(payload)?;
    let length = u32::try_from(payload.len()).map_err(|_| BrokerProtocolError::MessageTooLarge)?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<&[u8], BrokerProtocolError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(BrokerProtocolError::InvalidFrame)?
        .try_into()
        .map_err(|_| BrokerProtocolError::InvalidFrame)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_BROKER_MESSAGE_BYTES {
        return Err(BrokerProtocolError::MessageTooLarge);
    }
    if frame.len() != 4 + length {
        return Err(BrokerProtocolError::InvalidFrame);
    }
    Ok(&frame[4..])
}

pub fn decode_request(bytes: &[u8]) -> Result<BrokerRequestEnvelope, BrokerProtocolError> {
    ensure_bounded(bytes)?;
    let request: BrokerRequestEnvelope =
        serde_json::from_slice(bytes).map_err(|_| BrokerProtocolError::InvalidJson)?;
    validate_request_version(&request)?;
    Ok(request)
}

pub fn encode_request(request: &BrokerRequestEnvelope) -> Result<Vec<u8>, BrokerProtocolError> {
    validate_request_version(request)?;
    let bytes = serde_json::to_vec(request).map_err(|_| BrokerProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

pub fn encode_response(response: &BrokerResponseEnvelope) -> Result<Vec<u8>, BrokerProtocolError> {
    validate_response_version(response)?;
    validate_response(&response.response)?;
    let bytes = serde_json::to_vec(response).map_err(|_| BrokerProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<BrokerResponseEnvelope, BrokerProtocolError> {
    ensure_bounded(bytes)?;
    let response: BrokerResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| BrokerProtocolError::InvalidJson)?;
    validate_response_version(&response)?;
    validate_response(&response.response)?;
    Ok(response)
}

fn validate_request_version(request: &BrokerRequestEnvelope) -> Result<(), BrokerProtocolError> {
    let valid = match request.request {
        BrokerReadRequest::Inventory => request.schema_version == BROKER_SCHEMA_VERSION,
        #[cfg(feature = "diagnostic-export")]
        BrokerReadRequest::Diagnostics => {
            request.schema_version == BROKER_DIAGNOSTIC_SCHEMA_VERSION
        }
    };
    if valid {
        Ok(())
    } else {
        Err(BrokerProtocolError::UnsupportedSchemaVersion(
            request.schema_version,
        ))
    }
}

fn validate_response_version(response: &BrokerResponseEnvelope) -> Result<(), BrokerProtocolError> {
    let valid = match &response.response {
        BrokerReadResponse::Inventory { .. } => response.schema_version == BROKER_SCHEMA_VERSION,
        #[cfg(feature = "diagnostic-export")]
        BrokerReadResponse::Diagnostics { .. } => {
            response.schema_version == BROKER_DIAGNOSTIC_SCHEMA_VERSION
        }
        BrokerReadResponse::Error { .. } => error_schema_supported(response.schema_version),
    };
    if valid {
        Ok(())
    } else {
        Err(BrokerProtocolError::UnsupportedSchemaVersion(
            response.schema_version,
        ))
    }
}

fn error_schema_supported(schema_version: u16) -> bool {
    if schema_version == BROKER_SCHEMA_VERSION {
        return true;
    }
    #[cfg(feature = "diagnostic-export")]
    {
        schema_version == BROKER_DIAGNOSTIC_SCHEMA_VERSION
    }
    #[cfg(not(feature = "diagnostic-export"))]
    {
        false
    }
}

fn validate_response(response: &BrokerReadResponse) -> Result<(), BrokerProtocolError> {
    match response {
        BrokerReadResponse::Inventory { inventory } => inventory
            .validate()
            .map_err(|_| BrokerProtocolError::InvalidInventory),
        #[cfg(feature = "diagnostic-export")]
        BrokerReadResponse::Diagnostics {
            snapshot: Some(snapshot),
        } => snapshot.validate(),
        #[cfg(feature = "diagnostic-export")]
        BrokerReadResponse::Diagnostics { snapshot: None } | BrokerReadResponse::Error { .. } => {
            Ok(())
        }
        #[cfg(not(feature = "diagnostic-export"))]
        BrokerReadResponse::Error { .. } => Ok(()),
    }
}

fn ensure_bounded(bytes: &[u8]) -> Result<(), BrokerProtocolError> {
    if bytes.len() > MAX_BROKER_MESSAGE_BYTES {
        Err(BrokerProtocolError::MessageTooLarge)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_core::{
        ComponentObservation, InventoryBackend, RUNTIME_INVENTORY_SCHEMA_VERSION, RtcObservation,
        RuntimeVersions, ThermalObservation,
    };
    #[cfg(feature = "diagnostic-export")]
    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };

    use super::*;

    #[cfg(feature = "diagnostic-export")]
    fn bundle() -> DiagnosticBundle {
        let mut exporter = DiagnosticExporter::new();
        exporter
            .push(
                DiagnosticRecord::new(
                    7,
                    DiagnosticComponent::ControlBroker,
                    DiagnosticEvent::Ready,
                )
                .unwrap(),
            )
            .unwrap();
        exporter.export().unwrap()
    }

    #[test]
    fn v1_accepts_only_enabled_read_requests() {
        let request =
            decode_request(br#"{"schemaVersion":1,"requestId":7,"request":{"type":"inventory"}}"#)
                .unwrap();
        assert_eq!(request.request_id, 7);
        assert_eq!(request.request, BrokerReadRequest::Inventory);
        assert_eq!(
            decode_request(&encode_request(&request).unwrap()).unwrap(),
            request
        );
        let diagnostics = br#"{"schemaVersion":2,"requestId":8,"request":{"type":"diagnostics"}}"#;
        #[cfg(feature = "diagnostic-export")]
        {
            assert_eq!(
                decode_request(diagnostics).unwrap().request,
                BrokerReadRequest::Diagnostics
            );
            assert_eq!(
                decode_request(
                    br#"{"schemaVersion":1,"requestId":8,"request":{"type":"diagnostics"}}"#
                ),
                Err(BrokerProtocolError::UnsupportedSchemaVersion(1))
            );
            assert_eq!(
                decode_request(
                    br#"{"schemaVersion":2,"requestId":8,"request":{"type":"inventory"}}"#
                ),
                Err(BrokerProtocolError::UnsupportedSchemaVersion(2))
            );
        }
        #[cfg(not(feature = "diagnostic-export"))]
        assert_eq!(
            decode_request(diagnostics),
            Err(BrokerProtocolError::InvalidJson)
        );

        assert_eq!(
            decode_request(
                br#"{"schemaVersion":1,"requestId":7,"request":{"type":"power_short"}}"#
            ),
            Err(BrokerProtocolError::InvalidJson)
        );
    }

    #[test]
    fn response_contains_only_sanitized_inventory() {
        let inventory = RuntimeInventory {
            schema_version: RUNTIME_INVENTORY_SCHEMA_VERSION,
            platform_id: "bli-h616-pikvm".into(),
            backend: InventoryBackend::KvmdTransition,
            versions: RuntimeVersions::default(),
            video: ComponentObservation::default(),
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
            request_id: 7,
            response: BrokerReadResponse::Inventory { inventory },
        };
        let json = String::from_utf8(encode_response(&response).unwrap()).unwrap();
        assert!(json.contains("kvmd_transition"));
        for forbidden in ["password", "credential", "token", "username"] {
            assert!(!json.contains(forbidden));
        }
        assert_eq!(decode_response(json.as_bytes()).unwrap(), response);
    }

    #[cfg(feature = "diagnostic-export")]
    #[test]
    fn diagnostic_response_is_canonical_bounded_and_payload_redacted_in_debug() {
        let snapshot = BrokerDiagnosticSnapshot::from_bundle(7, 3, 2, &bundle()).unwrap();
        assert_eq!(snapshot.instance_id(), 7);
        assert_eq!(snapshot.window_id(), 3);
        assert_eq!(snapshot.omitted_lifecycle_events(), 2);
        assert_eq!(snapshot.record_count(), 1);
        assert!(snapshot.bundle().contains("component=control_broker"));
        assert!(!format!("{snapshot:?}").contains("component="));

        let response = BrokerResponseEnvelope {
            schema_version: BROKER_DIAGNOSTIC_SCHEMA_VERSION,
            request_id: 9,
            response: BrokerReadResponse::Diagnostics {
                snapshot: Some(snapshot.clone()),
            },
        };
        let encoded = encode_response(&response).unwrap();
        assert_eq!(decode_response(&encoded).unwrap(), response);
        let mut wrong_version = response.clone();
        wrong_version.schema_version = BROKER_SCHEMA_VERSION;
        assert_eq!(
            encode_response(&wrong_version),
            Err(BrokerProtocolError::UnsupportedSchemaVersion(
                BROKER_SCHEMA_VERSION
            ))
        );

        let mut invalid: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        invalid["response"]["snapshot"]["recordCount"] = 2.into();
        assert_eq!(
            decode_response(&serde_json::to_vec(&invalid).unwrap()),
            Err(BrokerProtocolError::InvalidDiagnostics)
        );
        invalid["response"]["snapshot"]["recordCount"] = 1.into();
        invalid["response"]["snapshot"]["bundle"] = "raw arbitrary text\n".into();
        assert_eq!(
            decode_response(&serde_json::to_vec(&invalid).unwrap()),
            Err(BrokerProtocolError::InvalidDiagnostics)
        );
    }

    #[test]
    fn oversized_messages_are_rejected_before_json_parsing() {
        let bytes = vec![b' '; MAX_BROKER_MESSAGE_BYTES + 1];
        assert_eq!(
            decode_request(&bytes),
            Err(BrokerProtocolError::MessageTooLarge)
        );
    }

    #[test]
    fn frame_requires_one_exact_bounded_message() {
        let payload = br#"{"schemaVersion":1}"#;
        let frame = encode_frame(payload).unwrap();
        assert_eq!(decode_frame(&frame).unwrap(), payload);
        assert_eq!(
            decode_frame(&frame[..frame.len() - 1]),
            Err(BrokerProtocolError::InvalidFrame)
        );
    }
}
