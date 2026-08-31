#![no_std]
#![forbid(unsafe_code)]

//! Fixed request/response framing for one authenticated sealed-window consumer.
//!
//! Requests contain only an operation and an opaque collector-instance/window
//! binding. Sealed responses can be constructed only from the typed bounded
//! diagnostic bundle. The protocol has no path, identity, clock, arbitrary
//! query, raw-log, allocation, I/O, or retry surface.

use bli_kvm_diagnostic_export::{DiagnosticBundle, MAX_EXPORT_BYTES, MAX_RECORDS};

pub const DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION: u8 = 1;
pub const DIAGNOSTIC_EXPORT_REQUEST_BYTES: usize = 16;
pub const DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES: usize = 24;
pub const MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES: usize =
    DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES + MAX_EXPORT_BYTES;

const REQUEST_READ_SEALED: u8 = 1;
const REQUEST_ACKNOWLEDGE: u8 = 2;
const RESPONSE_NO_SEALED: u8 = 1;
const RESPONSE_SEALED: u8 = 2;
const RESPONSE_ACKNOWLEDGED: u8 = 3;
const RESPONSE_ACKNOWLEDGEMENT_MISMATCH: u8 = 4;
const RESPONSE_COLLECTOR_UNAVAILABLE: u8 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportProtocolError {
    InvalidLength,
    UnsupportedSchemaVersion(u8),
    UnknownRequest(u8),
    UnknownResponse(u8),
    NonCanonicalReserved,
    InvalidBinding,
    InvalidPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportRequest {
    ReadSealed,
    Acknowledge { instance_id: u64, window_id: u32 },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticExportResponse<'a> {
    NoSealed,
    Sealed {
        instance_id: u64,
        window_id: u32,
        omitted_lifecycle_events: u8,
        bundle: &'a DiagnosticBundle,
    },
    Acknowledged {
        instance_id: u64,
        window_id: u32,
    },
    AcknowledgementMismatch {
        instance_id: u64,
        window_id: u32,
    },
    CollectorUnavailable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DecodedDiagnosticExportResponse<'a> {
    NoSealed,
    Sealed {
        instance_id: u64,
        window_id: u32,
        omitted_lifecycle_events: u8,
        record_count: u8,
        payload: &'a [u8],
    },
    Acknowledged {
        instance_id: u64,
        window_id: u32,
    },
    AcknowledgementMismatch {
        instance_id: u64,
        window_id: u32,
    },
    CollectorUnavailable,
}

pub struct DiagnosticExportResponseFrame {
    bytes: [u8; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES],
    length: usize,
}

impl DiagnosticExportResponseFrame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    pub const fn length(&self) -> usize {
        self.length
    }
}

pub const fn encode_request(
    request: DiagnosticExportRequest,
) -> Result<[u8; DIAGNOSTIC_EXPORT_REQUEST_BYTES], DiagnosticExportProtocolError> {
    let (tag, instance_id, window_id) = match request {
        DiagnosticExportRequest::ReadSealed => (REQUEST_READ_SEALED, 0, 0),
        DiagnosticExportRequest::Acknowledge {
            instance_id,
            window_id,
        } if instance_id != 0 && window_id != 0 => (REQUEST_ACKNOWLEDGE, instance_id, window_id),
        DiagnosticExportRequest::Acknowledge { .. } => {
            return Err(DiagnosticExportProtocolError::InvalidBinding);
        }
    };
    let instance = instance_id.to_be_bytes();
    let window = window_id.to_be_bytes();
    Ok([
        DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION,
        tag,
        0,
        0,
        instance[0],
        instance[1],
        instance[2],
        instance[3],
        instance[4],
        instance[5],
        instance[6],
        instance[7],
        window[0],
        window[1],
        window[2],
        window[3],
    ])
}

