//! Closed, bounded metadata for the local application-update boundary.
//! Status requests contain no mutation, path, command, URL or credential.

use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const UPDATE_SCHEMA_VERSION: u16 = 1;
pub const MAX_UPDATE_MESSAGE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateReadRequest {
    Status {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub request: UpdateReadRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationSlotVersion {
    pub slot_id: String,
    pub version: String,
    pub sequence: u64,
    pub minimum_system_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePendingPhase {
    Prepared,
    AwaitingHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingApplicationUpdate {
    pub operation_id: String,
    pub target: ApplicationSlotVersion,
    pub phase: UpdatePendingPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationRollbackReason {
    InterruptedPrepare,
    BootChanged,
    HealthTimeout,
    ExplicitHealthFailure,
    PointerMismatch,
    TargetInvalid,
    HealthChallengeLost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApplicationUpdateStatus {
    pub generation: u64,
    pub floor_sequence: u64,
    pub system_sequence: u64,
    pub selected: ApplicationSlotVersion,
    pub confirmed: ApplicationSlotVersion,
    pub previous: Option<ApplicationSlotVersion>,
    pub pending: Option<PendingApplicationUpdate>,
    pub last_rollback: Option<ApplicationRollbackReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpdateReadErrorCode {
    Busy,
    InvalidState,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UpdateReadResponse {
    Status {
        status: Box<ApplicationUpdateStatus>,
    },
    Error {
        code: UpdateReadErrorCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub response: UpdateReadResponse,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum UpdateProtocolError {
    #[error("update message length is invalid")]
    InvalidLength,
    #[error("update message JSON is invalid")]
    InvalidJson,
    #[error("update envelope is invalid")]
    InvalidEnvelope,
    #[error("update status is invalid")]
    InvalidStatus,
}

pub fn valid_slot_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 89
        && bytes.starts_with(b"app-")
        && bytes[4..24].iter().all(u8::is_ascii_digit)
        && bytes[24] == b'-'
        && valid_hex(&bytes[25..], 64)
        && value[4..24]
            .parse::<u64>()
            .is_ok_and(|sequence| sequence > 0)
}

fn valid_hex(bytes: &[u8], length: usize) -> bool {
    bytes.len() == length
        && bytes.iter().any(|byte| *byte != b'0')
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
}

impl ApplicationSlotVersion {
    pub fn validate(&self) -> Result<(), UpdateProtocolError> {
        if !valid_slot_id(&self.slot_id)
            || self.slot_id[4..24].parse::<u64>().ok() != Some(self.sequence)
            || self.minimum_system_sequence == 0
            || self.version.is_empty()
            || self.version.len() > 64
            || !self
                .version
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b".+-".contains(&byte))
        {
            return Err(UpdateProtocolError::InvalidStatus);
        }
        Ok(())
    }
}

impl ApplicationUpdateStatus {
    pub fn validate(&self) -> Result<(), UpdateProtocolError> {
        self.selected.validate()?;
        self.confirmed.validate()?;
        if self.generation == 0
            || self.system_sequence == 0
            || self.floor_sequence != self.confirmed.sequence
            || self.selected.minimum_system_sequence > self.system_sequence
            || self.confirmed.minimum_system_sequence > self.system_sequence
        {
            return Err(UpdateProtocolError::InvalidStatus);
        }
        if let Some(previous) = &self.previous {
            previous.validate()?;
        }
        if let Some(pending) = &self.pending {
            pending.target.validate()?;
            if !valid_hex(pending.operation_id.as_bytes(), 32)
                || pending.target.sequence <= self.floor_sequence
                || pending.target.minimum_system_sequence > self.system_sequence
                || (self.selected != self.confirmed && self.selected != pending.target)
                || (pending.phase == UpdatePendingPhase::AwaitingHealth
                    && self.selected != pending.target)
            {
                return Err(UpdateProtocolError::InvalidStatus);
            }
        } else if self.selected != self.confirmed {
            return Err(UpdateProtocolError::InvalidStatus);
        }
        Ok(())
    }
}

fn envelope(schema: u16, id: u64) -> Result<(), UpdateProtocolError> {
    if schema != UPDATE_SCHEMA_VERSION || id == 0 {
        return Err(UpdateProtocolError::InvalidEnvelope);
    }
    Ok(())
}

fn bounded(bytes: &[u8]) -> Result<(), UpdateProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_UPDATE_MESSAGE_BYTES {
        return Err(UpdateProtocolError::InvalidLength);
    }
    Ok(())
}

pub fn encode_frame(bytes: &[u8]) -> Result<Vec<u8>, UpdateProtocolError> {
    bounded(bytes)?;
    let mut frame = Vec::with_capacity(4 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    frame.extend_from_slice(bytes);
    Ok(frame)
}

pub fn decode_frame(frame: &[u8]) -> Result<&[u8], UpdateProtocolError> {
    let prefix: [u8; 4] = frame
        .get(..4)
        .ok_or(UpdateProtocolError::InvalidLength)?
        .try_into()
        .map_err(|_| UpdateProtocolError::InvalidLength)?;
    let length = u32::from_be_bytes(prefix) as usize;
    let bytes = frame.get(4..).ok_or(UpdateProtocolError::InvalidLength)?;
    bounded(bytes)?;
    if length != bytes.len() {
        return Err(UpdateProtocolError::InvalidLength);
    }
    Ok(bytes)
}

pub fn decode_request(bytes: &[u8]) -> Result<UpdateRequestEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value: UpdateRequestEnvelope =
        serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    envelope(value.schema_version, value.request_id)?;
    Ok(value)
}

pub fn encode_request(value: &UpdateRequestEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<UpdateResponseEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value: UpdateResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    validate_response(&value)?;
    Ok(value)
}

pub fn encode_response(value: &UpdateResponseEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    validate_response(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

fn validate_response(value: &UpdateResponseEnvelope) -> Result<(), UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    if let UpdateReadResponse::Status { status } = &value.response {
        status.validate()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(sequence: u64) -> ApplicationSlotVersion {
        ApplicationSlotVersion {
            slot_id: format!("app-{sequence:020}-{}", "a".repeat(64)),
            version: "0.1.0-dev2".into(),
            sequence,
            minimum_system_sequence: 1,
        }
    }

    fn status() -> ApplicationUpdateStatus {
        ApplicationUpdateStatus {
            generation: 10,
            floor_sequence: 2,
            system_sequence: 1,
            selected: slot(2),
            confirmed: slot(2),
            previous: Some(slot(3)),
            pending: None,
            last_rollback: Some(ApplicationRollbackReason::BootChanged),
        }
    }

    #[test]
    fn status_and_request_round_trip_with_exact_frames() {
        let request = UpdateRequestEnvelope {
            schema_version: 1,
            request_id: 42,
            request: UpdateReadRequest::Status {},
        };
        let bytes = encode_request(&request).unwrap();
        assert_eq!(
            decode_request(decode_frame(&encode_frame(&bytes).unwrap()).unwrap()).unwrap(),
            request
        );
        let response = UpdateResponseEnvelope {
            schema_version: 1,
            request_id: 42,
            response: UpdateReadResponse::Status {
                status: Box::new(status()),
            },
        };
        assert_eq!(
            decode_response(&encode_response(&response).unwrap()).unwrap(),
            response
        );
    }

    #[test]
    fn rejects_mutations_paths_unknown_fields_and_invalid_ids() {
        for bytes in [
            br#"{"schemaVersion":1,"requestId":1,"request":{"type":"activate","slot":"anything"}}"#
                .as_slice(),
            br#"{"schemaVersion":1,"requestId":1,"request":{"type":"status","path":"/etc"}}"#,
            br#"{"schemaVersion":1,"requestId":1,"request":{"type":"status"},"extra":true}"#,
            br#"{"schemaVersion":2,"requestId":1,"request":{"type":"status"}}"#,
            br#"{"schemaVersion":1,"requestId":0,"request":{"type":"status"}}"#,
        ] {
            assert!(decode_request(bytes).is_err());
        }
    }

    #[test]
    fn bounds_frames_and_rejects_trailing_or_truncated_data() {
        assert!(encode_frame(&[]).is_err());
        assert!(encode_frame(&vec![0; MAX_UPDATE_MESSAGE_BYTES + 1]).is_err());
        let frame = encode_frame(b"abc").unwrap();
        assert!(decode_frame(&frame[..frame.len() - 1]).is_err());
        assert!(decode_frame(&[frame, vec![0]].concat()).is_err());
        assert!(decode_request(&vec![b' '; MAX_UPDATE_MESSAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn validates_pending_and_confirmed_relationships() {
        let mut value = status();
        value.selected = slot(3);
        assert!(value.validate().is_err());
        value.pending = Some(PendingApplicationUpdate {
            operation_id: "b".repeat(32),
            target: slot(3),
            phase: UpdatePendingPhase::AwaitingHealth,
        });
        assert!(value.validate().is_ok());
        value.floor_sequence = 3;
        assert!(value.validate().is_err());
        value.floor_sequence = 2;
        value.pending.as_mut().unwrap().operation_id = "0".repeat(32);
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_unsafe_or_inconsistent_slot_metadata_without_panics() {
        for value in [
            "",
            "../slot",
            "ééé",
            &format!("app-{}-{}", "9".repeat(20), "a".repeat(64)),
            &format!("app-{}-{}", "0".repeat(20), "a".repeat(64)),
        ] {
            assert!(!valid_slot_id(value));
        }
        let mut value = slot(2);
        value.sequence = 3;
        assert!(value.validate().is_err());
        value = slot(2);
        value.version = "<script>".into();
        assert!(value.validate().is_err());
    }
}
