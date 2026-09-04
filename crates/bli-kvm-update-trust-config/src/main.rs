use std::{
    env,
    ffi::OsString,
    io::{self, Read, Write},
    process::ExitCode,
};

use bli_kvm_update_trust_config::{
    MAX_ENROLLMENT_BYTES, encode_hex, render_supervisor_config, validate_enrollment_bytes,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
enum CliError {
    #[error("trust enrollment arguments are invalid")]
    InvalidArguments,
    #[error("trust enrollment expected hash is invalid")]
    InvalidExpectedHash,
    #[error("trust enrollment request read failed")]
    Read,
    #[error("trust enrollment request hash mismatched")]
    HashMismatch,
    #[error("trust enrollment request was rejected")]
    InvalidEnrollment,
    #[error("trust enrollment configuration write failed")]
    Write,
}

fn main() -> ExitCode {
    let stdin = io::stdin();
    let stdout = io::stdout();
    match run(env::args_os(), stdin.lock(), stdout.lock()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Hardened BliKVM public trust validation failed: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

impl CliError {
    const fn exit_code(self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::InvalidExpectedHash => 10,
            Self::Read => 20,
            Self::HashMismatch => 21,
            Self::InvalidEnrollment => 30,
            Self::Write => 40,
        }
    }
}

fn run(
    arguments: impl IntoIterator<Item = OsString>,
    reader: impl Read,
    mut writer: impl Write,
) -> Result<(), CliError> {
    let expected = parse_arguments(arguments)?;
    let mut bytes = Vec::with_capacity(MAX_ENROLLMENT_BYTES);
    reader
        .take((MAX_ENROLLMENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| CliError::Read)?;
    if bytes.len() > MAX_ENROLLMENT_BYTES {
        return Err(CliError::InvalidEnrollment);
    }
    let actual = encode_hex(&Sha256::digest(&bytes));
    if actual != expected {
        return Err(CliError::HashMismatch);
    }
    let enrollment = validate_enrollment_bytes(&bytes).map_err(|_| CliError::InvalidEnrollment)?;
    let config = render_supervisor_config(&enrollment).map_err(|_| CliError::InvalidEnrollment)?;
    writer.write_all(&config).map_err(|_| CliError::Write)?;
    writer.flush().map_err(|_| CliError::Write)
}

fn parse_arguments(arguments: impl IntoIterator<Item = OsString>) -> Result<String, CliError> {
    let mut arguments = arguments.into_iter();
    let _program = arguments.next().ok_or(CliError::InvalidArguments)?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--expected-sha256")) {
        return Err(CliError::InvalidArguments);
    }
    let expected = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .ok_or(CliError::InvalidArguments)?;
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--confirm-reviewed-public-key"))
        || arguments.next().is_some()
    {
        return Err(CliError::InvalidArguments);
    }
    if expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(CliError::InvalidExpectedHash);
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use bli_kvm_update_trust_config::{
        INITIAL_SYSTEM_SEQUENCE, RUNTIME_ROOT, SLOT_ROOT, STATE_ROOT,
    };
    use ed25519_dalek::SigningKey;
    use serde_json::{Value, json};

    use super::*;

    fn request() -> Vec<u8> {
        let public_key = SigningKey::from_bytes(&[41; 32]).verifying_key().to_bytes();
        serde_json::to_vec(&json!({
            "schemaVersion": 1,
            "product": "Hardened BliKVM",
            "target": "bli-h616-pikvm",
            "generation": 1,
            "replacesKeyId": null,
            "keyId": "app-reviewed-test-1",
            "publicKeyHex": encode_hex(&public_key),
            "publicKeySha256": encode_hex(&Sha256::digest(public_key)),
        }))
        .unwrap()
    }

    fn arguments(hash: &str) -> Vec<OsString> {
        [
            "trust-config",
            "--expected-sha256",
            hash,
            "--confirm-reviewed-public-key",
        ]
        .into_iter()
        .map(OsString::from)
        .collect()
    }

    #[test]
    fn exact_hash_and_confirmation_render_only_fixed_configuration() {
        let request = request();
        let hash = encode_hex(&Sha256::digest(&request));
        let mut output = Vec::new();
        assert_eq!(
            run(arguments(&hash), Cursor::new(&request), &mut output),
            Ok(())
        );
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["slotRoot"], SLOT_ROOT);
        assert_eq!(value["stateRoot"], STATE_ROOT);
        assert_eq!(value["runtimeRoot"], RUNTIME_ROOT);
        assert_eq!(value["systemSequence"], INITIAL_SYSTEM_SEQUENCE);
        assert!(value.get("privateKey").is_none());
    }

    #[test]
    fn missing_confirmation_wrong_hash_and_extra_arguments_are_rejected() {
        let request = request();
        let hash = encode_hex(&Sha256::digest(&request));
        let missing = ["trust-config", "--expected-sha256", &hash]
            .into_iter()
            .map(OsString::from)
            .collect::<Vec<_>>();
        assert_eq!(
            run(missing, Cursor::new(&request), Vec::new()),
            Err(CliError::InvalidArguments)
        );
        assert_eq!(
            run(
                arguments(&"00".repeat(32)),
                Cursor::new(&request),
                Vec::new()
            ),
            Err(CliError::HashMismatch)
        );
        let mut extra = arguments(&hash);
        extra.push(OsString::from("unexpected"));
        assert_eq!(
            run(extra, Cursor::new(&request), Vec::new()),
            Err(CliError::InvalidArguments)
        );
    }

    #[test]
    fn invalid_expected_hash_document_and_size_are_rejected_without_output() {
        let request = request();
        let mut output = Vec::new();
        assert_eq!(
            run(arguments("ABC"), Cursor::new(&request), &mut output),
            Err(CliError::InvalidExpectedHash)
        );
        assert!(output.is_empty());

        let invalid = b"not-json";
        let hash = encode_hex(&Sha256::digest(invalid));
        assert_eq!(
            run(arguments(&hash), Cursor::new(invalid), &mut output),
            Err(CliError::InvalidEnrollment)
        );
        assert!(output.is_empty());

        let oversized = vec![b'x'; MAX_ENROLLMENT_BYTES + 1];
        let hash = encode_hex(&Sha256::digest(&oversized));
        assert_eq!(
            run(arguments(&hash), Cursor::new(oversized), &mut output),
            Err(CliError::InvalidEnrollment)
        );
        assert!(output.is_empty());
    }

    #[test]
    fn write_failures_are_coarse_and_distinct() {
        struct FailingWriter;
        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::other("synthetic"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let request = request();
        let hash = encode_hex(&Sha256::digest(&request));
        assert_eq!(
            run(arguments(&hash), Cursor::new(request), FailingWriter),
            Err(CliError::Write)
        );
    }
}