pub fn decode_request(
    bytes: &[u8],
) -> Result<DiagnosticExportRequest, DiagnosticExportProtocolError> {
    if bytes.len() != DIAGNOSTIC_EXPORT_REQUEST_BYTES {
        return Err(DiagnosticExportProtocolError::InvalidLength);
    }
    if bytes[0] != DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION {
        return Err(DiagnosticExportProtocolError::UnsupportedSchemaVersion(
            bytes[0],
        ));
    }
    if bytes[2..4] != [0, 0] {
        return Err(DiagnosticExportProtocolError::NonCanonicalReserved);
    }
    let instance_id = u64::from_be_bytes(
        bytes[4..12]
            .try_into()
            .map_err(|_| DiagnosticExportProtocolError::InvalidLength)?,
    );
    let window_id = u32::from_be_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| DiagnosticExportProtocolError::InvalidLength)?,
    );
    match bytes[1] {
        REQUEST_READ_SEALED if instance_id == 0 && window_id == 0 => {
            Ok(DiagnosticExportRequest::ReadSealed)
        }
        REQUEST_ACKNOWLEDGE if instance_id != 0 && window_id != 0 => {
            Ok(DiagnosticExportRequest::Acknowledge {
                instance_id,
                window_id,
            })
        }
        REQUEST_READ_SEALED | REQUEST_ACKNOWLEDGE => {
            Err(DiagnosticExportProtocolError::InvalidBinding)
        }
        tag => Err(DiagnosticExportProtocolError::UnknownRequest(tag)),
    }
}

pub fn encode_response(
    response: DiagnosticExportResponse<'_>,
) -> Result<DiagnosticExportResponseFrame, DiagnosticExportProtocolError> {
    let mut frame = DiagnosticExportResponseFrame {
        bytes: [0; MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES],
        length: DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES,
    };
    frame.bytes[0] = DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION;
    let (tag, instance_id, window_id, omitted, record_count, payload) = match response {
        DiagnosticExportResponse::NoSealed => (RESPONSE_NO_SEALED, 0, 0, 0, 0, &[][..]),
        DiagnosticExportResponse::Sealed {
            instance_id,
            window_id,
            omitted_lifecycle_events,
            bundle,
        } if instance_id != 0 && window_id != 0 => {
            let count = u8::try_from(bundle.record_count())
                .map_err(|_| DiagnosticExportProtocolError::InvalidPayload)?;
            if count == 0 {
                return Err(DiagnosticExportProtocolError::InvalidPayload);
            }
            (
                RESPONSE_SEALED,
                instance_id,
                window_id,
                omitted_lifecycle_events,
                count,
                bundle.as_bytes(),
            )
        }
        DiagnosticExportResponse::Sealed { .. } => {
            return Err(DiagnosticExportProtocolError::InvalidBinding);
        }
        DiagnosticExportResponse::Acknowledged {
            instance_id,
            window_id,
        } if instance_id != 0 && window_id != 0 => {
            (RESPONSE_ACKNOWLEDGED, instance_id, window_id, 0, 0, &[][..])
        }
        DiagnosticExportResponse::AcknowledgementMismatch {
            instance_id,
            window_id,
        } if instance_id != 0 && window_id != 0 => (
            RESPONSE_ACKNOWLEDGEMENT_MISMATCH,
            instance_id,
            window_id,
            0,
            0,
            &[][..],
        ),
        DiagnosticExportResponse::Acknowledged { .. }
        | DiagnosticExportResponse::AcknowledgementMismatch { .. } => {
            return Err(DiagnosticExportProtocolError::InvalidBinding);
        }
        DiagnosticExportResponse::CollectorUnavailable => {
            (RESPONSE_COLLECTOR_UNAVAILABLE, 0, 0, 0, 0, &[][..])
        }
    };
    if payload.len() > MAX_EXPORT_BYTES || usize::from(record_count) > MAX_RECORDS {
        return Err(DiagnosticExportProtocolError::InvalidPayload);
    }
    let payload_length =
        u16::try_from(payload.len()).map_err(|_| DiagnosticExportProtocolError::InvalidPayload)?;
    frame.bytes[1] = tag;
    frame.bytes[2] = omitted;
    frame.bytes[3] = record_count;
    frame.bytes[4..12].copy_from_slice(&instance_id.to_be_bytes());
    frame.bytes[12..16].copy_from_slice(&window_id.to_be_bytes());
    frame.bytes[16..18].copy_from_slice(&payload_length.to_be_bytes());
    frame.bytes[DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES..][..payload.len()]
        .copy_from_slice(payload);
    frame.length += payload.len();
    Ok(frame)
}

