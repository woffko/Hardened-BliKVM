//! Closed requests from the update coordinator to the root execution helper.
//! No request carries a path, unit name, command, policy value or credential;
//! the helper maps each closed variant to its own fixed configuration.
use crate::{UpdateProtocolError, envelope, valid_hex, valid_slot_id};
use serde::{Deserialize, Serialize};

/// Execution messages are much smaller than status documents.
pub const MAX_EXECUTION_MESSAGE_BYTES: usize = 1024;
/// A session cannot issue more requests than a complete composition attempt.
/// An activation with confirmation uses 78 (counted by the native pipeline
/// test in bli-kvm-update-runtime); 64 ended the session during confirmation
/// on the H616 board.
pub const MAX_EXECUTION_SESSION_REQUESTS: u16 = 160;

/// Every unit the coordinator may stop or observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionUnit {
    BoardServer,
    CurrentServer,
    CandidateServer,
    BootstrapServer,
    BoardDaemon,
    VideoProxy,
    Owner,
}

/// Only native slot components may be started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStartUnit {
    Owner,
    VideoProxy,
    BoardDaemon,
    CurrentServer,
    CandidateServer,
}

impl ExecutionStartUnit {
    pub const fn unit(self) -> ExecutionUnit {
        match self {
            Self::Owner => ExecutionUnit::Owner,
            Self::VideoProxy => ExecutionUnit::VideoProxy,
            Self::BoardDaemon => ExecutionUnit::BoardDaemon,
            Self::CurrentServer => ExecutionUnit::CurrentServer,
            Self::CandidateServer => ExecutionUnit::CandidateServer,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionProbe {
    Owner,
    VideoProxy,
    BoardDaemon,
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionRequest {
    Prepare {
        #[serde(rename = "slotId")]
        slot_id: String,
    },
    Select {
        #[serde(rename = "slotId")]
        slot_id: String,
    },
    Stop {
        unit: ExecutionUnit,
    },
    ObserveStop {
        unit: ExecutionUnit,
    },
    ValidateStart {
        unit: ExecutionStartUnit,
    },
    Start {
        unit: ExecutionStartUnit,
    },
    ObserveIdentity {
        unit: ExecutionStartUnit,
    },
    Probe {
        probe: ExecutionProbe,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub request: ExecutionRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionStopObservation {
    Clean,
    RunningOrQueued,
    StoppedAfterFailure,
    Failed,
    Unknown,
}

/// Identity of a running unit bound to the selected protected slot copy.
/// `executable_sha256` is the signed manifest digest of the unit's role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub executable_device: u64,
    pub executable_inode: u64,
    pub slot_id: String,
    pub executable_sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionErrorCode {
    /// Another session is active or a managed unit still has a queued job.
    Busy,
    /// Local policy, signature, contract or state refused the request.
    Refused,
    /// The session deadline or request budget is exhausted.
    Deadline,
    /// The helper could not complete an observation or operation.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutionResponse {
    Done {},
    Prepared {
        #[serde(rename = "slotId")]
        slot_id: String,
    },
    Stop {
        observation: ExecutionStopObservation,
    },
    Identity {
        identity: ExecutionIdentity,
    },
    Ready {},
    Error {
        code: ExecutionErrorCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExecutionResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub response: ExecutionResponse,
}

fn bounded(bytes: &[u8]) -> Result<(), UpdateProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_EXECUTION_MESSAGE_BYTES {
        return Err(UpdateProtocolError::InvalidLength);
    }
    Ok(())
}

fn validate_request(value: &ExecutionRequestEnvelope) -> Result<(), UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    match &value.request {
        ExecutionRequest::Prepare { slot_id } | ExecutionRequest::Select { slot_id }
            if !valid_slot_id(slot_id) =>
        {
            Err(UpdateProtocolError::InvalidStatus)
        }
        _ => Ok(()),
    }
}

fn validate_response(value: &ExecutionResponseEnvelope) -> Result<(), UpdateProtocolError> {
    envelope(value.schema_version, value.request_id)?;
    let valid = match &value.response {
        ExecutionResponse::Prepared { slot_id } => valid_slot_id(slot_id),
        ExecutionResponse::Identity { identity } => {
            identity.pid != 0
                && identity.pid <= i32::MAX as u32
                && identity.start_ticks != 0
                && valid_slot_id(&identity.slot_id)
                && valid_hex(identity.executable_sha256.as_bytes(), 64)
        }
        _ => true,
    };
    if !valid {
        return Err(UpdateProtocolError::InvalidStatus);
    }
    Ok(())
}

pub fn encode_request(value: &ExecutionRequestEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    validate_request(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_request(bytes: &[u8]) -> Result<ExecutionRequestEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value = serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    validate_request(&value)?;
    Ok(value)
}

pub fn encode_response(value: &ExecutionResponseEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    validate_response(value)?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<ExecutionResponseEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value = serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    validate_response(&value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot() -> String {
        format!("app-{:020}-{}", 15, "a".repeat(64))
    }

    fn request(request: ExecutionRequest) -> ExecutionRequestEnvelope {
        ExecutionRequestEnvelope {
            schema_version: 1,
            request_id: 1,
            request,
        }
    }

    #[test]
    fn requests_are_closed_and_carry_no_paths_or_commands() {
        for value in [
            ExecutionRequest::Prepare { slot_id: slot() },
            ExecutionRequest::Select { slot_id: slot() },
            ExecutionRequest::Stop {
                unit: ExecutionUnit::BootstrapServer,
            },
            ExecutionRequest::ObserveStop {
                unit: ExecutionUnit::Owner,
            },
            ExecutionRequest::ValidateStart {
                unit: ExecutionStartUnit::CandidateServer,
            },
            ExecutionRequest::Start {
                unit: ExecutionStartUnit::Owner,
            },
            ExecutionRequest::ObserveIdentity {
                unit: ExecutionStartUnit::BoardDaemon,
            },
            ExecutionRequest::Probe {
                probe: ExecutionProbe::Web,
            },
        ] {
            let envelope = request(value);
            let bytes = encode_request(&envelope).unwrap();
            assert_eq!(decode_request(&bytes).unwrap(), envelope);
            assert!(crate::activation::decode_request(&bytes).is_err());
            for field in ["path", "unitName", "command", "floor", "systemSequence"] {
                let mut value = serde_json::to_value(&envelope).unwrap();
                value["request"][field] = serde_json::json!("untrusted");
                assert!(decode_request(&serde_json::to_vec(&value).unwrap()).is_err());
            }
        }
        let start = serde_json::json!({
            "schemaVersion": 1,
            "requestId": 1,
            "request": {"type": "start", "unit": "bootstrap_server"},
        });
        assert!(decode_request(&serde_json::to_vec(&start).unwrap()).is_err());
        let unit = serde_json::json!({
            "schemaVersion": 1,
            "requestId": 1,
            "request": {"type": "stop", "unit": "ssh.service"},
        });
        assert!(decode_request(&serde_json::to_vec(&unit).unwrap()).is_err());
    }

    #[test]
    fn slot_identifiers_and_envelopes_are_validated() {
        for invalid in [
            String::new(),
            "../current".into(),
            format!("app-{:020}-{}", 0, "a".repeat(64)),
            format!("app-{:020}-{}", 15, "A".repeat(64)),
        ] {
            let envelope = request(ExecutionRequest::Select { slot_id: invalid });
            assert!(encode_request(&envelope).is_err());
            let bytes = serde_json::to_vec(&envelope).unwrap();
            assert!(decode_request(&bytes).is_err());
        }
        let mut envelope = request(ExecutionRequest::Prepare { slot_id: slot() });
        envelope.request_id = 0;
        assert!(encode_request(&envelope).is_err());
        envelope.request_id = 1;
        envelope.schema_version = 2;
        assert!(encode_request(&envelope).is_err());
        assert!(decode_request(&[b' '; MAX_EXECUTION_MESSAGE_BYTES + 1]).is_err());
        assert!(decode_request(b"").is_err());
    }

    #[test]
    fn identity_response_requires_a_bound_signed_digest() {
        let identity = ExecutionIdentity {
            pid: 42,
            start_ticks: 7,
            executable_device: 1,
            executable_inode: 2,
            slot_id: slot(),
            executable_sha256: "b".repeat(64),
        };
        let response = ExecutionResponseEnvelope {
            schema_version: 1,
            request_id: 9,
            response: ExecutionResponse::Identity {
                identity: identity.clone(),
            },
        };
        let bytes = encode_response(&response).unwrap();
        assert_eq!(decode_response(&bytes).unwrap(), response);
        for broken in [
            ExecutionIdentity {
                pid: 0,
                ..identity.clone()
            },
            ExecutionIdentity {
                pid: u32::MAX,
                ..identity.clone()
            },
            ExecutionIdentity {
                start_ticks: 0,
                ..identity.clone()
            },
            ExecutionIdentity {
                executable_sha256: "0".repeat(64),
                ..identity.clone()
            },
            ExecutionIdentity {
                slot_id: "current".into(),
                ..identity
            },
        ] {
            let response = ExecutionResponseEnvelope {
                schema_version: 1,
                request_id: 9,
                response: ExecutionResponse::Identity { identity: broken },
            };
            assert!(encode_response(&response).is_err());
            assert!(decode_response(&serde_json::to_vec(&response).unwrap()).is_err());
        }
        let error = ExecutionResponseEnvelope {
            schema_version: 1,
            request_id: 9,
            response: ExecutionResponse::Error {
                code: ExecutionErrorCode::Busy,
            },
        };
        assert_eq!(
            decode_response(&encode_response(&error).unwrap()).unwrap(),
            error
        );
    }
}
