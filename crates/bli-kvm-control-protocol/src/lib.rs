//! Versioned, bounded, and typed platform-control messages.
//!
//! This protocol does not enable a capability by itself. Runtime policy,
//! arming, cooldown, idempotency, and backend gates remain mandatory.

use bli_kvm_core::{PlatformControlRequest, PlatformControlRequestV2};
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONTROL_SCHEMA_VERSION: u16 = 1;
pub const CONTROL_SCHEMA_VERSION_V2: u16 = 2;
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
    NotAttested,
    LeaseConflict,
    LeaseExpired,
    CooldownActive,
    ConfirmationStale,
    Duplicate,
    PendingUnreconciled,
    InvalidState,
    BackendUnavailable,
    BackendRejected,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ControlResponse {
    Applied,
    LeaseGranted {
        lease_id: [u8; 16],
    },
    MediaState {
        file: Option<String>,
        connected: bool,
        cdrom: bool,
        writable: bool,
    },
    BoardStatus {
        power_led_present: bool,
        power_led_on: bool,
        hdd_led_present: bool,
        hdd_led_on: bool,
    },
    /// The video settings the owner will run the streamer with.
    VideoState {
        fps: u16,
        quality: u8,
    },
    /// Whether the device name is published over mDNS.
    MdnsState {
        enabled: bool,
    },
    Rejected {
        code: ControlRejectionCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub operation_id: String,
    pub response: ControlResponse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ControlRequestEnvelopeV2 {
    pub schema_version: u16,
    pub request_id: u64,
    pub operation_id: String,
    pub request: PlatformControlRequestV2,
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
    #[error("control response is invalid")]
    InvalidControlResponse,
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
    ensure_version(peek_schema_version(bytes)?, CONTROL_SCHEMA_VERSION)?;
    let request: ControlRequestEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        CONTROL_SCHEMA_VERSION,
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
        CONTROL_SCHEMA_VERSION,
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

pub fn decode_request_v2(bytes: &[u8]) -> Result<ControlRequestEnvelopeV2, ControlProtocolError> {
    ensure_bounded(bytes)?;
    ensure_version(peek_schema_version(bytes)?, CONTROL_SCHEMA_VERSION_V2)?;
    let request: ControlRequestEnvelopeV2 =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        CONTROL_SCHEMA_VERSION_V2,
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

pub fn encode_request_v2(
    request: &ControlRequestEnvelopeV2,
) -> Result<Vec<u8>, ControlProtocolError> {
    validate_common(
        CONTROL_SCHEMA_VERSION_V2,
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
    ensure_version(peek_schema_version(bytes)?, CONTROL_SCHEMA_VERSION)?;
    let response: ControlResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        CONTROL_SCHEMA_VERSION,
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    validate_response(&response.response)?;
    Ok(response)
}

pub fn encode_response(
    response: &ControlResponseEnvelope,
) -> Result<Vec<u8>, ControlProtocolError> {
    validate_common(
        CONTROL_SCHEMA_VERSION,
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    validate_response(&response.response)?;
    encode_json(response)
}

pub fn decode_response_v2(bytes: &[u8]) -> Result<ControlResponseEnvelope, ControlProtocolError> {
    ensure_bounded(bytes)?;
    ensure_version(peek_schema_version(bytes)?, CONTROL_SCHEMA_VERSION_V2)?;
    let response: ControlResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    validate_common(
        CONTROL_SCHEMA_VERSION_V2,
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    validate_response(&response.response)?;
    Ok(response)
}

pub fn encode_response_v2(
    response: &ControlResponseEnvelope,
) -> Result<Vec<u8>, ControlProtocolError> {
    validate_common(
        CONTROL_SCHEMA_VERSION_V2,
        response.schema_version,
        response.request_id,
        &response.operation_id,
    )?;
    validate_response(&response.response)?;
    encode_json(response)
}

fn validate_response(response: &ControlResponse) -> Result<(), ControlProtocolError> {
    if let ControlResponse::LeaseGranted { lease_id } = response
        && lease_id.iter().all(|byte| *byte == 0)
    {
        return Err(ControlProtocolError::InvalidControlResponse);
    }
    if let ControlResponse::MediaState { file, .. } = response
        && let Some(file) = file
        && (file.is_empty() || file.len() > 255 || file.contains('/'))
    {
        return Err(ControlProtocolError::InvalidControlResponse);
    }
    Ok(())
}

fn encode_json(value: &impl Serialize) -> Result<Vec<u8>, ControlProtocolError> {
    let bytes = serde_json::to_vec(value).map_err(|_| ControlProtocolError::InvalidJson)?;
    ensure_bounded(&bytes)?;
    Ok(bytes)
}

fn validate_common(
    expected_version: u16,
    schema_version: u16,
    request_id: u64,
    operation_id: &str,
) -> Result<(), ControlProtocolError> {
    if schema_version != expected_version {
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

fn peek_schema_version(bytes: &[u8]) -> Result<u16, ControlProtocolError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct VersionPeek {
        schema_version: u16,
    }
    let peek: VersionPeek =
        serde_json::from_slice(bytes).map_err(|_| ControlProtocolError::InvalidJson)?;
    Ok(peek.schema_version)
}

fn ensure_version(actual: u16, expected: u16) -> Result<(), ControlProtocolError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ControlProtocolError::UnsupportedSchemaVersion(actual))
    }
}

#[cfg(test)]
mod tests {
    use bli_kvm_core::{
        AtxConfirmation, AtxControlAction, HidLeaseRequest, HidReport, MediaObserveRequest,
        MediaRequest, SystemActionKind, SystemActionWireRequest, TargetAttestation,
        VirtualMediaCommand, VirtualMediaControlMode,
    };

    use super::*;

    const OPERATION_ID: &str = "0123456789abcdef0123456789abcdef";
    const TARGET: [u8; 32] = [0x11; 32];
    const LEASE: [u8; 16] = [0x33; 16];

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

    fn lease_report_v2() -> ControlRequestEnvelopeV2 {
        ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 21,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequestV2::HidLease {
                lease: HidLeaseRequest::Report {
                    lease_id: LEASE,
                    session_binding: 9,
                    target_binding: TARGET,
                    report: HidReport::Keyboard {
                        data: [0, 0, 4, 0, 0, 0, 0, 0],
                    },
                },
            },
        }
    }

    #[test]
    fn v2_lease_messages_roundtrip_without_generic_fields() {
        let request = lease_report_v2();
        let json = encode_request_v2(&request).unwrap();
        assert_eq!(decode_request_v2(&json).unwrap(), request);
        for forbidden in ["path", "url", "shell", "gpio", "duration", "credential"] {
            assert!(!String::from_utf8_lossy(&json).contains(forbidden));
        }
    }

    #[test]
    fn v1_and_v2_endpoints_reject_each_other() {
        let v1 = encode_request(&keyboard_request()).unwrap();
        assert_eq!(
            decode_request_v2(&v1),
            Err(ControlProtocolError::UnsupportedSchemaVersion(
                CONTROL_SCHEMA_VERSION
            ))
        );
        let v2 = encode_request_v2(&lease_report_v2()).unwrap();
        assert_eq!(
            decode_request(&v2),
            Err(ControlProtocolError::UnsupportedSchemaVersion(
                CONTROL_SCHEMA_VERSION_V2
            ))
        );
    }

    #[test]
    fn v2_rejects_unknown_fields_and_keeps_fixed_ttl_bounds() {
        let with_extra = br#"{"schemaVersion":2,"requestId":1,"operationId":"0123456789abcdef0123456789abcdef","request":{"type":"hid_lease","lease":{"op":"release","leaseId":[1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"sessionBinding":9,"targetBinding":[1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],"path":"/dev/hidg0"}}}"#;
        assert_eq!(
            decode_request_v2(with_extra),
            Err(ControlProtocolError::InvalidJson)
        );

        let mut attestation = lease_report_v2();
        attestation.request = PlatformControlRequestV2::TargetAttest {
            attestation: TargetAttestation {
                session_binding: 9,
                target_binding: TARGET,
                focus_binding: [0x22; 16],
                ttl_seconds: 61,
            },
        };
        assert_eq!(
            encode_request_v2(&attestation),
            Err(ControlProtocolError::InvalidControlRequest)
        );

        let mut long_lease = lease_report_v2();
        long_lease.request = PlatformControlRequestV2::HidLease {
            lease: HidLeaseRequest::Acquire {
                session_binding: 9,
                target_binding: TARGET,
                ttl_seconds: 31,
            },
        };
        assert_eq!(
            encode_request_v2(&long_lease),
            Err(ControlProtocolError::InvalidControlRequest)
        );
    }

    #[test]
    fn v2_lease_grant_carries_a_nonzero_lease_id() {
        let response = ControlResponseEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 30,
            operation_id: OPERATION_ID.into(),
            response: ControlResponse::LeaseGranted {
                lease_id: [0x55; 16],
            },
        };
        let json = encode_response_v2(&response).unwrap();
        assert_eq!(decode_response_v2(&json).unwrap(), response);
        assert!(!String::from_utf8_lossy(&json).contains("message"));

        let zero = ControlResponseEnvelope {
            response: ControlResponse::LeaseGranted { lease_id: [0; 16] },
            ..response
        };
        assert_eq!(
            encode_response_v2(&zero),
            Err(ControlProtocolError::InvalidControlResponse)
        );
    }

    #[test]
    fn v2_atx_confirmation_is_typed_and_responses_use_fixed_codes() {
        let request = ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 22,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequestV2::Atx {
                action: AtxControlAction::Reset,
                confirmation: AtxConfirmation {
                    session_binding: 9,
                    confirmation_binding: [0x44; 32],
                    confirmed_at_unix_ms: 1_700_000_000_000,
                },
            },
        };
        assert!(encode_request_v2(&request).is_ok());
        let response = ControlResponseEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: request.request_id,
            operation_id: request.operation_id,
            response: ControlResponse::Rejected {
                code: ControlRejectionCode::NotAttested,
            },
        };
        let json = encode_response_v2(&response).unwrap();
        assert_eq!(decode_response_v2(&json).unwrap(), response);
        assert!(!String::from_utf8_lossy(&json).contains("message"));
    }

    #[test]
    fn media_state_response_round_trips_and_rejects_unsafe_names() {
        let response = ControlResponseEnvelope {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 24,
            operation_id: OPERATION_ID.into(),
            response: ControlResponse::MediaState {
                file: Some("rescue.iso".into()),
                connected: true,
                cdrom: true,
                writable: false,
            },
        };
        let json = encode_response_v2(&response).unwrap();
        assert_eq!(decode_response_v2(&json).unwrap(), response);

        for file in ["", "../../etc/passwd", "a".repeat(256).as_str()] {
            let invalid = ControlResponseEnvelope {
                schema_version: CONTROL_SCHEMA_VERSION_V2,
                request_id: 25,
                operation_id: OPERATION_ID.into(),
                response: ControlResponse::MediaState {
                    file: Some(file.into()),
                    connected: false,
                    cdrom: false,
                    writable: false,
                },
            };
            assert!(encode_response_v2(&invalid).is_err(), "{file}");
        }
    }

    #[test]
    fn v2_system_action_request_round_trips() {
        let request = ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 27,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequestV2::SystemAction {
                request: SystemActionWireRequest {
                    action: SystemActionKind::Reboot,
                    session_binding: 9,
                    confirmation_binding: [0x11; 32],
                    confirmed_at_unix_ms: 1_700_000_000_000,
                },
            },
        };
        let json = encode_request_v2(&request).unwrap();
        assert_eq!(decode_request_v2(&json).unwrap(), request);
        let text = String::from_utf8_lossy(&json);
        assert!(text.contains("\"confirmedAtUnixMs\""));
        assert!(!text.contains("path"));
    }

    #[test]
    fn v2_virtual_media_observe_request_round_trips() {
        let request = ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 26,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequestV2::VirtualMediaObserve {
                request: MediaObserveRequest {
                    session_binding: 9,
                    target_binding: [0x11; 32],
                    issued_at_unix_ms: 1_700_000_000_000,
                },
            },
        };
        let json = encode_request_v2(&request).unwrap();
        assert_eq!(decode_request_v2(&json).unwrap(), request);
    }

    #[test]
    fn v2_virtual_media_request_round_trips() {
        let request = ControlRequestEnvelopeV2 {
            schema_version: CONTROL_SCHEMA_VERSION_V2,
            request_id: 23,
            operation_id: OPERATION_ID.into(),
            request: PlatformControlRequestV2::VirtualMedia {
                request: MediaRequest {
                    command: VirtualMediaCommand::Select {
                        image_id: 7,
                        mode: VirtualMediaControlMode::CdRom,
                        writable: false,
                    },
                    session_binding: 9,
                    target_binding: [0x11; 32],
                    issued_at_unix_ms: 1_700_000_000_000,
                },
            },
        };
        let json = encode_request_v2(&request).unwrap();
        assert_eq!(decode_request_v2(&json).unwrap(), request);
        let text = String::from_utf8_lossy(&json);
        assert!(text.contains("\"imageId\""));
        assert!(!text.contains("image_id"));
        assert!(!text.contains("path"));
    }
}
