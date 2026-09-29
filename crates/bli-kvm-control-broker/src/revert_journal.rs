//! A pending network change that reverts unless confirmed is written here
//! before the change is applied, so an owner restart or a reboot inside the
//! confirmation window cannot leave the unconfirmed change in place: a record
//! found at start means nothing can still be waiting to confirm it.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;

use serde::{Serialize, de::DeserializeOwned};

/// Where the records live, next to the owner's other state.
pub const REVERT_JOURNAL_DIR: &str = "/var/lib/hardened-blikvm-gadget";

#[derive(Debug, Clone)]
pub struct RevertJournal {
    path: PathBuf,
}

impl RevertJournal {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Replaces the record: a file of its own, synced, renamed over the old
    /// one, and the directory synced.
    pub fn write<T: Serialize>(&self, record: &T) -> std::io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("the journal has no directory"))?;
        // Each journal has one writer at a time (its state's own lock).
        let temporary = self.path.with_extension("new");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(&serde_json::to_vec(record).map_err(std::io::Error::other)?)?;
        file.sync_all()?;
        drop(file);
        std::fs::rename(&temporary, &self.path)?;
        std::fs::File::open(parent)?.sync_all()
    }

    /// The record, if there is a readable one. A damaged file is removed and
    /// logged: it cannot say what to restore.
    pub fn read<T: DeserializeOwned>(&self) -> Option<T> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
            Err(error) => {
                eprintln!("network: the revert journal is unreadable: {error}");
                return None;
            }
        };
        match serde_json::from_slice(&bytes) {
            Ok(record) => Some(record),
            Err(error) => {
                eprintln!("network: the revert journal is damaged and was discarded: {error}");
                self.clear();
                None
            }
        }
    }

    pub fn clear(&self) {
        if let Err(error) = std::fs::remove_file(&self.path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            eprintln!("network: the revert journal could not be removed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_record_is_written_read_and_cleared_and_a_damaged_one_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let journal = RevertJournal::new(dir.path().join("wifi.json"));
        assert_eq!(journal.read::<Vec<String>>(), None);
        journal.write(&vec!["a".to_string()]).unwrap();
        assert_eq!(journal.read::<Vec<String>>(), Some(vec!["a".to_string()]));
        journal.clear();
        assert_eq!(journal.read::<Vec<String>>(), None);
        std::fs::write(dir.path().join("wifi.json"), b"{broken").unwrap();
        assert_eq!(journal.read::<Vec<String>>(), None);
        assert!(!dir.path().join("wifi.json").exists());
    }
}
