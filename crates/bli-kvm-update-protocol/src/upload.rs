//! Stage-only metadata for a separate authenticated upload socket.
//!
//! One framed request precedes exactly `archiveBytes` raw archive bytes and
//! write-half EOF. One framed response follows. Never dispatch this protocol
//! through the read-only status socket. Transport deadlines and peer identity
//! are enforced by the endpoints, not by these metadata codecs.

use serde::{Deserialize, Serialize};

use crate::{ApplicationSlotVersion, UpdateProtocolError, bounded, envelope};

pub const MAX_UPLOAD_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UploadRequest {
    Stage {
        #[serde(rename = "archiveBytes")]
        archive_bytes: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadRequestEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub request: UploadRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadDisposition {
    Published,
    AlreadyPresent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadReceipt {
    pub slot: ApplicationSlotVersion,
    pub disposition: UploadDisposition,
    pub manifest_sha256: String,
    pub payload_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UploadErrorCode {
    Busy,
    InvalidArchive,
    InvalidState,
    QuotaExceeded,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UploadResponse {
    Staged { receipt: UploadReceipt },
    Error { code: UploadErrorCode },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadResponseEnvelope {
    pub schema_version: u16,
    pub request_id: u64,
    pub response: UploadResponse,
}

impl UploadRequestEnvelope {
    fn validate(&self) -> Result<(), UpdateProtocolError> {
        envelope(self.schema_version, self.request_id)?;
        let UploadRequest::Stage { archive_bytes } = self.request;
        if archive_bytes == 0 || archive_bytes > MAX_UPLOAD_ARCHIVE_BYTES {
            return Err(UpdateProtocolError::InvalidLength);
        }
        Ok(())
    }
}

impl UploadResponseEnvelope {
    fn validate(&self) -> Result<(), UpdateProtocolError> {
        envelope(self.schema_version, self.request_id)?;
        if let UploadResponse::Staged { receipt } = &self.response {
            receipt.slot.validate()?;
            // Slot identity binds the exact signed manifest, not an arbitrary
            // server-provided hash. Slot validation makes this slice safe.
            if receipt.manifest_sha256 != receipt.slot.slot_id[25..]
                || receipt.payload_bytes == 0
                || receipt.payload_bytes > MAX_UPLOAD_ARCHIVE_BYTES
            {
                return Err(UpdateProtocolError::InvalidStatus);
            }
        }
        Ok(())
    }
}

pub fn decode_request(bytes: &[u8]) -> Result<UploadRequestEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value: UploadRequestEnvelope =
        serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    value.validate()?;
    Ok(value)
}

pub fn encode_request(value: &UploadRequestEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    value.validate()?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

pub fn decode_response(bytes: &[u8]) -> Result<UploadResponseEnvelope, UpdateProtocolError> {
    bounded(bytes)?;
    let value: UploadResponseEnvelope =
        serde_json::from_slice(bytes).map_err(|_| UpdateProtocolError::InvalidJson)?;
    value.validate()?;
    Ok(value)
}

pub fn encode_response(value: &UploadResponseEnvelope) -> Result<Vec<u8>, UpdateProtocolError> {
    value.validate()?;
    let bytes = serde_json::to_vec(value).map_err(|_| UpdateProtocolError::InvalidJson)?;
    bounded(&bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MAX_UPDATE_MESSAGE_BYTES, decode_frame, encode_frame};

    fn request() -> UploadRequestEnvelope {
        UploadRequestEnvelope {
            schema_version: 1,
            request_id: u64::MAX,
            request: UploadRequest::Stage {
                archive_bytes: MAX_UPLOAD_ARCHIVE_BYTES,
            },
        }
    }

    fn response() -> UploadResponseEnvelope {
        UploadResponseEnvelope {
            schema_version: 1,
            request_id: u64::MAX,
            response: UploadResponse::Staged {
                receipt: UploadReceipt {
                    slot: ApplicationSlotVersion {
                        slot_id: format!("app-{:020}-{}", u64::MAX, "a".repeat(64)),
                        version: "1.0.0".into(),
                        sequence: u64::MAX,
                        minimum_system_sequence: 1,
                    },
                    disposition: UploadDisposition::Published,
                    manifest_sha256: "a".repeat(64),
                    payload_bytes: 1024,
                },
            },
        }
    }

    #[test]
    fn round_trip_exact_framed_metadata() {
        let request = request();
        let frame = encode_frame(&encode_request(&request).unwrap()).unwrap();
        assert_eq!(
            decode_request(decode_frame(&frame).unwrap()).unwrap(),
            request
        );
        let response = response();
        assert_eq!(
            decode_response(&encode_response(&response).unwrap()).unwrap(),
            response
        );
        assert!(crate::decode_request(&encode_request(&request).unwrap()).is_err());
        assert!(
            decode_request(br#"{"schemaVersion":1,"requestId":1,"request":{"type":"status"}}"#)
                .is_err()
        );
    }

    #[test]
    fn rejects_unknown_fields_commands_duplicates_and_bounds() {
        for bytes in [
            r#"{"schemaVersion":1,"requestId":1,"request":{"type":"activate","archiveBytes":1}}"#,
            r#"{"schemaVersion":1,"requestId":1,"request":{"type":"stage","archiveBytes":1,"path":"/tmp/archive"}}"#,
            r#"{"schemaVersion":1,"requestId":1,"request":{"type":"stage","archiveBytes":1,"url":"https://example.invalid"}}"#,
            r#"{"schemaVersion":1,"requestId":1,"request":{"type":"stage","archiveBytes":1,"archiveBytes":2}}"#,
            r#"{"schemaVersion":1,"requestId":1,"request":{"type":"stage","archiveBytes":1},"extra":1}"#,
        ] {
            assert!(decode_request(bytes.as_bytes()).is_err());
        }
        for size in [0, MAX_UPLOAD_ARCHIVE_BYTES + 1, u64::MAX] {
            let mut value = request();
            value.request = UploadRequest::Stage {
                archive_bytes: size,
            };
            assert!(encode_request(&value).is_err());
            assert!(decode_request(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        let mut value = request();
        value.request_id = 0;
        assert!(encode_request(&value).is_err());
        value = request();
        value.schema_version = 2;
        assert!(encode_request(&value).is_err());
        assert!(decode_request(&vec![b' '; MAX_UPDATE_MESSAGE_BYTES + 1]).is_err());
    }

    #[test]
    fn receipt_binds_manifest_identity_and_rejects_unsafe_values() {
        for field in ["hash", "size", "slot", "sequence", "version"] {
            let mut value = response();
            let UploadResponse::Staged { receipt } = &mut value.response else {
                unreachable!()
            };
            match field {
                "hash" => receipt.manifest_sha256 = "b".repeat(64),
                "size" => receipt.payload_bytes = MAX_UPLOAD_ARCHIVE_BYTES + 1,
                "slot" => receipt.slot.slot_id = "../bad".into(),
                "sequence" => receipt.slot.sequence = 1,
                "version" => receipt.slot.version = "<script>".into(),
                _ => unreachable!(),
            }
            assert!(encode_response(&value).is_err());
            assert!(decode_response(&serde_json::to_vec(&value).unwrap()).is_err());
        }
    }

    #[test]
    fn error_codes_and_idempotent_receipts_are_closed() {
        for code in [
            UploadErrorCode::Busy,
            UploadErrorCode::InvalidArchive,
            UploadErrorCode::InvalidState,
            UploadErrorCode::QuotaExceeded,
            UploadErrorCode::Unavailable,
        ] {
            let mut value = response();
            value.response = UploadResponse::Error { code };
            assert_eq!(
                decode_response(&encode_response(&value).unwrap()).unwrap(),
                value
            );
        }
        let mut value = response();
        let UploadResponse::Staged { receipt } = &mut value.response else {
            unreachable!()
        };
        receipt.disposition = UploadDisposition::AlreadyPresent;
        assert_eq!(
            decode_response(&encode_response(&value).unwrap()).unwrap(),
            value
        );
        assert!(decode_response(br#"{"schemaVersion":1,"requestId":1,"response":{"type":"error","code":"unavailable","path":"/private"}}"#).is_err());
    }
}
