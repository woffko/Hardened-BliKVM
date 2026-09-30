//! Generation-fenced explicit activation on its own authenticated Unix socket.
//! Prepared and awaiting-health receipts are distinct; neither confirms installation.
use crate::{UpdateProtocolError, bounded, envelope, valid_hex, valid_slot_id};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ActivationRequest {
    Admit {
        #[serde(rename = "slotId")]
        slot_id: String,
        #[serde(rename = "expectedGeneration")]
        expected_generation: u64,
    },
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PreparedActivationReceipt {
    pub operation_id: String,
    pub target_slot_id: String,
    pub prepared_generation: u64,
    pub deadline_monotonic_ms: u64,
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
    Prepared { receipt: PreparedActivationReceipt },
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
    let (slot_id, expected_generation) = match &value.request {
        ActivationRequest::Activate {
            slot_id,
            expected_generation,
        }
        | ActivationRequest::Admit {
            slot_id,
            expected_generation,
        } => (slot_id, expected_generation),
    };
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
    if let ActivationResponse::Prepared { receipt } = &value.response {
        if !valid_slot_id(&receipt.target_slot_id)
            || !valid_hex(receipt.operation_id.as_bytes(), 32)
            || receipt.prepared_generation == 0
            || receipt.deadline_monotonic_ms == 0
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
    fn prepared_is_distinct_closed_and_preserves_full_width_counters() {
        let response = ActivationResponseEnvelope {
            schema_version: 1,
            request_id: 1,
            response: ActivationResponse::Prepared {
                receipt: PreparedActivationReceipt {
                    operation_id: "b".repeat(32),
                    target_slot_id: format!("app-00000000000000000002-{}", "a".repeat(64)),
                    prepared_generation: u64::MAX,
                    deadline_monotonic_ms: u64::MAX,
                },
            },
        };
        let bytes = encode_response(&response).unwrap();
        assert_eq!(decode_response(&bytes).unwrap(), response);
        for field in ["preparedGeneration", "deadlineMonotonicMs"] {
            let mut invalid = serde_json::to_value(&response).unwrap();
            invalid["response"]["receipt"][field] = serde_json::json!(0);
            assert!(decode_response(&serde_json::to_vec(&invalid).unwrap()).is_err());
        }
        let mut invalid = serde_json::to_value(&response).unwrap();
        invalid["response"]["type"] = serde_json::json!("awaiting_health");
        assert!(decode_response(&serde_json::to_vec(&invalid).unwrap()).is_err());
        invalid["response"]["type"] = serde_json::json!("prepared");
        invalid["response"]["receipt"]["confirmed"] = serde_json::json!(true);
        assert!(decode_response(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
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
        } = &mut zero.request
        else {
            unreachable!()
        };
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
