#![no_std]
#![forbid(unsafe_code)]

//! Fixed producer-to-collector diagnostic event frames.
//!
//! A frame contains only a schema byte, a closed event tag, two typed argument
//! bytes, and four mandatory-zero reserved bytes. Source role, process
//! identity, lifecycle epoch, sequence, time, paths, text, and arbitrary bytes
//! are outside this protocol and must be assigned or rejected by the collector.
//! A producer must wait for one exact response before sending another event;
//! accepted, policy-rejected, backpressured, and unavailable outcomes therefore
//! cannot be confused. Invalid frames are connection-fatal and have no response.

use bli_kvm_diagnostic_export::MAX_RETRY_ATTEMPT;
pub use bli_kvm_diagnostic_export::{
    DiagnosticEvent, JournalHealth, OperationResult, PressureLevel, RejectReason, ResourceKind,
};

pub const DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION: u8 = 1;
pub const DIAGNOSTIC_EVENT_FRAME_BYTES: usize = 8;
pub const DIAGNOSTIC_RESPONSE_FRAME_BYTES: usize = 4;

const TAG_READY: u8 = 1;
const TAG_UNAVAILABLE: u8 = 2;
const TAG_REQUEST_REJECTED: u8 = 3;
const TAG_RETRY_SCHEDULED: u8 = 4;
const TAG_OPERATION_FINISHED: u8 = 5;
const TAG_RESOURCE_PRESSURE: u8 = 6;
const TAG_JOURNAL_OBSERVED: u8 = 7;
const RESPONSE_ACCEPTED: u8 = 1;
const RESPONSE_REJECTED: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticProtocolError {
    InvalidLength,
    UnsupportedSchemaVersion(u8),
    UnknownEventTag(u8),
    UnknownResponseTag(u8),
    InvalidPayload,
    NonCanonicalReserved,
    LifecycleEventForbidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticProducerRejection {
    EventNotAllowed,
    Backpressure,
    CollectorUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DiagnosticProducerResponse {
    Accepted,
    Rejected { reason: DiagnosticProducerRejection },
}

pub fn encode_event(
    event: DiagnosticEvent,
) -> Result<[u8; DIAGNOSTIC_EVENT_FRAME_BYTES], DiagnosticProtocolError> {
    let (tag, argument_a, argument_b) = match event {
        DiagnosticEvent::Started | DiagnosticEvent::Stopped => {
            return Err(DiagnosticProtocolError::LifecycleEventForbidden);
        }
        DiagnosticEvent::Ready => (TAG_READY, 0, 0),
        DiagnosticEvent::Unavailable => (TAG_UNAVAILABLE, 0, 0),
        DiagnosticEvent::RequestRejected { reason } => {
            (TAG_REQUEST_REJECTED, encode_reject_reason(reason), 0)
        }
        DiagnosticEvent::RetryScheduled { attempt }
            if attempt != 0 && attempt <= MAX_RETRY_ATTEMPT =>
        {
            (TAG_RETRY_SCHEDULED, attempt, 0)
        }
        DiagnosticEvent::RetryScheduled { .. } => {
            return Err(DiagnosticProtocolError::InvalidPayload);
        }
        DiagnosticEvent::OperationFinished { result } => {
            (TAG_OPERATION_FINISHED, encode_operation_result(result), 0)
        }
        DiagnosticEvent::ResourcePressure { resource, level } => (
            TAG_RESOURCE_PRESSURE,
            encode_resource_kind(resource),
            encode_pressure_level(level),
        ),
        DiagnosticEvent::JournalObserved { health } => {
            (TAG_JOURNAL_OBSERVED, encode_journal_health(health), 0)
        }
    };
    Ok([
        DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION,
        tag,
        argument_a,
        argument_b,
        0,
        0,
        0,
        0,
    ])
}

pub fn decode_event(bytes: &[u8]) -> Result<DiagnosticEvent, DiagnosticProtocolError> {
    if bytes.len() != DIAGNOSTIC_EVENT_FRAME_BYTES {
        return Err(DiagnosticProtocolError::InvalidLength);
    }
    if bytes[0] != DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION {
        return Err(DiagnosticProtocolError::UnsupportedSchemaVersion(bytes[0]));
    }
    if bytes[4..].iter().any(|byte| *byte != 0) {
        return Err(DiagnosticProtocolError::NonCanonicalReserved);
    }
    let argument_a = bytes[2];
    let argument_b = bytes[3];
    match bytes[1] {
        TAG_READY if argument_a == 0 && argument_b == 0 => Ok(DiagnosticEvent::Ready),
        TAG_UNAVAILABLE if argument_a == 0 && argument_b == 0 => Ok(DiagnosticEvent::Unavailable),
        TAG_REQUEST_REJECTED if argument_b == 0 => Ok(DiagnosticEvent::RequestRejected {
            reason: decode_reject_reason(argument_a)?,
        }),
        TAG_RETRY_SCHEDULED
            if argument_a != 0 && argument_a <= MAX_RETRY_ATTEMPT && argument_b == 0 =>
        {
            Ok(DiagnosticEvent::RetryScheduled {
                attempt: argument_a,
            })
        }
        TAG_OPERATION_FINISHED if argument_b == 0 => Ok(DiagnosticEvent::OperationFinished {
            result: decode_operation_result(argument_a)?,
        }),
        TAG_RESOURCE_PRESSURE => Ok(DiagnosticEvent::ResourcePressure {
            resource: decode_resource_kind(argument_a)?,
            level: decode_pressure_level(argument_b)?,
        }),
        TAG_JOURNAL_OBSERVED if argument_b == 0 => Ok(DiagnosticEvent::JournalObserved {
            health: decode_journal_health(argument_a)?,
        }),
        TAG_READY..=TAG_JOURNAL_OBSERVED => Err(DiagnosticProtocolError::InvalidPayload),
        tag => Err(DiagnosticProtocolError::UnknownEventTag(tag)),
    }
}

pub fn encode_response(
    response: DiagnosticProducerResponse,
) -> [u8; DIAGNOSTIC_RESPONSE_FRAME_BYTES] {
    let (tag, argument) = match response {
        DiagnosticProducerResponse::Accepted => (RESPONSE_ACCEPTED, 0),
        DiagnosticProducerResponse::Rejected { reason } => {
            (RESPONSE_REJECTED, encode_rejection(reason))
        }
    };
    [DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION, tag, argument, 0]
}

pub fn decode_response(
    bytes: &[u8],
) -> Result<DiagnosticProducerResponse, DiagnosticProtocolError> {
    if bytes.len() != DIAGNOSTIC_RESPONSE_FRAME_BYTES {
        return Err(DiagnosticProtocolError::InvalidLength);
    }
    if bytes[0] != DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION {
        return Err(DiagnosticProtocolError::UnsupportedSchemaVersion(bytes[0]));
    }
    if bytes[3] != 0 {
        return Err(DiagnosticProtocolError::NonCanonicalReserved);
    }
    match bytes[1] {
        RESPONSE_ACCEPTED if bytes[2] == 0 => Ok(DiagnosticProducerResponse::Accepted),
        RESPONSE_ACCEPTED => Err(DiagnosticProtocolError::InvalidPayload),
        RESPONSE_REJECTED => Ok(DiagnosticProducerResponse::Rejected {
            reason: decode_rejection(bytes[2])?,
        }),
        tag => Err(DiagnosticProtocolError::UnknownResponseTag(tag)),
    }
}

const fn encode_rejection(reason: DiagnosticProducerRejection) -> u8 {
    match reason {
        DiagnosticProducerRejection::EventNotAllowed => 1,
        DiagnosticProducerRejection::Backpressure => 2,
        DiagnosticProducerRejection::CollectorUnavailable => 3,
    }
}

const fn decode_rejection(
    value: u8,
) -> Result<DiagnosticProducerRejection, DiagnosticProtocolError> {
    match value {
        1 => Ok(DiagnosticProducerRejection::EventNotAllowed),
        2 => Ok(DiagnosticProducerRejection::Backpressure),
        3 => Ok(DiagnosticProducerRejection::CollectorUnavailable),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

const fn encode_reject_reason(reason: RejectReason) -> u8 {
    match reason {
        RejectReason::Unauthorized => 1,
        RejectReason::CapabilityDisabled => 2,
        RejectReason::BackendUnavailable => 3,
        RejectReason::Busy => 4,
        RejectReason::InvalidInput => 5,
        RejectReason::RateLimited => 6,
        RejectReason::Conflict => 7,
    }
}

const fn decode_reject_reason(value: u8) -> Result<RejectReason, DiagnosticProtocolError> {
    match value {
        1 => Ok(RejectReason::Unauthorized),
        2 => Ok(RejectReason::CapabilityDisabled),
        3 => Ok(RejectReason::BackendUnavailable),
        4 => Ok(RejectReason::Busy),
        5 => Ok(RejectReason::InvalidInput),
        6 => Ok(RejectReason::RateLimited),
        7 => Ok(RejectReason::Conflict),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

const fn encode_operation_result(result: OperationResult) -> u8 {
    match result {
        OperationResult::Applied => 1,
        OperationResult::Rejected => 2,
        OperationResult::Noop => 3,
        OperationResult::RolledBack => 4,
    }
}

const fn decode_operation_result(value: u8) -> Result<OperationResult, DiagnosticProtocolError> {
    match value {
        1 => Ok(OperationResult::Applied),
        2 => Ok(OperationResult::Rejected),
        3 => Ok(OperationResult::Noop),
        4 => Ok(OperationResult::RolledBack),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

const fn encode_resource_kind(resource: ResourceKind) -> u8 {
    match resource {
        ResourceKind::Memory => 1,
        ResourceKind::Storage => 2,
        ResourceKind::Temperature => 3,
        ResourceKind::Sessions => 4,
    }
}

const fn decode_resource_kind(value: u8) -> Result<ResourceKind, DiagnosticProtocolError> {
    match value {
        1 => Ok(ResourceKind::Memory),
        2 => Ok(ResourceKind::Storage),
        3 => Ok(ResourceKind::Temperature),
        4 => Ok(ResourceKind::Sessions),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

const fn encode_pressure_level(level: PressureLevel) -> u8 {
    match level {
        PressureLevel::Elevated => 1,
        PressureLevel::Critical => 2,
    }
}

const fn decode_pressure_level(value: u8) -> Result<PressureLevel, DiagnosticProtocolError> {
    match value {
        1 => Ok(PressureLevel::Elevated),
        2 => Ok(PressureLevel::Critical),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

const fn encode_journal_health(health: JournalHealth) -> u8 {
    match health {
        JournalHealth::Healthy => 1,
        JournalHealth::Degraded => 2,
        JournalHealth::RecoveryRequired => 3,
    }
}

const fn decode_journal_health(value: u8) -> Result<JournalHealth, DiagnosticProtocolError> {
    match value {
        1 => Ok(JournalHealth::Healthy),
        2 => Ok(JournalHealth::Degraded),
        3 => Ok(JournalHealth::RecoveryRequired),
        _ => Err(DiagnosticProtocolError::InvalidPayload),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_producer_owned_variant_roundtrips_canonically() {
        let events = [
            DiagnosticEvent::Ready,
            DiagnosticEvent::Unavailable,
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::Unauthorized,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::CapabilityDisabled,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::BackendUnavailable,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::Busy,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::InvalidInput,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::RateLimited,
            },
            DiagnosticEvent::RequestRejected {
                reason: RejectReason::Conflict,
            },
            DiagnosticEvent::RetryScheduled { attempt: 1 },
            DiagnosticEvent::RetryScheduled {
                attempt: MAX_RETRY_ATTEMPT,
            },
            DiagnosticEvent::OperationFinished {
                result: OperationResult::Applied,
            },
            DiagnosticEvent::OperationFinished {
                result: OperationResult::Rejected,
            },
            DiagnosticEvent::OperationFinished {
                result: OperationResult::Noop,
            },
            DiagnosticEvent::OperationFinished {
                result: OperationResult::RolledBack,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Memory,
                level: PressureLevel::Elevated,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Memory,
                level: PressureLevel::Critical,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Storage,
                level: PressureLevel::Elevated,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Storage,
                level: PressureLevel::Critical,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Temperature,
                level: PressureLevel::Elevated,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Temperature,
                level: PressureLevel::Critical,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Sessions,
                level: PressureLevel::Elevated,
            },
            DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Sessions,
                level: PressureLevel::Critical,
            },
            DiagnosticEvent::JournalObserved {
                health: JournalHealth::Healthy,
            },
            DiagnosticEvent::JournalObserved {
                health: JournalHealth::Degraded,
            },
            DiagnosticEvent::JournalObserved {
                health: JournalHealth::RecoveryRequired,
            },
        ];
        for event in events {
            let frame = encode_event(event).unwrap();
            assert_eq!(frame.len(), DIAGNOSTIC_EVENT_FRAME_BYTES);
            assert_eq!(frame[0], DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION);
            assert!(frame[4..].iter().all(|byte| *byte == 0));
            assert_eq!(decode_event(&frame), Ok(event));
            assert_eq!(encode_event(decode_event(&frame).unwrap()).unwrap(), frame);
        }
    }

    #[test]
    fn exact_frames_have_no_source_lifecycle_sequence_or_time() {
        assert_eq!(
            encode_event(DiagnosticEvent::Ready),
            Ok([1, TAG_READY, 0, 0, 0, 0, 0, 0])
        );
        assert_eq!(
            encode_event(DiagnosticEvent::RequestRejected {
                reason: RejectReason::RateLimited,
            }),
            Ok([1, TAG_REQUEST_REJECTED, 6, 0, 0, 0, 0, 0])
        );
        assert_eq!(
            encode_event(DiagnosticEvent::ResourcePressure {
                resource: ResourceKind::Temperature,
                level: PressureLevel::Critical,
            }),
            Ok([1, TAG_RESOURCE_PRESSURE, 3, 2, 0, 0, 0, 0])
        );
    }

    #[test]
    fn producer_cannot_emit_collector_owned_lifecycle() {
        assert_eq!(
            encode_event(DiagnosticEvent::Started),
            Err(DiagnosticProtocolError::LifecycleEventForbidden)
        );
        assert_eq!(
            encode_event(DiagnosticEvent::Stopped),
            Err(DiagnosticProtocolError::LifecycleEventForbidden)
        );
    }

    #[test]
    fn length_schema_and_reserved_bytes_are_exact() {
        let valid = encode_event(DiagnosticEvent::Ready).unwrap();
        let mut lengths = [0_u8; 33];
        lengths[..DIAGNOSTIC_EVENT_FRAME_BYTES].copy_from_slice(&valid);
        for length in 0..=lengths.len() {
            if length == DIAGNOSTIC_EVENT_FRAME_BYTES {
                assert_eq!(decode_event(&lengths[..length]), Ok(DiagnosticEvent::Ready));
            } else {
                assert_eq!(
                    decode_event(&lengths[..length]),
                    Err(DiagnosticProtocolError::InvalidLength)
                );
            }
        }
        for schema in 0..=u8::MAX {
            if schema == DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION {
                continue;
            }
            let mut wrong_schema = valid;
            wrong_schema[0] = schema;
            assert_eq!(
                decode_event(&wrong_schema),
                Err(DiagnosticProtocolError::UnsupportedSchemaVersion(schema))
            );
        }
        for index in 4..DIAGNOSTIC_EVENT_FRAME_BYTES {
            for value in 1..=u8::MAX {
                let mut noncanonical = valid;
                noncanonical[index] = value;
                assert_eq!(
                    decode_event(&noncanonical),
                    Err(DiagnosticProtocolError::NonCanonicalReserved)
                );
            }
        }
    }

    #[test]
    fn every_unknown_event_tag_is_rejected() {
        for tag in 0..=u8::MAX {
            if (TAG_READY..=TAG_JOURNAL_OBSERVED).contains(&tag) {
                continue;
            }
            assert_eq!(
                decode_event(&[1, tag, 0, 0, 0, 0, 0, 0]),
                Err(DiagnosticProtocolError::UnknownEventTag(tag))
            );
        }
    }

    #[test]
    fn every_payload_byte_pair_has_a_closed_domain() {
        for tag in TAG_READY..=TAG_JOURNAL_OBSERVED {
            for argument_a in 0..=u8::MAX {
                for argument_b in 0..=u8::MAX {
                    let expected_valid = match tag {
                        TAG_READY | TAG_UNAVAILABLE => argument_a == 0 && argument_b == 0,
                        TAG_REQUEST_REJECTED => (1..=7).contains(&argument_a) && argument_b == 0,
                        TAG_RETRY_SCHEDULED => {
                            (1..=MAX_RETRY_ATTEMPT).contains(&argument_a) && argument_b == 0
                        }
                        TAG_OPERATION_FINISHED => (1..=4).contains(&argument_a) && argument_b == 0,
                        TAG_RESOURCE_PRESSURE => {
                            (1..=4).contains(&argument_a) && (1..=2).contains(&argument_b)
                        }
                        TAG_JOURNAL_OBSERVED => (1..=3).contains(&argument_a) && argument_b == 0,
                        _ => unreachable!(),
                    };
                    let decoded = decode_event(&[
                        DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION,
                        tag,
                        argument_a,
                        argument_b,
                        0,
                        0,
                        0,
                        0,
                    ]);
                    assert_eq!(decoded.is_ok(), expected_valid);
                    if !expected_valid {
                        assert_eq!(decoded, Err(DiagnosticProtocolError::InvalidPayload));
                    }
                }
            }
        }
    }

    #[test]
    fn unused_or_invalid_payload_values_are_rejected() {
        for tag in [TAG_READY, TAG_UNAVAILABLE] {
            for (argument_a, argument_b) in [(1, 0), (0, 1), (1, 1)] {
                assert_eq!(
                    decode_event(&[1, tag, argument_a, argument_b, 0, 0, 0, 0]),
                    Err(DiagnosticProtocolError::InvalidPayload)
                );
            }
        }
        for frame in [
            [1, TAG_REQUEST_REJECTED, 0, 0, 0, 0, 0, 0],
            [1, TAG_REQUEST_REJECTED, 8, 0, 0, 0, 0, 0],
            [1, TAG_REQUEST_REJECTED, 1, 1, 0, 0, 0, 0],
            [1, TAG_RETRY_SCHEDULED, 0, 0, 0, 0, 0, 0],
            [1, TAG_RETRY_SCHEDULED, 17, 0, 0, 0, 0, 0],
            [1, TAG_RETRY_SCHEDULED, 1, 1, 0, 0, 0, 0],
            [1, TAG_OPERATION_FINISHED, 0, 0, 0, 0, 0, 0],
            [1, TAG_OPERATION_FINISHED, 5, 0, 0, 0, 0, 0],
            [1, TAG_OPERATION_FINISHED, 1, 1, 0, 0, 0, 0],
            [1, TAG_RESOURCE_PRESSURE, 0, 1, 0, 0, 0, 0],
            [1, TAG_RESOURCE_PRESSURE, 5, 1, 0, 0, 0, 0],
            [1, TAG_RESOURCE_PRESSURE, 1, 0, 0, 0, 0, 0],
            [1, TAG_RESOURCE_PRESSURE, 1, 3, 0, 0, 0, 0],
            [1, TAG_JOURNAL_OBSERVED, 0, 0, 0, 0, 0, 0],
            [1, TAG_JOURNAL_OBSERVED, 4, 0, 0, 0, 0, 0],
            [1, TAG_JOURNAL_OBSERVED, 1, 1, 0, 0, 0, 0],
        ] {
            assert_eq!(
                decode_event(&frame),
                Err(DiagnosticProtocolError::InvalidPayload)
            );
        }
        assert_eq!(
            encode_event(DiagnosticEvent::RetryScheduled { attempt: 0 }),
            Err(DiagnosticProtocolError::InvalidPayload)
        );
        assert_eq!(
            encode_event(DiagnosticEvent::RetryScheduled { attempt: 17 }),
            Err(DiagnosticProtocolError::InvalidPayload)
        );
    }

    #[test]
    fn decoding_and_reencoding_are_deterministic() {
        let frame = [1, TAG_JOURNAL_OBSERVED, 3, 0, 0, 0, 0, 0];
        let event = decode_event(&frame).unwrap();
        assert_eq!(
            event,
            DiagnosticEvent::JournalObserved {
                health: JournalHealth::RecoveryRequired
            }
        );
        assert_eq!(encode_event(event), Ok(frame));
    }

    #[test]
    fn every_response_roundtrips_with_exact_acknowledgement_bytes() {
        let cases = [
            (DiagnosticProducerResponse::Accepted, [1, 1, 0, 0]),
            (
                DiagnosticProducerResponse::Rejected {
                    reason: DiagnosticProducerRejection::EventNotAllowed,
                },
                [1, 2, 1, 0],
            ),
            (
                DiagnosticProducerResponse::Rejected {
                    reason: DiagnosticProducerRejection::Backpressure,
                },
                [1, 2, 2, 0],
            ),
            (
                DiagnosticProducerResponse::Rejected {
                    reason: DiagnosticProducerRejection::CollectorUnavailable,
                },
                [1, 2, 3, 0],
            ),
        ];
        for (response, frame) in cases {
            assert_eq!(encode_response(response), frame);
            assert_eq!(decode_response(&frame), Ok(response));
        }
    }

    #[test]
    fn every_response_tag_and_argument_pair_has_a_closed_domain() {
        for tag in 0..=u8::MAX {
            for argument in 0..=u8::MAX {
                let decoded =
                    decode_response(&[DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION, tag, argument, 0]);
                let expected_valid = (tag == RESPONSE_ACCEPTED && argument == 0)
                    || (tag == RESPONSE_REJECTED && (1..=3).contains(&argument));
                assert_eq!(decoded.is_ok(), expected_valid);
                if !expected_valid {
                    let expected = if matches!(tag, RESPONSE_ACCEPTED | RESPONSE_REJECTED) {
                        DiagnosticProtocolError::InvalidPayload
                    } else {
                        DiagnosticProtocolError::UnknownResponseTag(tag)
                    };
                    assert_eq!(decoded, Err(expected));
                }
            }
        }
    }

    #[test]
    fn response_length_schema_and_reserved_byte_are_exact() {
        let valid = encode_response(DiagnosticProducerResponse::Accepted);
        let mut lengths = [0_u8; 17];
        lengths[..DIAGNOSTIC_RESPONSE_FRAME_BYTES].copy_from_slice(&valid);
        for length in 0..=lengths.len() {
            if length == DIAGNOSTIC_RESPONSE_FRAME_BYTES {
                assert_eq!(
                    decode_response(&lengths[..length]),
                    Ok(DiagnosticProducerResponse::Accepted)
                );
            } else {
                assert_eq!(
                    decode_response(&lengths[..length]),
                    Err(DiagnosticProtocolError::InvalidLength)
                );
            }
        }
        for schema in 0..=u8::MAX {
            if schema == DIAGNOSTIC_PROTOCOL_SCHEMA_VERSION {
                continue;
            }
            let mut frame = valid;
            frame[0] = schema;
            assert_eq!(
                decode_response(&frame),
                Err(DiagnosticProtocolError::UnsupportedSchemaVersion(schema))
            );
        }
        for reserved in 1..=u8::MAX {
            let mut frame = valid;
            frame[3] = reserved;
            assert_eq!(
                decode_response(&frame),
                Err(DiagnosticProtocolError::NonCanonicalReserved)
            );
        }
    }
}
