//! Compile-time test-profile selection for the transition backends.
//!
//! Shipped builds never compile this module. A build with the
//! `hid-transition-profile` feature may select the HID transition backend from
//! fixed environment keys; every value must be an absolute path and the KVMD
//! credential is read only from systemd's `CREDENTIALS_DIRECTORY`.

use std::path::{Path, PathBuf};

use bli_kvm_kvmd_client::KvmdCredentials;

const MAX_CREDENTIAL_BYTES: u64 = 1024;

#[derive(Debug)]
pub struct HidProfile {
    pub ledger: PathBuf,
    pub kvmd_socket: PathBuf,
}

#[derive(Debug)]
pub enum ProfileError {
    MissingLedger,
    InvalidLedger,
    MissingKvmdSocket,
    InvalidKvmdSocket,
    MissingCredentials,
    InvalidCredentials,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLedger | Self::InvalidLedger => "HID profile ledger path is invalid",
            Self::MissingKvmdSocket | Self::InvalidKvmdSocket => {
                "HID profile KVMD socket path is invalid"
            }
            Self::MissingCredentials | Self::InvalidCredentials => {
                "HID profile credential is invalid"
            }
        })
    }
}

impl std::error::Error for ProfileError {}

/// Parses the HID test-profile selection. A disabled profile returns `None`
/// even when the path values are absent.
pub fn hid_profile_from_values(
    enabled: bool,
    ledger: Option<&str>,
    kvmd_socket: Option<&str>,
) -> Result<Option<HidProfile>, ProfileError> {
    if !enabled {
        return Ok(None);
    }
    let ledger = ledger.ok_or(ProfileError::MissingLedger)?;
    let kvmd_socket = kvmd_socket.ok_or(ProfileError::MissingKvmdSocket)?;
    let ledger = absolute_path(ledger).ok_or(ProfileError::InvalidLedger)?;
    let kvmd_socket = absolute_path(kvmd_socket).ok_or(ProfileError::InvalidKvmdSocket)?;
    Ok(Some(HidProfile {
        ledger,
        kvmd_socket,
    }))
}

fn absolute_path(value: &str) -> Option<PathBuf> {
    if value.is_empty() {
        return None;
    }
    let path = PathBuf::from(value);
    if path.is_absolute() { Some(path) } else { None }
}

/// Loads the KVMD credential that systemd delivered to the profile unit.
pub fn kvmd_credentials_from_directory(directory: &Path) -> Result<KvmdCredentials, ProfileError> {
    let username =
        read_credential(directory, "kvmd-user").ok_or(ProfileError::MissingCredentials)?;
    let password =
        read_credential(directory, "kvmd-password").ok_or(ProfileError::MissingCredentials)?;
    KvmdCredentials::new(username, password).map_err(|_| ProfileError::InvalidCredentials)
}

fn read_credential(directory: &Path, name: &str) -> Option<Vec<u8>> {
    let metadata = std::fs::symlink_metadata(directory.join(name)).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    if metadata.len() > MAX_CREDENTIAL_BYTES {
        return None;
    }
    let mut bytes = std::fs::read(directory.join(name)).ok()?;
    while matches!(bytes.last(), Some(b'\n' | b'\r')) {
        bytes.pop();
    }
    if bytes.is_empty() { None } else { Some(bytes) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_profile_ignores_missing_values() {
        assert!(matches!(
            hid_profile_from_values(false, None, None),
            Ok(None)
        ));
        assert!(matches!(
            hid_profile_from_values(false, Some("relative"), Some("relative")),
            Ok(None)
        ));
    }

    #[test]
    fn enabled_profile_requires_absolute_paths() {
        assert!(matches!(
            hid_profile_from_values(true, None, Some("/run/kvmd/kvmd.sock")),
            Err(ProfileError::MissingLedger)
        ));
        assert!(matches!(
            hid_profile_from_values(true, Some("/tmp/ledger.json"), None),
            Err(ProfileError::MissingKvmdSocket)
        ));
        assert!(matches!(
            hid_profile_from_values(true, Some("relative.json"), Some("/run/kvmd/kvmd.sock")),
            Err(ProfileError::InvalidLedger)
        ));
        assert!(matches!(
            hid_profile_from_values(true, Some("/tmp/ledger.json"), Some("relative.sock")),
            Err(ProfileError::InvalidKvmdSocket)
        ));
        let profile =
            hid_profile_from_values(true, Some("/tmp/ledger.json"), Some("/run/kvmd/kvmd.sock"))
                .unwrap()
                .unwrap();
        assert_eq!(profile.ledger, PathBuf::from("/tmp/ledger.json"));
        assert_eq!(profile.kvmd_socket, PathBuf::from("/run/kvmd/kvmd.sock"));
    }

    #[test]
    fn credential_loading_is_fixed_and_rejects_empty_or_symlinked_files() {
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            kvmd_credentials_from_directory(directory.path()),
            Err(ProfileError::MissingCredentials)
        ));

        std::fs::write(directory.path().join("kvmd-user"), b"admin\n").unwrap();
        std::fs::write(directory.path().join("kvmd-password"), b"").unwrap();
        assert!(matches!(
            kvmd_credentials_from_directory(directory.path()),
            Err(ProfileError::MissingCredentials)
        ));

        std::fs::write(
            directory.path().join("kvmd-password"),
            b"not-a-real-secret\n",
        )
        .unwrap();
        let credentials = kvmd_credentials_from_directory(directory.path()).unwrap();
        drop(credentials);
    }
}
