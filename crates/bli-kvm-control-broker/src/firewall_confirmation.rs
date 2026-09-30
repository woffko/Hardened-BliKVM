//! Operation-bound confirmation state for the native firewall transaction.
//! Callers must durably store a prepared record before changing kernel rules,
//! and clear it only after confirmation or verified rollback has succeeded.

use crate::firewall_rules::FirewallMode;
use crate::firewall_settings::{FirewallRunner, SystemFirewallRunner};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const CONFIRMATION_WINDOW_MS: u64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingChange {
    operation_id: [u8; 16],
    session_binding: [u8; 32],
    boot_id: [u8; 16],
    issued_at_ms: u64,
    deadline_ms: u64,
    previous: FirewallMode,
    requested: FirewallMode,
    phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Prepared,
    Applied,
    RollbackRequired,
}

const MAX_PENDING_BYTES: usize = 4096;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPending {
    schema_version: u8,
    pending: PendingChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmationError {
    InvalidIdentity,
    InvalidMode,
    InvalidDeadline,
    WrongOperation,
    WrongSession,
    NotApplied,
    RollbackRequired,
    InvalidRecord,
    PendingExists,
    NoPending,
    StorageFailure,
}

/// Implementations must bound reads, atomically replace records and sync clears.
/// A missing record is distinct from an unreadable or malformed record.
pub trait PendingStore {
    fn load(&mut self) -> Result<Option<String>, String>;
    fn save(&mut self, record: &str) -> Result<(), String>;
    fn clear(&mut self) -> Result<(), String>;
}

const CLEARED_RECORD: &str = "{\"schemaVersion\":1,\"cleared\":true}";

/// The directory is supplied by the trusted owner configuration, not an API
/// path. Directory replacement by the same privileged owner is out of scope.
pub struct PendingFileStore {
    path: PathBuf,
}

impl PendingFileStore {
    pub fn new(directory: &Path) -> Result<Self, String> {
        if !directory.is_absolute() {
            return Err("pending directory must be absolute".into());
        }
        Ok(Self {
            path: directory.join("pending.json"),
        })
    }

    pub(crate) fn validate_directory(&self) -> Result<(), String> {
        let parent = self.path.parent().ok_or("missing pending directory")?;
        for ancestor in parent.ancestors() {
            match std::fs::symlink_metadata(ancestor) {
                Ok(metadata) if !metadata.is_dir() => {
                    return Err("invalid pending directory".into());
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if metadata.permissions().mode() & 0o022 != 0 => {
                Err("pending directory is writable by others".into())
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        }
    }
}

impl PendingStore for PendingFileStore {
    fn load(&mut self) -> Result<Option<String>, String> {
        self.validate_directory()?;
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file()
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.len() > MAX_PENDING_BYTES as u64
        {
            return Err("invalid pending file".into());
        }
        let mut record = String::new();
        file.take(MAX_PENDING_BYTES as u64 + 1)
            .read_to_string(&mut record)
            .map_err(|error| error.to_string())?;
        if record.len() > MAX_PENDING_BYTES {
            return Err("pending record exceeds bound".into());
        }
        if record == CLEARED_RECORD {
            return Ok(None);
        }
        PendingChange::decode(&record).map_err(|_| "invalid pending record")?;
        Ok(Some(record))
    }

    fn save(&mut self, record: &str) -> Result<(), String> {
        PendingChange::decode(record).map_err(|_| "invalid pending record")?;
        self.validate_directory()?;
        SystemFirewallRunner.write_file(&self.path, record)
    }

    fn clear(&mut self) -> Result<(), String> {
        self.validate_directory()?;
        // Atomic, synced tombstone avoids treating a torn/truncated record as
        // an empty journal, and uses the same durability boundary as save.
        SystemFirewallRunner.write_file(&self.path, CLEARED_RECORD)
    }
}

pub struct PendingJournal<S: PendingStore> {
    store: S,
    pending: Option<PendingChange>,
}

impl<S: PendingStore> PendingJournal<S> {
    pub fn open(mut store: S) -> Result<Self, ConfirmationError> {
        let record = store
            .load()
            .map_err(|_| ConfirmationError::StorageFailure)?;
        let pending = record.as_deref().map(PendingChange::decode).transpose()?;
        Ok(Self { store, pending })
    }

    pub fn pending(&self) -> Option<&PendingChange> {
        self.pending.as_ref()
    }

    /// Persist before kernel application. There is never a second pending slot.
    pub fn stage(&mut self, pending: PendingChange) -> Result<(), ConfirmationError> {
        if self.pending.is_some() {
            return Err(ConfirmationError::PendingExists);
        }
        let record = pending.encode()?;
        // Keep the slot occupied even if persistence reports an ambiguous error
        // after rename. The caller must not apply, and must recover this record.
        self.pending = Some(pending);
        self.store
            .save(&record)
            .map_err(|_| ConfirmationError::StorageFailure)
    }

    pub fn mark_applied(&mut self, operation_id: [u8; 16]) -> Result<(), ConfirmationError> {
        let mut next = self.pending.clone().ok_or(ConfirmationError::NoPending)?;
        next.mark_applied(operation_id)?;
        self.store
            .save(&next.encode()?)
            .map_err(|_| ConfirmationError::StorageFailure)?;
        self.pending = Some(next);
        Ok(())
    }

    pub fn confirm(
        &mut self,
        operation_id: [u8; 16],
        session_binding: [u8; 32],
        boot_id: [u8; 16],
        now_ms: u64,
    ) -> Result<(), ConfirmationError> {
        self.pending
            .as_ref()
            .ok_or(ConfirmationError::NoPending)?
            .can_confirm(operation_id, session_binding, boot_id, now_ms)?;
        self.store
            .clear()
            .map_err(|_| ConfirmationError::StorageFailure)?;
        self.pending = None;
        Ok(())
    }

    pub fn begin_rollback(
        &mut self,
        operation_id: [u8; 16],
    ) -> Result<FirewallMode, ConfirmationError> {
        let pending = self.pending.as_mut().ok_or(ConfirmationError::NoPending)?;
        // Latch in memory before I/O so a failed durable write cannot reopen
        // confirmation while rollback is being retried.
        let previous = pending.begin_rollback(operation_id)?;
        self.store
            .save(&pending.encode()?)
            .map_err(|_| ConfirmationError::StorageFailure)?;
        Ok(previous)
    }

    /// Call only after kernel rollback and read-back have succeeded.
    pub fn finish_rollback(&mut self, operation_id: [u8; 16]) -> Result<(), ConfirmationError> {
        let pending = self.pending.as_ref().ok_or(ConfirmationError::NoPending)?;
        if pending.operation_id != operation_id {
            return Err(ConfirmationError::WrongOperation);
        }
        if pending.phase != Phase::RollbackRequired {
            return Err(ConfirmationError::NotApplied);
        }
        self.store
            .clear()
            .map_err(|_| ConfirmationError::StorageFailure)?;
        self.pending = None;
        Ok(())
    }
}

impl PendingChange {
    pub fn encode(&self) -> Result<String, ConfirmationError> {
        self.validate()?;
        serde_json::to_string(&StoredPending {
            schema_version: 1,
            pending: self.clone(),
        })
        .map_err(|_| ConfirmationError::InvalidRecord)
    }

    pub fn decode(record: &str) -> Result<Self, ConfirmationError> {
        if record.len() > MAX_PENDING_BYTES {
            return Err(ConfirmationError::InvalidRecord);
        }
        let stored: StoredPending =
            serde_json::from_str(record).map_err(|_| ConfirmationError::InvalidRecord)?;
        if stored.schema_version != 1 {
            return Err(ConfirmationError::InvalidRecord);
        }
        stored.pending.validate()?;
        Ok(stored.pending)
    }

    fn validate(&self) -> Result<(), ConfirmationError> {
        if self.operation_id == [0; 16]
            || self.session_binding == [0; 32]
            || self.boot_id == [0; 16]
        {
            return Err(ConfirmationError::InvalidIdentity);
        }
        if self.previous == self.requested {
            return Err(ConfirmationError::InvalidMode);
        }
        if self.issued_at_ms.checked_add(CONFIRMATION_WINDOW_MS) != Some(self.deadline_ms) {
            return Err(ConfirmationError::InvalidDeadline);
        }
        Ok(())
    }
    pub fn prepare(
        operation_id: [u8; 16],
        session_binding: [u8; 32],
        boot_id: [u8; 16],
        previous: FirewallMode,
        requested: FirewallMode,
        now_ms: u64,
    ) -> Result<Self, ConfirmationError> {
        if operation_id == [0; 16] || session_binding == [0; 32] || boot_id == [0; 16] {
            return Err(ConfirmationError::InvalidIdentity);
        }
        if requested == previous {
            return Err(ConfirmationError::InvalidMode);
        }
        let deadline_ms = now_ms
            .checked_add(CONFIRMATION_WINDOW_MS)
            .ok_or(ConfirmationError::InvalidDeadline)?;
        Ok(Self {
            operation_id,
            session_binding,
            boot_id,
            issued_at_ms: now_ms,
            deadline_ms,
            previous,
            requested,
            phase: Phase::Prepared,
        })
    }

    /// Mark only after kernel read-back matches the candidate. A lost response
    /// leaves Prepared pending and must still be recovered conservatively.
    pub fn mark_applied(&mut self, operation_id: [u8; 16]) -> Result<(), ConfirmationError> {
        if operation_id != self.operation_id {
            return Err(ConfirmationError::WrongOperation);
        }
        if self.phase != Phase::Prepared {
            return Err(ConfirmationError::NotApplied);
        }
        self.phase = Phase::Applied;
        Ok(())
    }

    /// This is a read-only authorization decision. Durable deletion belongs to
    /// the transaction owner; an I/O failure must not lose the pending record.
    pub fn can_confirm(
        &self,
        operation_id: [u8; 16],
        session_binding: [u8; 32],
        boot_id: [u8; 16],
        now_ms: u64,
    ) -> Result<(), ConfirmationError> {
        if operation_id != self.operation_id {
            return Err(ConfirmationError::WrongOperation);
        }
        if session_binding != self.session_binding {
            return Err(ConfirmationError::WrongSession);
        }
        if self.rollback_due(boot_id, now_ms) {
            return Err(ConfirmationError::RollbackRequired);
        }
        if self.phase != Phase::Applied {
            return Err(ConfirmationError::NotApplied);
        }
        Ok(())
    }

    /// A restarted boot or backwards clock cannot extend a pending grant.
    pub fn rollback_due(&self, boot_id: [u8; 16], now_ms: u64) -> bool {
        self.phase == Phase::RollbackRequired
            || boot_id != self.boot_id
            || now_ms < self.issued_at_ms
            || now_ms >= self.deadline_ms
    }

    /// Latch rollback before I/O. Failed attempts leave the record retryable;
    /// no later confirmation may cancel a rollback already in progress.
    pub fn begin_rollback(
        &mut self,
        operation_id: [u8; 16],
    ) -> Result<FirewallMode, ConfirmationError> {
        if operation_id != self.operation_id {
            return Err(ConfirmationError::WrongOperation);
        }
        self.phase = Phase::RollbackRequired;
        Ok(self.previous)
    }

    pub fn requested_mode(&self) -> FirewallMode {
        self.requested
    }

    pub fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
    pub fn awaiting_confirmation(&self, boot_id: [u8; 16], now_ms: u64) -> bool {
        self.phase == Phase::Applied && !self.rollback_due(boot_id, now_ms)
    }
    pub fn operation_id(&self) -> [u8; 16] {
        self.operation_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pending() -> PendingChange {
        PendingChange::prepare(
            [1; 16],
            [2; 32],
            [3; 16],
            FirewallMode::Moderate,
            FirewallMode::Restricted,
            100,
        )
        .unwrap()
    }

    #[derive(Default)]
    struct Store {
        record: Option<String>,
        fail_save: bool,
        fail_clear: bool,
    }
    impl PendingStore for Store {
        fn load(&mut self) -> Result<Option<String>, String> {
            Ok(self.record.clone())
        }
        fn save(&mut self, record: &str) -> Result<(), String> {
            if self.fail_save {
                return Err("injected save failure".into());
            }
            self.record = Some(record.to_string());
            Ok(())
        }
        fn clear(&mut self) -> Result<(), String> {
            if self.fail_clear {
                return Err("injected clear failure".into());
            }
            self.record = None;
            Ok(())
        }
    }

    #[test]
    fn journal_keeps_single_slot_and_recovers_persisted_state() {
        let mut journal = PendingJournal::open(Store::default()).unwrap();
        journal.stage(pending()).unwrap();
        assert_eq!(
            journal.stage(pending()),
            Err(ConfirmationError::PendingExists)
        );
        journal.mark_applied([1; 16]).unwrap();
        let mut recovered = PendingJournal::open(journal.store).unwrap();
        assert_eq!(
            recovered.confirm([9; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::WrongOperation)
        );
        recovered.confirm([1; 16], [2; 32], [3; 16], 101).unwrap();
        assert!(recovered.pending().is_none());
        recovered.stage(pending()).unwrap();
    }

    #[test]
    fn journal_io_failure_never_discards_or_reopens_pending_work() {
        let mut journal = PendingJournal::open(Store {
            fail_save: true,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            journal.stage(pending()),
            Err(ConfirmationError::StorageFailure)
        );
        assert_eq!(
            journal.stage(pending()),
            Err(ConfirmationError::PendingExists)
        );
        journal.store.fail_save = false;
        journal.mark_applied([1; 16]).unwrap();
        journal.store.fail_clear = true;
        assert_eq!(
            journal.confirm([1; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::StorageFailure)
        );
        assert!(journal.pending().is_some());
        journal.store.fail_save = true;
        assert_eq!(
            journal.begin_rollback([1; 16]),
            Err(ConfirmationError::StorageFailure)
        );
        assert_eq!(
            journal.confirm([1; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::RollbackRequired)
        );
        journal.store.fail_save = false;
        assert_eq!(journal.begin_rollback([1; 16]), Ok(FirewallMode::Moderate));
        assert_eq!(
            journal.finish_rollback([1; 16]),
            Err(ConfirmationError::StorageFailure)
        );
        assert!(journal.pending().is_some());
        journal.store.fail_clear = false;
        journal.finish_rollback([1; 16]).unwrap();
        assert!(journal.pending().is_none());
    }

    #[test]
    fn only_same_operation_session_and_boot_can_confirm_applied_state() {
        let mut state = pending();
        assert_eq!(
            state.can_confirm([1; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::NotApplied)
        );
        state.mark_applied([1; 16]).unwrap();
        assert!(state.can_confirm([1; 16], [2; 32], [3; 16], 101).is_ok());
        assert_eq!(
            state.can_confirm([4; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::WrongOperation)
        );
        assert_eq!(
            state.can_confirm([1; 16], [4; 32], [3; 16], 101),
            Err(ConfirmationError::WrongSession)
        );
        assert_eq!(
            state.can_confirm([1; 16], [2; 32], [4; 16], 101),
            Err(ConfirmationError::RollbackRequired)
        );
    }

    #[test]
    fn timeout_clock_reversal_and_restart_require_rollback() {
        let mut state = pending();
        state.mark_applied([1; 16]).unwrap();
        assert!(!state.rollback_due([3; 16], 60_099));
        for (boot, now) in [([3; 16], 60_100), ([3; 16], 99), ([4; 16], 101)] {
            assert!(state.rollback_due(boot, now));
            assert_eq!(
                state.can_confirm([1; 16], [2; 32], boot, now),
                Err(ConfirmationError::RollbackRequired)
            );
        }
    }

    #[test]
    fn stale_timer_cannot_rollback_new_operation_and_failure_keeps_retry_state() {
        let mut state = pending();
        assert_eq!(
            state.begin_rollback([9; 16]),
            Err(ConfirmationError::WrongOperation)
        );
        assert!(!state.rollback_due([3; 16], 101));
        assert_eq!(state.begin_rollback([1; 16]), Ok(FirewallMode::Moderate));
        assert!(state.rollback_due([3; 16], 101));
        assert_eq!(
            state.can_confirm([1; 16], [2; 32], [3; 16], 101),
            Err(ConfirmationError::RollbackRequired)
        );
        assert_eq!(state.begin_rollback([1; 16]), Ok(FirewallMode::Moderate));
    }

    #[test]
    fn persisted_phases_retain_confirmation_and_rollback_semantics() {
        let mut state = pending();
        for phase in [Phase::Prepared, Phase::Applied, Phase::RollbackRequired] {
            state.phase = phase;
            let decoded = PendingChange::decode(&state.encode().unwrap()).unwrap();
            assert_eq!(decoded, state);
            assert_eq!(
                decoded.can_confirm([1; 16], [2; 32], [3; 16], 101).is_ok(),
                phase == Phase::Applied
            );
            assert!(decoded.rollback_due([4; 16], 101));
        }
    }

    #[test]
    fn malformed_or_ambiguous_pending_records_are_refused() {
        let record = pending().encode().unwrap();
        for (field, value) in [
            ("deadlineMs", serde_json::json!(999999)),
            ("operationId", serde_json::json!(vec![0; 16])),
            ("requested", serde_json::json!("moderate")),
            ("phase", serde_json::json!("confirmed")),
            ("extra", serde_json::json!(true)),
        ] {
            let mut data: serde_json::Value = serde_json::from_str(&record).unwrap();
            data["pending"][field] = value;
            assert!(PendingChange::decode(&data.to_string()).is_err(), "{field}");
        }
        assert!(
            PendingChange::decode(&record.replace("\"schemaVersion\":1", "\"schemaVersion\":2"))
                .is_err()
        );
        assert!(
            PendingChange::decode(&record.replace(
                "\"schemaVersion\":1",
                "\"schemaVersion\":1,\"schemaVersion\":1"
            ))
            .is_err()
        );
        assert!(PendingChange::decode(&" ".repeat(MAX_PENDING_BYTES + 1)).is_err());
    }

    #[test]
    fn file_journal_recovers_and_clears_with_a_private_tombstone() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("journal");
        let mut journal = PendingJournal::open(PendingFileStore::new(&directory).unwrap()).unwrap();
        journal.stage(pending()).unwrap();
        journal.mark_applied([1; 16]).unwrap();
        drop(journal);
        let mut journal = PendingJournal::open(PendingFileStore::new(&directory).unwrap()).unwrap();
        journal.confirm([1; 16], [2; 32], [3; 16], 101).unwrap();
        drop(journal);
        assert!(
            PendingJournal::open(PendingFileStore::new(&directory).unwrap())
                .unwrap()
                .pending()
                .is_none()
        );
        let path = directory.join("pending.json");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), CLEARED_RECORD);
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn file_journal_rejects_corruption_permissions_and_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let mut store = PendingFileStore::new(root.path()).unwrap();
        store.save(&pending().encode().unwrap()).unwrap();
        let path = root.path().join("pending.json");
        for bad in [
            String::new(),
            "{}".into(),
            "x".repeat(MAX_PENDING_BYTES + 1),
        ] {
            std::fs::write(&path, bad).unwrap();
            assert!(store.load().is_err());
        }
        store.save(&pending().encode().unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(store.load().is_err());
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(root.path().join("missing"), &path).unwrap();
        assert!(store.load().is_err());
        assert!(store.save(&pending().encode().unwrap()).is_err());
        assert!(store.clear().is_err());
    }

    #[test]
    fn zero_identity_and_overflow_are_rejected() {
        assert_eq!(
            PendingChange::prepare(
                [0; 16],
                [2; 32],
                [3; 16],
                FirewallMode::Moderate,
                FirewallMode::Paranoid,
                0
            ),
            Err(ConfirmationError::InvalidIdentity)
        );
        assert_eq!(
            PendingChange::prepare(
                [1; 16],
                [2; 32],
                [3; 16],
                FirewallMode::Moderate,
                FirewallMode::Paranoid,
                u64::MAX
            ),
            Err(ConfirmationError::InvalidDeadline)
        );
    }
}
