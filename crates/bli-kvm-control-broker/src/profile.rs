//! Compile-time test-profile selection for the transition backends.
//!
//! Shipped builds never compile this module. A build with the
//! `hid-transition-profile` feature may select the HID transition backend from
//! fixed environment keys; every value must be an absolute path and the KVMD
//! credential is read only from systemd's `CREDENTIALS_DIRECTORY`.

use std::path::{Path, PathBuf};

use bli_kvm_kvmd_client::KvmdCredentials;

const MAX_CREDENTIAL_BYTES: u64 = 1024;

#[cfg(feature = "hid-transition-profile")]
#[derive(Debug)]
pub struct HidProfile {
    pub ledger: PathBuf,
    pub kvmd_socket: PathBuf,
}

#[cfg(feature = "atx-transition-profile")]
#[derive(Debug)]
pub struct AtxProfile {
    pub journal: PathBuf,
    pub kvmd_socket: PathBuf,
}

#[cfg(feature = "media-transition-profile")]
#[derive(Debug)]
pub struct MediaProfile {
    pub journal: PathBuf,
    pub catalog: PathBuf,
    pub kvmd_socket: PathBuf,
}

#[derive(Debug)]
pub enum ProfileError {
    MissingLedger,
    InvalidLedger,
    MissingKvmdSocket,
    InvalidKvmdSocket,
    MissingAtxJournal,
    InvalidAtxJournal,
    MissingAtxKvmdSocket,
    InvalidAtxKvmdSocket,
    MissingMediaJournal,
    InvalidMediaJournal,
    MissingMediaCatalog,
    InvalidMediaCatalog,
    MissingMediaKvmdSocket,
    InvalidMediaKvmdSocket,
    MissingCredentials,
    InvalidCredentials,
    InvalidReconciliation,
}

impl std::fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::MissingLedger | Self::InvalidLedger => "HID profile ledger path is invalid",
            Self::MissingKvmdSocket | Self::InvalidKvmdSocket => {
                "HID profile KVMD socket path is invalid"
            }
            Self::MissingAtxJournal | Self::InvalidAtxJournal => {
                "ATX profile journal path is invalid"
            }
            Self::MissingAtxKvmdSocket | Self::InvalidAtxKvmdSocket => {
                "ATX profile KVMD socket path is invalid"
            }
            Self::MissingMediaJournal | Self::InvalidMediaJournal => {
                "media profile journal path is invalid"
            }
            Self::MissingMediaCatalog | Self::InvalidMediaCatalog => {
                "media profile catalog path is invalid"
            }
            Self::MissingMediaKvmdSocket | Self::InvalidMediaKvmdSocket => {
                "media profile KVMD socket path is invalid"
            }
            Self::MissingCredentials | Self::InvalidCredentials => {
                "transition profile credential is invalid"
            }
            Self::InvalidReconciliation => "ATX reconciliation file is invalid",
        })
    }
}

impl std::error::Error for ProfileError {}

/// Parses the HID test-profile selection. A disabled profile returns `None`
/// even when the path values are absent.
#[cfg(feature = "hid-transition-profile")]
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

/// Parses the ATX test-profile selection. A disabled profile returns `None`
/// even when the path values are absent.
#[cfg(feature = "atx-transition-profile")]
pub fn atx_profile_from_values(
    enabled: bool,
    journal: Option<&str>,
    kvmd_socket: Option<&str>,
) -> Result<Option<AtxProfile>, ProfileError> {
    if !enabled {
        return Ok(None);
    }
    let journal = journal.ok_or(ProfileError::MissingAtxJournal)?;
    let kvmd_socket = kvmd_socket.ok_or(ProfileError::MissingAtxKvmdSocket)?;
    let journal = absolute_path(journal).ok_or(ProfileError::InvalidAtxJournal)?;
    let kvmd_socket = absolute_path(kvmd_socket).ok_or(ProfileError::InvalidAtxKvmdSocket)?;
    Ok(Some(AtxProfile {
        journal,
        kvmd_socket,
    }))
}

