//! Root-owned selection ledger for protected native execution copies.
//! The ledger limits the journal owner to one rollback step: a slot may be
//! selected if it is not older than the current one, or if it is exactly the
//! previously selected slot. The ledger never confirms health.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::PathBuf,
};

use bli_kvm_update_stager::ApplicationStageError;
use serde::{Deserialize, Serialize};

const LEDGER_FILE_NAME: &str = "selection.json";
const LEDGER_TEMPORARY_NAME: &str = ".selection.json.next";
const LEDGER_SCHEMA_VERSION: u32 = 1;
const MAX_LEDGER_BYTES: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SelectionLedger {
    schema_version: u32,
    current: Option<String>,
    previous: Option<String>,
}

pub(crate) fn slot_sequence(slot_id: &str) -> Option<u64> {
    bli_kvm_update_protocol::valid_slot_id(slot_id)
        .then(|| slot_id[4..24].parse().ok())
        .flatten()
}

impl SelectionLedger {
    fn empty() -> Self {
        Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            current: None,
            previous: None,
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn previous(&self) -> Option<&str> {
        self.previous.as_deref()
    }

    fn valid(&self) -> bool {
        let slot = |value: &Option<String>| {
            value
                .as_deref()
                .is_none_or(|id| slot_sequence(id).is_some())
        };
        self.schema_version == LEDGER_SCHEMA_VERSION
            && slot(&self.current)
            && slot(&self.previous)
            && (self.previous.is_none()
                || (self.current.is_some() && self.previous != self.current))
    }

    /// The ledger after selecting `slot_id`, or `None` when it is refused.
    pub fn select(&self, slot_id: &str) -> Option<Self> {
        let target = slot_sequence(slot_id)?;
        let only = |slot: &str| Self {
            current: Some(slot.to_string()),
            ..Self::empty()
        };
        let Some(current) = self.current.as_deref() else {
            return Some(only(slot_id));
        };
        if current == slot_id {
            return Some(self.clone());
        }
        if self.previous.as_deref() == Some(slot_id) {
            return Some(only(slot_id));
        }
        (target >= slot_sequence(current)?).then(|| Self {
            schema_version: LEDGER_SCHEMA_VERSION,
            current: Some(slot_id.to_string()),
            previous: Some(current.to_string()),
        })
    }
}

/// A directory owned by the effective user that holds only the ledger.
/// Production construction also requires protected root-owned ancestry.
pub struct SelectionLedgerStore {
    root: PathBuf,
    protected: bool,
}

fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not dereference pointers.
    unsafe { libc::geteuid() }
}

impl SelectionLedgerStore {
    /// The path must come from protected local configuration.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ApplicationStageError> {
        if effective_uid() != 0 {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        let store = Self {
            root: root.into(),
            protected: true,
        };
        store.validate_root()?;
        Ok(store)
    }

