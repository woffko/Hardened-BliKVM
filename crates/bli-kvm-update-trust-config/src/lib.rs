//! Public-only application update trust enrollment.
//!
//! This crate validates a reviewed public-key request and renders the exact
//! supervisor configuration. It has no private-key, filesystem, activation,
//! state-initialization, network, or device authority.

use bli_kvm_update_verifier::{APPLICATION_PRODUCT, APPLICATION_TARGET, ApplicationTrustRoot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const TRUST_ENROLLMENT_SCHEMA_VERSION: u32 = 1;
pub const INITIAL_TRUST_GENERATION: u64 = 1;
pub const INITIAL_SYSTEM_SEQUENCE: u64 = 1;
pub const HEALTH_TIMEOUT_MS: u64 = 60_000;
pub const SLOT_ROOT: &str = "/var/lib/hardened-blikvm-slots";
pub const STATE_ROOT: &str = "/var/lib/hardened-blikvm-update";
pub const RUNTIME_ROOT: &str = "/run/hardened-blikvm-update";
pub const MAX_ENROLLMENT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TrustEnrollmentRequest {
    pub schema_version: u32,
    pub product: String,
    pub target: String,
    pub generation: u64,
    pub replaces_key_id: Option<String>,
    pub key_id: String,
    pub public_key_hex: String,
    pub public_key_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTrustEnrollment {
    key_id: String,
    public_key_hex: String,
    public_key_sha256: String,
}

impl ValidatedTrustEnrollment {
    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn public_key_sha256(&self) -> &str {
        &self.public_key_sha256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorConfig<'a> {
    schema_version: u32,
    product: &'static str,
    target: &'static str,
    key_id: &'a str,
    public_key_hex: &'a str,
    slot_root: &'static str,
    state_root: &'static str,
    runtime_root: &'static str,
    system_sequence: u64,
    health_timeout_ms: u64,
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TrustEnrollmentError {
    #[error("trust enrollment document exceeded its size limit")]
    DocumentTooLarge,
    #[error("trust enrollment document is invalid")]
    InvalidDocument,
    #[error("trust enrollment identity is invalid")]
    InvalidIdentity,
    #[error("trust enrollment generation is invalid")]
    InvalidGeneration,
    #[error("trust enrollment public key is invalid")]
    InvalidPublicKey,
    #[error("trust enrollment public-key fingerprint is invalid")]
    InvalidFingerprint,
    #[error("supervisor configuration serialization failed")]
    Serialization,
}

pub fn validate_enrollment_bytes(
    bytes: &[u8],
) -> Result<ValidatedTrustEnrollment, TrustEnrollmentError> {
    if bytes.is_empty() || bytes.len() > MAX_ENROLLMENT_BYTES {
        return Err(TrustEnrollmentError::DocumentTooLarge);
    }
    let request: TrustEnrollmentRequest =
        serde_json::from_slice(bytes).map_err(|_| TrustEnrollmentError::InvalidDocument)?;
    validate_enrollment(request)
}

pub fn validate_enrollment(
    request: TrustEnrollmentRequest,
) -> Result<ValidatedTrustEnrollment, TrustEnrollmentError> {
    if request.schema_version != TRUST_ENROLLMENT_SCHEMA_VERSION
        || request.product != APPLICATION_PRODUCT
        || request.target != APPLICATION_TARGET
    {
        return Err(TrustEnrollmentError::InvalidIdentity);
    }
    if request.generation != INITIAL_TRUST_GENERATION || request.replaces_key_id.is_some() {
        return Err(TrustEnrollmentError::InvalidGeneration);
    }
    let public_key = decode_public_key(&request.public_key_hex)?;
    let fingerprint = encode_hex(&Sha256::digest(public_key));
    if request.public_key_sha256 != fingerprint {
        return Err(TrustEnrollmentError::InvalidFingerprint);
    }
    ApplicationTrustRoot::new(&request.key_id, public_key)
        .map_err(|_| TrustEnrollmentError::InvalidPublicKey)?;
    Ok(ValidatedTrustEnrollment {
        key_id: request.key_id,
        public_key_hex: request.public_key_hex,
        public_key_sha256: request.public_key_sha256,
    })
}

pub fn render_supervisor_config(
    enrollment: &ValidatedTrustEnrollment,
) -> Result<Vec<u8>, TrustEnrollmentError> {
    let config = SupervisorConfig {
        schema_version: 1,
        product: APPLICATION_PRODUCT,
        target: APPLICATION_TARGET,
        key_id: &enrollment.key_id,
        public_key_hex: &enrollment.public_key_hex,
        slot_root: SLOT_ROOT,
        state_root: STATE_ROOT,
        runtime_root: RUNTIME_ROOT,
        system_sequence: INITIAL_SYSTEM_SEQUENCE,
        health_timeout_ms: HEALTH_TIMEOUT_MS,
    };
    let mut bytes = serde_json::to_vec(&config).map_err(|_| TrustEnrollmentError::Serialization)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn decode_public_key(value: &str) -> Result<[u8; 32], TrustEnrollmentError> {
    if value.len() != 64
        || value.bytes().all(|byte| byte == b'0')
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(TrustEnrollmentError::InvalidPublicKey);
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (decode_nibble(pair[0])? << 4) | decode_nibble(pair[1])?;
    }
    Ok(output)
}

fn decode_nibble(value: u8) -> Result<u8, TrustEnrollmentError> {
    match value {
        b'0'..=b'9' => Ok(value - b'0'),
        b'a'..=b'f' => Ok(value - b'a' + 10),
        _ => Err(TrustEnrollmentError::InvalidPublicKey),
    }
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use serde_json::{Value, json};

    use super::*;

    fn request() -> Value {
        let public_key = SigningKey::from_bytes(&[29; 32]).verifying_key().to_bytes();
        json!({
            "schemaVersion": 1,
            "product": APPLICATION_PRODUCT,
            "target": APPLICATION_TARGET,
            "generation": 1,
            "replacesKeyId": null,
            "keyId": "app-production-2026-01",
            "publicKeyHex": encode_hex(&public_key),
            "publicKeySha256": encode_hex(&Sha256::digest(public_key)),
        })
    }

    fn validate(value: &Value) -> Result<ValidatedTrustEnrollment, TrustEnrollmentError> {
        validate_enrollment_bytes(&serde_json::to_vec(value).unwrap())
    }

    #[test]
    fn validates_public_only_initial_enrollment_and_renders_fixed_config() {
        let enrollment = validate(&request()).unwrap();
        assert_eq!(enrollment.key_id(), "app-production-2026-01");
        assert_eq!(enrollment.public_key_sha256().len(), 64);
        let config = render_supervisor_config(&enrollment).unwrap();
        assert_eq!(config.last(), Some(&b'\n'));
        let value: Value = serde_json::from_slice(&config).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(value["slotRoot"], SLOT_ROOT);
        assert_eq!(value["stateRoot"], STATE_ROOT);
        assert_eq!(value["runtimeRoot"], RUNTIME_ROOT);
        assert_eq!(value["systemSequence"], INITIAL_SYSTEM_SEQUENCE);
        assert_eq!(value["healthTimeoutMs"], HEALTH_TIMEOUT_MS);
        assert!(value.get("privateKey").is_none());
        assert!(value.get("generation").is_none());
        assert!(value.get("replacesKeyId").is_none());
    }

    #[test]
    fn rejects_identity_generation_rotation_and_unknown_fields() {
        for (field, value) in [
            ("schemaVersion", json!(2)),
            ("product", json!("another-product")),
            ("target", json!("another-target")),
            ("generation", json!(2)),
            ("replacesKeyId", json!("old-key")),
        ] {
            let mut invalid = request();
            invalid[field] = value;
            assert!(validate(&invalid).is_err(), "accepted invalid {field}");
        }
        let mut unknown = request();
        unknown["privateKeyHex"] = json!("00");
        assert_eq!(
            validate(&unknown),
            Err(TrustEnrollmentError::InvalidDocument)
        );
        let mut path = request();
        path["slotRoot"] = json!("/tmp/selected-by-request");
        assert_eq!(validate(&path), Err(TrustEnrollmentError::InvalidDocument));
    }

    #[test]
    fn rejects_invalid_key_encoding_point_id_and_fingerprint() {
        let mut uppercase = request();
        uppercase["publicKeyHex"] = json!("AA".repeat(32));
        assert_eq!(
            validate(&uppercase),
            Err(TrustEnrollmentError::InvalidPublicKey)
        );

        let mut zeros = request();
        zeros["publicKeyHex"] = json!("00".repeat(32));
        zeros["publicKeySha256"] = json!(encode_hex(&Sha256::digest([0_u8; 32])));
        assert_eq!(
            validate(&zeros),
            Err(TrustEnrollmentError::InvalidPublicKey)
        );

        let mut key_id = request();
        key_id["keyId"] = json!("invalid key id");
        assert_eq!(
            validate(&key_id),
            Err(TrustEnrollmentError::InvalidPublicKey)
        );

        let mut fingerprint = request();
        fingerprint["publicKeySha256"] = json!("11".repeat(32));
        assert_eq!(
            validate(&fingerprint),
            Err(TrustEnrollmentError::InvalidFingerprint)
        );
    }

    #[test]
    fn rejects_empty_oversized_and_non_json_documents() {
        assert_eq!(
            validate_enrollment_bytes(&[]),
            Err(TrustEnrollmentError::DocumentTooLarge)
        );
        assert_eq!(
            validate_enrollment_bytes(&vec![b'x'; MAX_ENROLLMENT_BYTES + 1]),
            Err(TrustEnrollmentError::DocumentTooLarge)
        );
        assert_eq!(
            validate_enrollment_bytes(b"not-json"),
            Err(TrustEnrollmentError::InvalidDocument)
        );
    }
}
