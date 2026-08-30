//! Versioned, bounded, and typed platform-control messages.
//!
//! This protocol does not enable a capability by itself. Runtime policy,
//! arming, cooldown, idempotency, and backend gates remain mandatory.

use bli_kvm_core::PlatformControlRequest;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONTROL_SCHEMA_VERSION: u16 = 1;
pub const MAX_CONTROL_MESSAGE_BYTES: usize = 4 * 1024;
const OPERATION_ID_LENGTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub operation_id: String,
    pub request: PlatformControlRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRejectionCode {
    CapabilityDisabled,
    NotArmed,
    CooldownActive,
    InvalidState,
    BackendUnavailable,
    BackendRejected,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResponse {
    Applied,
    Rejected { code: ControlRejectionCode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub operation_id: String,
    pub response: ControlResponse,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ControlProtocolError {
    #[error("control message exceeded the size limit")]
    MessageTooLarge,
    #[error("control message is invalid JSON")]
    InvalidJson,
    #[error("unsupported control schema version {0}")]
    UnsupportedSchemaVersion(u16),
    #[error("control request ID is invalid")]
    InvalidRequestId,
    #[error("control operation ID is invalid")]
    InvalidOperationId,
    #[error("control request is invalid")]
    InvalidControlRequest,
    #[error("control frame is invalid")]
    InvalidFrame,
}

pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, ControlProtocolError> {
    ensure_bounded(payload)?;
    let length = u32::try_from(payload.len()).map_err(|_| ControlProtocolError::MessageTooLarge)?;
    let mut frame = Vec::with_capacity(4 + payload.len());
    frame.extend_from_slice(&length.to_be_bytes());
    frame.extend_from_slice(payload);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<&[u8], ControlProtocolError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(ControlProtocolError::InvalidFrame)?
        .try_into()
        .map_err(|_| ControlProtocolError::InvalidFrame)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if length > MAX_CONTROL_MESSAGE_BYTES {
        return Err(ControlProtocolError::MessageTooLarge);
    }
    if frame.len() != 4 + length {
        return Err(ControlProtocolError::InvalidFrame);
    }
    Ok(&frame[4..])
}

pub fn decode_request(bytes: &[u8]) -> Result<ControlRequestEnvelope, ControlProtocolError> {
    ensure_bounded(bytes)?;
    let request: ControlRequestEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        request.schema_version,
        request.request_id,
        &request.operation_id,
    )?;
    request
        .request
        .validate()
        .map_err(|_| ControlProtocolError::InvalidControlRequest)?;
    Ok(request)
}

pub fn encode_request(request: &ControlRequestEnvelope) -> Result<Vec<u8>, ControlProtocolError> {
    validate_common(
        request.schema_version,
        request.request_id,
        &request.operation_id,
    )?;
    request
        .request
        .validate()
        .map_err(|_| ControlProtocolError::InvalidControlRequest)?;
    encode_json(request)
}

pub fn decode_response(bytes: &[u8]) -> Result<ControlResponseEnvelope, ControlProtocolError> {
    ensure_bounded(bytes)?;
    let response: ControlResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    Ok(response)
}

pub fn encode_response(
    response: &ControlResponseEnvelope,
) -> Result<Vec<u8>, ControlProtocolError> {
    validate_common(
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    encode_json(response)
}

fn encode_json(value: &impl Serialize) -> Result<Vec<u8>, ControlProtocolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ControlProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

fn validate_common(
    schema_version: u16,
    request_id: u64,
    operation_id: &str,
) -> Result<(), ControlProtocolError> {
    if schema_version != CONTROL_SCHEMA_VERSION {
        return Err(ControlProtocolError::UnsupportedSchemaVersion(
            schema_version,
        ));
    }
    if request_id == 0 {
        return Err(ControlProtocolError::InvalidRequestId);
    }
    if operation_id.len() != OPERATION_ID_LENGTH
        || operation_id.bytes().all(|byte| byte == b'0')
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ControlProtocolError::InvalidOperationId);
    }
    Ok(())
}

fn ensure_bounded(bytes: &[u8]) -> Result<(), ControlProtocolError> {
    if bytes.len() > MAX_CONTROL_MESSAGE_BYTES {
        Err(ControlProtocolError::MessageTooLarge)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_core::{AtxControlAction, HidReport, VirtualMediaCommand};

    use super::*;

    const OPERATION_ID: &str = "0123456789abcdef0123456789abcdef";

    fn keyboard_request() -> ControlRequestEnvelope {
        ControlRequestEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id: 7,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequest::Hid {
                report: HidReport::Keyboard {
                    data: [0, 0, 4, 0, 0, 0, 0, 0],
                },
            },
        }
    }

    #[test]
    fn valid_request_roundtrips_without_generic_paths_or_commands() {
        let request = keyboard_request();
        let json = encode_request(&request).unwrap();
        assert_eq!(decode_request(&json).unwrap(), request);
        for forbidden in ["path", "url", "shell", "command", "credential"] {
            assert!(!String::from_utf8_lossy(&json).contains(forbidden));
        }
    }

    #[test]
    fn operation_id_is_exact_lowercase_hex_and_nonzero() {
        for invalid in [
            "00000000000000000000000000000000",
            "0123456789abcdef",
            "0123456789ABCDEF0123456789ABCDEF",
            "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz",
        ] {
            let mut request = keyboard_request();
            request.operation_id = invalid.into();
            assert_eq!(
                encode_request(&request),
                Err(ControlProtocolError::InvalidOperationId)
            );
        }
    }

    #[test]
    fn request_id_zero_is_rejected() {
        let mut request = keyboard_request();
        request.request_id = 0;
        assert_eq!(
            encode_request(&request),
            Err(ControlProtocolError::InvalidRequestId)
        );
    }

    #[test]
    fn invalid_hid_and_media_payloads_fail_before_runtime_policy() {
        let invalid_hid = br#"{"schemaVersion":1,"requestId":1,"operationId":"0123456789abcdef0123456789abcdef","request":{"type":"hid","report":{"mode":"keyboard","data":[0,1,4,0,0,0,0,0]}}}"#;
        assert_eq!(
            decode_request(invalid_hid),
            Err(ControlProtocolError::InvalidControlRequest)
        );
        let invalid_media = ControlRequestEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id: 2,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequest::VirtualMedia {
                command: VirtualMediaCommand::Select {
                    image_id: 0,
                    mode: bli_kvm_core::VirtualMediaControlMode::RemovableDisk,
                    writable: false,
                },
            },
        };
        assert_eq!(
            encode_request(&invalid_media),
            Err(ControlProtocolError::InvalidControlRequest)
        );
    }

    #[test]
    fn atx_response_is_typed_and_contains_no_backend_detail() {
        let request = ControlRequestEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id: 8,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequest::Atx {
                action: AtxControlAction::PowerShort,
            },
        };
        assert!(encode_request(&request).is_ok());
        let response = ControlResponseEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION,
            request_id: request.request_id,
            operation_id: request.operation_id,
            response: ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled,
            },
        };
        let json = encode_response(&response).unwrap();
        assert_eq!(decode_response(&json).unwrap(), response);
        assert!(!String::from_utf8_lossy(&json).contains("message"));
    }

    #[test]
    fn frame_requires_one_exact_bounded_message() {
        let payload = encode_request(&keyboard_request()).unwrap();
        let frame = encode_frame(&payload).unwrap();
        assert_eq!(decode_frame(&frame).unwrap(), payload);
        assert_eq!(
            decode_frame(&frame[..frame.len() - 1]),
            Err(ControlProtocolError::InvalidFrame)
        );
        assert_eq!(
            encode_frame(&vec![0; MAX_CONTROL_MESSAGE_BYTES + 1]),
            Err(ControlProtocolError::MessageTooLarge)
        );
    }
}