pub fn decode_response(
    bytes: &[u8],
) -> Result<DecodedDiagnosticExportResponse<'_>, DiagnosticExportProtocolError> {
    if bytes.len() < DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES
        || bytes.len() > MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES
    {
        return Err(DiagnosticExportProtocolError::InvalidLength);
    }
    if bytes[0] != DIAGNOSTIC_EXPORT_PROTOCOL_SCHEMA_VERSION {
        return Err(DiagnosticExportProtocolError::UnsupportedSchemaVersion(
            bytes[0],
        ));
    }
    if bytes[18..DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES]
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(DiagnosticExportProtocolError::NonCanonicalReserved);
    }
    let instance_id = u64::from_be_bytes(
        bytes[4..12]
            .try_into()
            .map_err(|_| DiagnosticExportProtocolError::InvalidLength)?,
    );
    let window_id = u32::from_be_bytes(
        bytes[12..16]
            .try_into()
            .map_err(|_| DiagnosticExportProtocolError::InvalidLength)?,
    );
    let payload_length = usize::from(u16::from_be_bytes(
        bytes[16..18]
            .try_into()
            .map_err(|_| DiagnosticExportProtocolError::InvalidLength)?,
    ));
    if payload_length > MAX_EXPORT_BYTES
        || bytes.len() != DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES + payload_length
        || usize::from(bytes[3]) > MAX_RECORDS
    {
        return Err(DiagnosticExportProtocolError::InvalidPayload);
    }
    let payload = &bytes[DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES..];
    match bytes[1] {
        RESPONSE_NO_SEALED
            if canonical_empty(instance_id, window_id, bytes[2], bytes[3], payload) =>
        {
            Ok(DecodedDiagnosticExportResponse::NoSealed)
        }
        RESPONSE_SEALED
            if instance_id != 0 && window_id != 0 && bytes[3] != 0 && !payload.is_empty() =>
        {
            Ok(DecodedDiagnosticExportResponse::Sealed {
                instance_id,
                window_id,
                omitted_lifecycle_events: bytes[2],
                record_count: bytes[3],
                payload,
            })
        }
        RESPONSE_ACKNOWLEDGED
            if canonical_binding(instance_id, window_id, bytes[2], bytes[3], payload) =>
        {
            Ok(DecodedDiagnosticExportResponse::Acknowledged {
                instance_id,
                window_id,
            })
        }
        RESPONSE_ACKNOWLEDGEMENT_MISMATCH
            if canonical_binding(instance_id, window_id, bytes[2], bytes[3], payload) =>
        {
            Ok(DecodedDiagnosticExportResponse::AcknowledgementMismatch {
                instance_id,
                window_id,
            })
        }
        RESPONSE_COLLECTOR_UNAVAILABLE
            if canonical_empty(instance_id, window_id, bytes[2], bytes[3], payload) =>
        {
            Ok(DecodedDiagnosticExportResponse::CollectorUnavailable)
        }
        RESPONSE_NO_SEALED..=RESPONSE_COLLECTOR_UNAVAILABLE => {
            Err(DiagnosticExportProtocolError::InvalidPayload)
        }
        tag => Err(DiagnosticExportProtocolError::UnknownResponse(tag)),
    }
}

fn canonical_empty(
    instance_id: u64,
    window_id: u32,
    omitted: u8,
    record_count: u8,
    payload: &[u8],
) -> bool {
    instance_id == 0 && window_id == 0 && omitted == 0 && record_count == 0 && payload.is_empty()
}

fn canonical_binding(
    instance_id: u64,
    window_id: u32,
    omitted: u8,
    record_count: u8,
    payload: &[u8],
) -> bool {
    instance_id != 0 && window_id != 0 && omitted == 0 && record_count == 0 && payload.is_empty()
}

