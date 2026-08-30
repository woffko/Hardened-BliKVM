use std::{
    fs::{self, File, Permissions},
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_hid_adapter::{KvmdHidEvent, KvmdKeyCode, KvmdMouseButton};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const LEDGER_SCHEMA_VERSION: u16 = 1;
const MAX_LEDGER_BYTES: u64 = 8 * 1024;

#[derive(Debug, Error)]
pub enum HidReleaseLedgerError {
    #[error("HID release ledger path is invalid")]
    InvalidPath,
    #[error("HID release ledger directory is unsafe")]
    UnsafeDirectory,
    #[error("HID release ledger file is unsafe")]
    UnsafeFile,
    #[error("HID release ledger is too large")]
    TooLarge,
    #[error("HID release ledger contains an invalid event")]
    InvalidEvent,
    #[error("HID release ledger contains invalid JSON")]
    InvalidJson,
    #[error("HID release ledger I/O failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct HidReleaseLedger {
    path: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LedgerDocument {
    schema_version: u16,
    releases: Vec<ReleaseRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ReleaseRecord {
    Key { usb_usage: u8 },
    MouseButton { button: LedgerMouseButton },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum LedgerMouseButton {
    Left,
    Right,
    Middle,
}

impl HidReleaseLedger {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, HidReleaseLedgerError> {
        let path = path.into();
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(HidReleaseLedgerError::InvalidPath);
        }
        validate_directory(path.parent().ok_or(HidReleaseLedgerError::InvalidPath)?)?;
        let _ = validate_existing_file(&path)?;
        Ok(Self { path })
    }

    pub fn load(&self) -> Result<Vec<KvmdHidEvent>, HidReleaseLedgerError> {
        if !validate_existing_file(&self.path)? {
            return Ok(Vec::new());
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.len() > MAX_LEDGER_BYTES {
            return Err(HidReleaseLedgerError::TooLarge);
        }
        let mut file = File::open(&self.path)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAX_LEDGER_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(HidReleaseLedgerError::TooLarge);
        }
        let document: LedgerDocument =
            serde_json::from_slice(&bytes).map_err(|_| HidReleaseLedgerError::InvalidJson)?;
        if document.schema_version != LEDGER_SCHEMA_VERSION {
            return Err(HidReleaseLedgerError::InvalidJson);
        }
        let mut events = Vec::new();
        for record in document.releases {
            let event = record_to_event(record)?;
            if !events.contains(&event) {
                events.push(event);
            }
        }
        Ok(events)
    }

    pub fn store(&self, events: &[KvmdHidEvent]) -> Result<(), HidReleaseLedgerError> {
        validate_directory(
            self.path
                .parent()
                .ok_or(HidReleaseLedgerError::InvalidPath)?,
        )?;
        let _ = validate_existing_file(&self.path)?;
        let releases = events
            .iter()
            .map(event_to_record)
            .collect::<Result<Vec<_>, _>>()?;
        let bytes = serde_json::to_vec(&LedgerDocument {
            schema_version: LEDGER_SCHEMA_VERSION,
            releases,
        })
        .map_err(|_| HidReleaseLedgerError::InvalidJson)?;
        if bytes.len() as u64 > MAX_LEDGER_BYTES {
            return Err(HidReleaseLedgerError::TooLarge);
        }
        let parent = self
            .path
            .parent()
            .ok_or(HidReleaseLedgerError::InvalidPath)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| HidReleaseLedgerError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        validate_file(&self.path)?;
        Ok(())
    }
}

fn validate_directory(path: &Path) -> Result<(), HidReleaseLedgerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(HidReleaseLedgerError::UnsafeDirectory);
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), HidReleaseLedgerError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(HidReleaseLedgerError::UnsafeFile);
    }
    Ok(())
}

fn validate_existing_file(path: &Path) -> Result<bool, HidReleaseLedgerError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_file(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(HidReleaseLedgerError::Io(error)),
    }
}

fn event_to_record(event: &KvmdHidEvent) -> Result<ReleaseRecord, HidReleaseLedgerError> {
    match event {
        KvmdHidEvent::Key {
            key,
            pressed: false,
        } => Ok(ReleaseRecord::Key {
            usb_usage: key.usb_usage(),
        }),
        KvmdHidEvent::MouseButton {
            button,
            pressed: false,
        } => Ok(ReleaseRecord::MouseButton {
            button: match button {
                KvmdMouseButton::Left => LedgerMouseButton::Left,
                KvmdMouseButton::Right => LedgerMouseButton::Right,
                KvmdMouseButton::Middle => LedgerMouseButton::Middle,
            },
        }),
        _ => Err(HidReleaseLedgerError::InvalidEvent),
    }
}

fn record_to_event(record: ReleaseRecord) -> Result<KvmdHidEvent, HidReleaseLedgerError> {
    match record {
        ReleaseRecord::Key { usb_usage } => Ok(KvmdHidEvent::Key {
            key: KvmdKeyCode::from_usb_usage(usb_usage)
                .ok_or(HidReleaseLedgerError::InvalidEvent)?,
            pressed: false,
        }),
        ReleaseRecord::MouseButton { button } => Ok(KvmdHidEvent::MouseButton {
            button: match button {
                LedgerMouseButton::Left => KvmdMouseButton::Left,
                LedgerMouseButton::Right => KvmdMouseButton::Right,
                LedgerMouseButton::Middle => KvmdMouseButton::Middle,
            },
            pressed: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn atomically_roundtrips_private_release_events() {
        let directory = tempfile::tempdir().unwrap();
        let ledger = HidReleaseLedger::new(directory.path().join("releases.json")).unwrap();
        let events = vec![
            KvmdHidEvent::Key {
                key: KvmdKeyCode::from_usb_usage(4).unwrap(),
                pressed: false,
            },
            KvmdHidEvent::MouseButton {
                button: KvmdMouseButton::Left,
                pressed: false,
            },
        ];
        ledger.store(&events).unwrap();
        assert_eq!(ledger.load().unwrap(), events);
        assert_eq!(
            fs::symlink_metadata(directory.path().join("releases.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(directory.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn rejects_relative_group_writable_and_non_release_inputs() {
        assert!(matches!(
            HidReleaseLedger::new("releases.json"),
            Err(HidReleaseLedgerError::InvalidPath)
        ));
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            HidReleaseLedger::new(directory.path().join("releases.json")),
            Err(HidReleaseLedgerError::UnsafeDirectory)
        ));
        fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        let ledger = HidReleaseLedger::new(directory.path().join("releases.json")).unwrap();
        assert!(matches!(
            ledger.store(&[KvmdHidEvent::Key {
                key: KvmdKeyCode::from_usb_usage(4).unwrap(),
                pressed: true,
            }]),
            Err(HidReleaseLedgerError::InvalidEvent)
        ));

        let dangling = directory.path().join("dangling.json");
        symlink(directory.path().join("missing-target"), &dangling).unwrap();
        assert!(matches!(
            HidReleaseLedger::new(dangling),
            Err(HidReleaseLedgerError::UnsafeFile)
        ));
    }
}
