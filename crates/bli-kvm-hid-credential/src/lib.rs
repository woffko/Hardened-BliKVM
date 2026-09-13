//! Fixed-path systemd credential loader for the HID transition channel.
//!
//! Systemd copies the `LoadCredential=` sources into a per-unit read-only
//! directory and exports only that path through `CREDENTIALS_DIRECTORY`. This
//! loader accepts a single compile-time directory and two compile-time
//! filenames, never a caller-supplied path. Credential values are never logged
//! or placed in the environment.

use std::{
    env, fs,
    path::{Path, PathBuf},
};

use bli_kvm_kvmd_client::{CredentialError, KvmdCredentials};
use thiserror::Error;

pub const HID_CREDENTIAL_SERVICE: &str = "hardened-blikvm-hid-credential.service";
pub const HID_CREDENTIAL_DIRECTORY: &str =
    "/run/credentials/hardened-blikvm-hid-credential.service";
pub const KVMD_USER_CREDENTIAL: &str = "kvmd-user";
pub const KVMD_PASSWORD_CREDENTIAL: &str = "kvmd-password";

const MAX_CREDENTIAL_FILE_BYTES: u64 = 1024;

#[derive(Debug, Error)]
pub enum HidCredentialError {
    #[error("systemd credential directory is not set")]
    MissingEnvironment,
    #[error("systemd credential directory is not the fixed HID channel")]
    UnexpectedDirectory,
    #[error("systemd credential {0} is missing")]
    MissingCredential(&'static str),
    #[error("systemd credential {0} could not be read")]
    UnreadableCredential(&'static str),
    #[error("systemd credential {0} is too large")]
    OversizedCredential(&'static str),
    #[error("systemd credential {0} is empty or invalid")]
    InvalidCredential(&'static str),
    #[error("KVMD credential is invalid")]
    Credential(#[from] CredentialError),
}

/// Loads the KVMD credential delivered to the fixed HID credential unit.
pub fn load_from_credentials_directory() -> Result<KvmdCredentials, HidCredentialError> {
    let directory = env::var_os("CREDENTIALS_DIRECTORY")
        .map(PathBuf::from)
        .ok_or(HidCredentialError::MissingEnvironment)?;
    if !is_accepted_directory(&directory) {
        return Err(HidCredentialError::UnexpectedDirectory);
    }
    load_from_directory(&directory)
}

pub fn is_accepted_directory(directory: &Path) -> bool {
    directory == Path::new(HID_CREDENTIAL_DIRECTORY)
}

fn load_from_directory(directory: &Path) -> Result<KvmdCredentials, HidCredentialError> {
    let username = read_credential(directory, KVMD_USER_CREDENTIAL)?;
    let password = read_credential(directory, KVMD_PASSWORD_CREDENTIAL)?;
    Ok(KvmdCredentials::new(&username, &password)?)
}

fn read_credential(directory: &Path, name: &'static str) -> Result<Vec<u8>, HidCredentialError> {
    let path = directory.join(name);
    let metadata =
        fs::symlink_metadata(&path).map_err(|_| HidCredentialError::MissingCredential(name))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(HidCredentialError::MissingCredential(name));
    }
    if metadata.len() > MAX_CREDENTIAL_FILE_BYTES {
        return Err(HidCredentialError::OversizedCredential(name));
    }
    let mut bytes = fs::read(&path).map_err(|_| HidCredentialError::UnreadableCredential(name))?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() {
        return Err(HidCredentialError::InvalidCredential(name));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn write(directory: &Path, name: &str, bytes: &[u8]) {
        fs::write(directory.join(name), bytes).unwrap();
    }

    #[test]
    fn only_the_fixed_directory_is_accepted() {
        assert!(is_accepted_directory(Path::new(HID_CREDENTIAL_DIRECTORY)));
        assert!(!is_accepted_directory(Path::new(
            "/run/credentials/hardened-blikvm-server.service"
        )));
        assert!(!is_accepted_directory(Path::new(
            "/etc/hardened-blikvm/hid"
        )));
    }

    #[test]
    fn loads_newline_terminated_credentials() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), KVMD_USER_CREDENTIAL, b"admin\n");
        write(
            directory.path(),
            KVMD_PASSWORD_CREDENTIAL,
            b"not-a-real-secret\r\n",
        );
        let credentials = load_from_directory(directory.path()).unwrap();
        drop(credentials);
    }

    #[test]
    fn rejects_missing_symlinked_oversized_and_empty_credentials() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            load_from_directory(directory.path()),
            Err(HidCredentialError::MissingCredential(KVMD_USER_CREDENTIAL))
        ));

        write(directory.path(), KVMD_USER_CREDENTIAL, b"");
        write(directory.path(), KVMD_PASSWORD_CREDENTIAL, b"value");
        assert!(matches!(
            load_from_directory(directory.path()),
            Err(HidCredentialError::InvalidCredential(KVMD_USER_CREDENTIAL))
        ));

        let oversized = vec![b'a'; MAX_CREDENTIAL_FILE_BYTES as usize + 1];
        write(directory.path(), KVMD_USER_CREDENTIAL, &oversized);
        assert!(matches!(
            load_from_directory(directory.path()),
            Err(HidCredentialError::OversizedCredential(
                KVMD_USER_CREDENTIAL
            ))
        ));

        write(directory.path(), KVMD_USER_CREDENTIAL, b"admin");
        let password = directory.path().join(KVMD_PASSWORD_CREDENTIAL);
        fs::remove_file(&password).unwrap();
        std::os::unix::fs::symlink(directory.path().join(KVMD_USER_CREDENTIAL), &password).unwrap();
        assert!(matches!(
            load_from_directory(directory.path()),
            Err(HidCredentialError::MissingCredential(
                KVMD_PASSWORD_CREDENTIAL
            ))
        ));
    }

    #[test]
    fn rejects_header_injection_content() {
        let directory = tempfile::tempdir().unwrap();
        write(directory.path(), KVMD_USER_CREDENTIAL, b"admin\n");
        write(
            directory.path(),
            KVMD_PASSWORD_CREDENTIAL,
            b"bad\r\nx-kvmd-user: attacker",
        );
        assert!(matches!(
            load_from_directory(directory.path()),
            Err(HidCredentialError::Credential(
                CredentialError::InvalidHeaderValue
            ))
        ));
    }
}
