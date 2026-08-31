//! Versioned and bounded platform-broker messages.
//!
//! Schema v1 is intentionally read-only. Adding a mutation is a protocol and
//! security review, not a generic string or passthrough request.

use bli_kvm_core::RuntimeInventory;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const BROKER_SCHEMA_VERSION: u16 = 1;
pub const MAX_BROKER_MESSAGE_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerReadRequest {
    Inventory,
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
    BackendUnavailable,
    BackendUnauthorized,
    InvalidBackendResponse,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BrokerReadResponse {
    Inventory { inventory: RuntimeInventory },
    Error { code: BrokerErrorCode },
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
    if request.schema_version != BROKER_SCHEMA_VERSION {
        return Err(BrokerProtocolError::UnsupportedSchemaVersion(
            request.schema_version,
        ));
    }
    Ok(request)
}

pub fn encode_request(request: &BrokerRequestEnvelope) -> Result<Vec<u8>, BrokerProtocolError> {
    if request.schema_version != BROKER_SCHEMA_VERSION {
        return Err(BrokerProtocolError::UnsupportedSchemaVersion(
            request.schema_version,
        ));
    }
    let bytes = serde_json::to_vec(request).map_err(|_| BrokerProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

pub fn encode_response(response: &BrokerResponseEnvelope) -> Result<Vec<u8>, BrokerProtocolError> {
    if response.schema_version != BROKER_SCHEMA_VERSION {
        return Err(BrokerProtocolError::UnsupportedSchemaVersion(
            response.schema_version,
        ));
    }
    if let BrokerReadResponse::Inventory { inventory } = &response.response {
        inventory
            .validate()
            .map_err(|_| BrokerProtocolError::InvalidInventory)?;
    }
    let bytes = serde_json::to_vec(response).map_err(|_| BrokerProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<BrokerResponseEnvelope, BrokerProtocolError> {
    ensure_bounded(bytes)?;
    let response: BrokerResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| BrokerProtocolError::InvalidJson)?;
    if response.schema_version != BROKER_SCHEMA_VERSION {
        return Err(BrokerProtocolError::UnsupportedSchemaVersion(
            response.schema_version,
        ));
    }
    if let BrokerReadResponse::Inventory { inventory } = &response.response {
        inventory
            .validate()
            .map_err(|_| BrokerProtocolError::InvalidInventory)?;
    }
    Ok(response)
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

    use super::*;

    #[test]
    fn v1_accepts_only_the_inventory_request() {
        let request =
            decode_request(br#"{"schemaVersion":1,"requestId":7,"request":{"type":"inventory"}}"#)
                .unwrap();
        assert_eq!(request.request_id, 7);
        assert_eq!(request.request, BrokerReadRequest::Inventory);
        assert_eq!(
            decode_request(&encode_request(&request).unwrap()).unwrap(),
            request
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
