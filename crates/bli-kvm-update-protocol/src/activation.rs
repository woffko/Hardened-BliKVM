//! Generation-fenced explicit activation on its own authenticated Unix socket.
//! A receipt means awaiting candidate health, never confirmed installation.
use crate::{UpdateProtocolError, bounded, envelope, valid_hex, valid_slot_id};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationRequest {
    Activate {
        #[serde(rename = "slotId")]
        slot_id: String,
        #[serde(rename = "expectedGeneration")]
        expected_generation: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub request: ActivationRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationReceipt {
    pub operation_id: String,
    pub target_slot_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationErrorCode {
    StaleGeneration,
    AlreadyActive,
    PendingActivation,
    InvalidTarget,
    Busy,
    InvalidState,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationResponse {
    AwaitingHealth { receipt: ActivationReceipt },
    Error { code: ActivationErrorCode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub response: ActivationResponse,
}

fn validate_request(value: &ActivationRequestEnvelope) -> Result<(), UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    let ActivationRequest::Activate {
        slot_id,
        expected_generation,
    } = &value.request;
    if !valid_slot_id(slot_id) || *expected_generation == 0 {
        return Err(UpdateProtocolError::InvalidStatus);
    }
    Ok(())
}

fn validate_response(value: &ActivationResponseEnvelope) -> Result<(), UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    if let ActivationResponse::AwaitingHealth { receipt } = &value.response {
        if !valid_slot_id(&receipt.target_slot_id)
            || !valid_hex(receipt.operation_id.as_bytes(), 32)
        {
            return Err(UpdateProtocolError::InvalidStatus);
        }
    }
    Ok(())
}

pub fn encode_request(value: &ActivationRequestEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    validate_request(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_request(bytes: &[u8]) -> Result<ActivationRequestEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value = serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    validate_request(&value)?;
    Ok(value)
}

pub fn encode_response(value: &ActivationResponseEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    validate_response(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<ActivationResponseEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value = serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    validate_response(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_generation_fenced_request_and_health_receipt() {
        let slot = format!("app-{:020}-{}", u64::MAX, "a".repeat(64));
        let request = ActivationRequestEnvelope {
            schema_version: 1,
            request_id: 1,
            request: ActivationRequest::Activate {
                slot_id: slot.clone(),
                expected_generation: u64::MAX,
            },
        };
        let bytes = encode_request(&request).unwrap();
        assert_eq!(decode_request(&bytes).unwrap(), request);
        assert!(crate::decode_request(&bytes).is_err());
        assert!(crate::upload::decode_request(&bytes).is_err());
        for field in ["path", "url", "command", "healthToken", "confirmed"] {
            let mut value = serde_json::to_value(&request).unwrap();
            value["request"][field] = serde_json::json!("untrusted");
            assert!(decode_request(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let response = ActivationResponseEnvelope {
            schema_version: 1,
            request_id: 1,
            response: ActivationResponse::AwaitingHealth {
                receipt: ActivationReceipt {
                    operation_id: "b".repeat(32),
                    target_slot_id: slot,
                },
            },
        };
        assert_eq!(
            decode_response(&encode_response(&response).unwrap()).unwrap(),
            response
        );
        let mut zero = request;
        let ActivationRequest::Activate {
            expected_generation,
            ..
        } = &mut zero.request;
        *expected_generation = 0;
        assert!(encode_request(&zero).is_err());
        let mut invalid = response;
        let ActivationResponse::AwaitingHealth { receipt } = &mut invalid.response else {
            unreachable!()
        };
        receipt.operation_id = "0".repeat(32);
        assert!(encode_response(&invalid).is_err());
    }
}