#[cfg(test)]
mod tests {
    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };

    use super::*;

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
    fn requests_are_exact_and_instance_bound() {
        assert_eq!(MAX_DIAGNOSTIC_EXPORT_RESPONSE_BYTES, 8_216);
        let read = encode_request(DiagnosticExportRequest::ReadSealed).unwrap();
        assert_eq!(read, [1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            decode_request(&read),
            Ok(DiagnosticExportRequest::ReadSealed)
        );

        let acknowledgement = DiagnosticExportRequest::Acknowledge {
            instance_id: 7,
            window_id: 3,
        };
        let encoded = encode_request(acknowledgement).unwrap();
        assert_eq!(decode_request(&encoded), Ok(acknowledgement));

        for invalid in [
            DiagnosticExportRequest::Acknowledge {
                instance_id: 0,
                window_id: 3,
            },
            DiagnosticExportRequest::Acknowledge {
                instance_id: 7,
                window_id: 0,
            },
        ] {
            assert_eq!(
                encode_request(invalid),
                Err(DiagnosticExportProtocolError::InvalidBinding)
            );
        }
    }

    #[test]
    fn sealed_response_borrows_only_the_typed_bundle() {
        let bundle = bundle();
        let frame = encode_response(DiagnosticExportResponse::Sealed {
            instance_id: 7,
            window_id: 3,
            omitted_lifecycle_events: 2,
            bundle: &bundle,
        })
        .unwrap();
        assert_eq!(
            frame.length(),
            DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES + bundle.length()
        );
        match decode_response(frame.as_bytes()).unwrap() {
            DecodedDiagnosticExportResponse::Sealed {
                instance_id,
                window_id,
                omitted_lifecycle_events,
                record_count,
                payload,
            } => {
                assert_eq!(instance_id, 7);
                assert_eq!(window_id, 3);
                assert_eq!(omitted_lifecycle_events, 2);
                assert_eq!(record_count, 1);
                assert_eq!(payload, bundle.as_bytes());
            }
            _ => panic!("unexpected response"),
        }
    }

    #[test]
    fn every_fixed_response_is_canonical() {
        let responses = [
            DiagnosticExportResponse::NoSealed,
            DiagnosticExportResponse::Acknowledged {
                instance_id: 7,
                window_id: 3,
            },
            DiagnosticExportResponse::AcknowledgementMismatch {
                instance_id: 7,
                window_id: 3,
            },
            DiagnosticExportResponse::CollectorUnavailable,
        ];
        for response in responses {
            let frame = encode_response(response).unwrap();
            assert_eq!(frame.length(), DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES);
            assert!(decode_response(frame.as_bytes()).is_ok());
        }
    }

    #[test]
    fn malformed_frames_fail_closed() {
        let mut request = encode_request(DiagnosticExportRequest::ReadSealed).unwrap();
        request[2] = 1;
        assert_eq!(
            decode_request(&request),
            Err(DiagnosticExportProtocolError::NonCanonicalReserved)
        );
        request[2] = 0;
        request[1] = 99;
        assert_eq!(
            decode_request(&request),
            Err(DiagnosticExportProtocolError::UnknownRequest(99))
        );

        let frame = encode_response(DiagnosticExportResponse::NoSealed).unwrap();
        let mut invalid = [0_u8; DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES];
        invalid.copy_from_slice(frame.as_bytes());
        invalid[18] = 1;
        assert!(matches!(
            decode_response(&invalid),
            Err(DiagnosticExportProtocolError::NonCanonicalReserved)
        ));
        invalid[18] = 0;
        invalid[1] = 99;
        assert!(matches!(
            decode_response(&invalid),
            Err(DiagnosticExportProtocolError::UnknownResponse(99))
        ));
        assert!(matches!(
            decode_response(&invalid[..invalid.len() - 1]),
            Err(DiagnosticExportProtocolError::InvalidLength)
        ));

        let frame = encode_response(DiagnosticExportResponse::NoSealed).unwrap();
        let mut noncanonical = [0_u8; DIAGNOSTIC_EXPORT_RESPONSE_HEADER_BYTES];
        noncanonical.copy_from_slice(frame.as_bytes());
        noncanonical[11] = 1;
        assert!(matches!(
            decode_response(&noncanonical),
            Err(DiagnosticExportProtocolError::InvalidPayload)
        ));
        noncanonical[11] = 0;
        noncanonical[3] = (MAX_RECORDS + 1) as u8;
        assert!(matches!(
            decode_response(&noncanonical),
            Err(DiagnosticExportProtocolError::InvalidPayload)
        ));
        noncanonical[3] = 0;
        noncanonical[17] = 1;
        assert!(matches!(
            decode_response(&noncanonical),
            Err(DiagnosticExportProtocolError::InvalidPayload)
        ));
    }

    #[test]
    fn invalid_bindings_are_never_encoded() {
        for response in [
            DiagnosticExportResponse::Acknowledged {
                instance_id: 0,
                window_id: 1,
            },
            DiagnosticExportResponse::AcknowledgementMismatch {
                instance_id: 1,
                window_id: 0,
            },
        ] {
            assert!(matches!(
                encode_response(response),
                Err(DiagnosticExportProtocolError::InvalidBinding)
            ));
        }
        let empty = DiagnosticExporter::new().export().unwrap();
        assert!(matches!(
            encode_response(DiagnosticExportResponse::Sealed {
                instance_id: 1,
                window_id: 1,
                omitted_lifecycle_events: 0,
                bundle: &empty,
            }),
            Err(DiagnosticExportProtocolError::InvalidPayload)
        ));
    }
}