    /// Unprivileged fixture: owner checks only, no ancestry walk.
    #[cfg(test)]
    pub(crate) fn fixture(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            protected: false,
        }
    }

    fn validate_root(&self) -> Result<(), ApplicationStageError> {
        if self.protected {
            bli_kvm_update_stager::validate_protected_directory(&self.root)?;
        }
        let metadata =
            fs::symlink_metadata(&self.root).map_err(|_| ApplicationStageError::UnsafeRoot)?;
        if !metadata.is_dir() || metadata.uid() != effective_uid() || metadata.mode() & 0o022 != 0 {
            return Err(ApplicationStageError::UnsafeRoot);
        }
        Ok(())
    }

    pub fn load(&self) -> Result<SelectionLedger, ApplicationStageError> {
        self.validate_root()?;
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(self.root.join(LEDGER_FILE_NAME))
        {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(SelectionLedger::empty());
            }
            Err(_) => return Err(ApplicationStageError::UnsafeExistingSlot),
        };
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != effective_uid()
            || metadata.nlink() != 1
            || metadata.mode() & 0o022 != 0
            || metadata.len() > MAX_LEDGER_BYTES
        {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        let mut bytes = Vec::new();
        file.take(MAX_LEDGER_BYTES + 1).read_to_end(&mut bytes)?;
        let ledger: SelectionLedger = serde_json::from_slice(&bytes)
            .map_err(|_| ApplicationStageError::UnsafeExistingSlot)?;
        if !ledger.valid() {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        Ok(ledger)
    }

    /// Atomically replace the ledger and verify the durable result.
    pub fn store(&self, ledger: &SelectionLedger) -> Result<(), ApplicationStageError> {
        if !ledger.valid() {
            return Err(ApplicationStageError::PolicyRejected);
        }
        self.validate_root()?;
        let bytes = serde_json::to_vec(ledger).map_err(|_| ApplicationStageError::Serialization)?;
        let temporary = self.root.join(LEDGER_TEMPORARY_NAME);
        match fs::remove_file(&temporary) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, self.root.join(LEDGER_FILE_NAME))?;
        File::open(&self.root)?.sync_all()?;
        if self.load()? != *ledger {
            return Err(ApplicationStageError::UnsafeExistingSlot);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    fn slot(sequence: u64, digit: char) -> String {
        format!("app-{sequence:020}-{}", digit.to_string().repeat(64))
    }

    #[test]
    fn ledger_allows_forward_moves_and_exactly_one_rollback_step() {
        let empty = SelectionLedger::empty();
        let first = empty.select(&slot(14, 'a')).unwrap();
        assert_eq!(
            (first.current(), first.previous()),
            (Some(slot(14, 'a').as_str()), None)
        );
        assert_eq!(first.select(&slot(14, 'a')).unwrap(), first);
        assert!(first.select(&slot(13, 'b')).is_none());

        let candidate = first.select(&slot(15, 'c')).unwrap();
        assert_eq!(candidate.previous(), Some(slot(14, 'a').as_str()));
        // A same-sequence different build is not a rollback.
        assert!(candidate.select(&slot(15, 'd')).is_some());

        let restored = candidate.select(&slot(14, 'a')).unwrap();
        assert_eq!(
            (restored.current(), restored.previous()),
            (Some(slot(14, 'a').as_str()), None)
        );
        // No second step back, and the rejected candidate is still selectable.
        assert!(restored.select(&slot(13, 'b')).is_none());
        assert!(restored.select(&slot(15, 'c')).is_some());
        assert!(candidate.select(&slot(13, 'b')).is_none());
        assert!(candidate.select("current").is_none());
    }

    #[test]
    fn ledger_store_is_atomic_bounded_and_rejects_foreign_nodes() {
        let root = tempfile::tempdir().unwrap();
        let store = SelectionLedgerStore::fixture(root.path());
        assert_eq!(store.load().unwrap(), SelectionLedger::empty());
        let ledger = SelectionLedger::empty()
            .select(&slot(14, 'a'))
            .unwrap()
            .select(&slot(15, 'b'))
            .unwrap();
        store.store(&ledger).unwrap();
        assert_eq!(store.load().unwrap(), ledger);
        assert!(!root.path().join(LEDGER_TEMPORARY_NAME).exists());

        let invalid = SelectionLedger {
            previous: Some(slot(15, 'b')),
            ..ledger.clone()
        };
        assert!(matches!(
            store.store(&invalid),
            Err(ApplicationStageError::PolicyRejected)
        ));

        let path = root.path().join(LEDGER_FILE_NAME);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o622)).unwrap();
        assert!(matches!(
            store.load(),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::write(
            &path,
            b"{\"schemaVersion\":1,\"current\":null,\"previous\":null,\"x\":1}",
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));
        fs::write(&path, vec![b' '; MAX_LEDGER_BYTES as usize + 1]).unwrap();
        assert!(matches!(
            store.load(),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));

        fs::remove_file(&path).unwrap();
        let outside = root.path().join("outside.json");
        fs::write(&outside, serde_json::to_vec(&ledger).unwrap()).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(matches!(
            store.load(),
            Err(ApplicationStageError::UnsafeExistingSlot)
        ));

        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            store.load(),
            Err(ApplicationStageError::UnsafeRoot)
        ));
        assert!(matches!(
            SelectionLedgerStore::open(root.path()),
            Err(ApplicationStageError::UnsafeRoot)
        ));
    }
}