/// Parses the media test-profile selection. A disabled profile returns `None`
/// even when the path values are absent.
#[cfg(feature = "media-transition-profile")]
pub fn media_profile_from_values(
    enabled: bool,
    journal: Option<&str>,
    catalog: Option<&str>,
    kvmd_socket: Option<&str>,
) -> Result<Option<MediaProfile>, ProfileError> {
    if !enabled {
        return Ok(None);
    }
    let journal = journal.ok_or(ProfileError::MissingMediaJournal)?;
    let catalog = catalog.ok_or(ProfileError::MissingMediaCatalog)?;
    let kvmd_socket = kvmd_socket.ok_or(ProfileError::MissingMediaKvmdSocket)?;
    let journal = absolute_path(journal).ok_or(ProfileError::InvalidMediaJournal)?;
    let catalog = absolute_path(catalog).ok_or(ProfileError::InvalidMediaCatalog)?;
    let kvmd_socket = absolute_path(kvmd_socket).ok_or(ProfileError::InvalidMediaKvmdSocket)?;
    Ok(Some(MediaProfile {
        journal,
        catalog,
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

/// Parses the optional reviewed startup reconciliation file for the ATX
/// profile.
///
/// Each non-empty, non-comment line is one reviewed decision with the exact
/// fields `operationId=<32 hex>`, `completion=applied|rejected` and
/// `observation=on|off`. The observation is mandatory read-only evidence and
/// never infers the completion; the operator supplies both. A disabled profile
/// returns an empty list even when a path is present.
#[cfg(feature = "atx-transition-profile")]
pub fn atx_reconciliations_from_values(
    enabled: bool,
    path: Option<&str>,
) -> Result<Vec<crate::AtxReconciliation>, ProfileError> {
    if !enabled {
        return Ok(Vec::new());
    }
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let path = absolute_path(path).ok_or(ProfileError::InvalidReconciliation)?;
    atx_reconciliations_from_file(&path)
}

#[cfg(feature = "atx-transition-profile")]
fn atx_reconciliations_from_file(
    path: &Path,
) -> Result<Vec<crate::AtxReconciliation>, ProfileError> {
    use bli_kvm_atx_policy::AtxCompletion;
    use bli_kvm_atx_transition::PowerLedObservation;

    const MAX_RECONCILIATION_BYTES: u64 = 16 * 1024;
    const MAX_RECONCILIATIONS: usize = 64;

    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| ProfileError::InvalidReconciliation)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_RECONCILIATION_BYTES
    {
        return Err(ProfileError::InvalidReconciliation);
    }
    let content = std::fs::read_to_string(path).map_err(|_| ProfileError::InvalidReconciliation)?;
    let mut reconciliations = Vec::new();
    let mut seen: Vec<[u8; 16]> = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if reconciliations.len() >= MAX_RECONCILIATIONS {
            return Err(ProfileError::InvalidReconciliation);
        }
        let mut operation_id = None;
        let mut completion = None;
        let mut observation = None;
        for token in line.split_whitespace() {
            let (key, value) = token
                .split_once('=')
                .ok_or(ProfileError::InvalidReconciliation)?;
            match key {
                "operationId" if operation_id.is_none() => {
                    operation_id = Some(parse_operation_id(value)?);
                }
                "completion" if completion.is_none() => {
                    completion = Some(match value {
                        "applied" => AtxCompletion::Applied,
                        "rejected" => AtxCompletion::Rejected,
                        _ => return Err(ProfileError::InvalidReconciliation),
                    });
                }
                "observation" if observation.is_none() => {
                    observation = Some(match value {
                        "on" => PowerLedObservation::On,
                        "off" => PowerLedObservation::Off,
                        _ => return Err(ProfileError::InvalidReconciliation),
                    });
                }
                _ => return Err(ProfileError::InvalidReconciliation),
            }
        }
        let operation_id = operation_id.ok_or(ProfileError::InvalidReconciliation)?;
        let completion = completion.ok_or(ProfileError::InvalidReconciliation)?;
        let observation = observation.ok_or(ProfileError::InvalidReconciliation)?;
        if seen.contains(&operation_id) {
            return Err(ProfileError::InvalidReconciliation);
        }
        seen.push(operation_id);
        reconciliations.push(crate::AtxReconciliation {
            operation_id,
            completion,
            observation,
        });
    }
    Ok(reconciliations)
}

#[cfg(feature = "atx-transition-profile")]
fn parse_operation_id(value: &str) -> Result<[u8; 16], ProfileError> {
    if value.len() != 32 {
        return Err(ProfileError::InvalidReconciliation);
    }
    let mut output = [0_u8; 16];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).map_err(|_| ProfileError::InvalidReconciliation)?;
        output[index] =
            u8::from_str_radix(text, 16).map_err(|_| ProfileError::InvalidReconciliation)?;
    }
    Ok(output)
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
    #[cfg(feature = "hid-transition-profile")]
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
    #[cfg(feature = "hid-transition-profile")]
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
    #[cfg(feature = "atx-transition-profile")]
    fn atx_profile_requires_absolute_paths() {
        assert!(matches!(
            atx_profile_from_values(true, None, Some("/run/kvmd/kvmd.sock")),
            Err(ProfileError::MissingAtxJournal)
        ));
        assert!(matches!(
            atx_profile_from_values(true, Some("/tmp/atx.json"), None),
            Err(ProfileError::MissingAtxKvmdSocket)
        ));
        assert!(matches!(
            atx_profile_from_values(true, Some("relative.json"), Some("/run/kvmd/kvmd.sock")),
            Err(ProfileError::InvalidAtxJournal)
        ));
        assert!(matches!(
            atx_profile_from_values(true, Some("/tmp/atx.json"), Some("relative.sock")),
            Err(ProfileError::InvalidAtxKvmdSocket)
        ));
        assert!(matches!(
            atx_profile_from_values(false, None, None),
            Ok(None)
        ));
        let profile =
            atx_profile_from_values(true, Some("/tmp/atx.json"), Some("/run/kvmd/kvmd.sock"))
                .unwrap()
                .unwrap();
        assert_eq!(profile.journal, PathBuf::from("/tmp/atx.json"));
        assert_eq!(profile.kvmd_socket, PathBuf::from("/run/kvmd/kvmd.sock"));
    }

    #[test]
    #[cfg(feature = "atx-transition-profile")]
    fn atx_reconciliations_are_reviewed_and_bounded() {
        use crate::AtxReconciliation;
        use bli_kvm_atx_policy::AtxCompletion;
        use bli_kvm_atx_transition::PowerLedObservation;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reconciliations.txt");
        std::fs::write(
            &path,
            b"# reviewed at 2026-09-13\n\
              operationId=00112233445566778899aabbccddeeff completion=applied observation=on\n",
        )
        .unwrap();
        let parsed = atx_reconciliations_from_values(true, path.to_str()).unwrap();
        assert_eq!(
            parsed,
            vec![AtxReconciliation {
                operation_id: [
                    0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc,
                    0xdd, 0xee, 0xff,
                ],
                completion: AtxCompletion::Applied,
                observation: PowerLedObservation::On,
            }]
        );

        assert!(
            atx_reconciliations_from_values(false, path.to_str())
                .unwrap()
                .is_empty()
        );
        assert!(atx_reconciliations_from_values(true, Some("relative.txt")).is_err());
        assert!(
            atx_reconciliations_from_values(true, None)
                .unwrap()
                .is_empty()
        );

        for invalid in [
            "operationId=00112233445566778899aabbccddeeff completion=applied observation=maybe\n",
            "operationId=zz112233445566778899aabbccddeeff completion=applied observation=on\n",
            "operationId=00112233445566778899aabbccddeeff completion=applied\n",
            "operationId=00112233445566778899aabbccddeeff completion=applied observation=on extra=1\n",
            "operationId=00112233445566778899aabbccddeeff completion=applied observation=on\n\
             operationId=00112233445566778899aabbccddeeff completion=rejected observation=off\n",
        ] {
            std::fs::write(&path, invalid.as_bytes()).unwrap();
            assert!(
                atx_reconciliations_from_values(true, path.to_str()).is_err(),
                "invalid reconciliation content was accepted: {invalid:?}"
            );
        }
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
