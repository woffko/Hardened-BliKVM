use std::{
    fs::{self, File, Permissions},
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_core::AtxControlAction;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const JOURNAL_SCHEMA_VERSION: u16 = 1;
const MAX_JOURNAL_BYTES: u64 = 128 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_CONFIRMATION_AGE_MS: u64 = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtxOperation {
    operation_id: [u8; 16],
    session_binding: u64,
    confirmation_binding: [u8; 32],
    confirmation_issued_at_ms: u64,
    action: AtxControlAction,
}

impl AtxOperation {
    pub fn new(
        operation_id: [u8; 16],
        session_binding: u64,
        confirmation_binding: [u8; 32],
        confirmation_issued_at_ms: u64,
        action: AtxControlAction,
    ) -> Result<Self, AtxPolicyError> {
        if operation_id.iter().all(|byte| *byte == 0)
            || session_binding == 0
            || confirmation_binding.iter().all(|byte| *byte == 0)
            || confirmation_issued_at_ms == 0
        {
            return Err(AtxPolicyError::InvalidOperation);
        }
        Ok(Self {
            operation_id,
            session_binding,
            confirmation_binding,
            confirmation_issued_at_ms,
            action,
        })
    }

    pub const fn action(self) -> AtxControlAction {
        self.action
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtxEligibility {
    capability_enabled: bool,
    backend_ready: bool,
    physical_actions_allowed: bool,
    confirmation_verified: bool,
}

impl AtxEligibility {
    pub const fn disabled() -> Self {
        Self {
            capability_enabled: false,
            backend_ready: false,
            physical_actions_allowed: false,
            confirmation_verified: false,
        }
    }

    #[cfg(test)]
    const fn verified_for_test() -> Self {
        Self {
            capability_enabled: true,
            backend_ready: true,
            physical_actions_allowed: true,
            confirmation_verified: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtxDecision {
    Permit,
    ReplayApplied,
    ReplayRejected,
    Reject(AtxRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtxRejection {
    CapabilityDisabled,
    BackendUnavailable,
    PhysicalActionsDisabled,
    ConfirmationInvalid,
    ConfirmationExpired,
    OperationIdConflict,
    IndeterminatePending,
    CooldownActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtxCompletion {
    Applied,
    Rejected,
}

#[derive(Debug, Error)]
pub enum AtxPolicyError {
    #[error("ATX operation is invalid")]
    InvalidOperation,
    #[error("ATX journal path is invalid")]
    InvalidPath,
    #[error("ATX journal directory is unsafe")]
    UnsafeDirectory,
    #[error("ATX journal file is unsafe")]
    UnsafeFile,
    #[error("ATX journal is too large")]
    TooLarge,
    #[error("ATX journal is invalid")]
    InvalidJournal,
    #[error("ATX operation is not pending")]
    NotPending,
    #[error("ATX journal I/O failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct AtxPolicyEngine {
    journal: AtxJournal,
    document: JournalDocument,
}

impl AtxPolicyEngine {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, AtxPolicyError> {
        let journal = AtxJournal::new(path.into())?;
        let document = journal.load()?;
        Ok(Self { journal, document })
    }

    pub fn prepare(
        &mut self,
        operation: AtxOperation,
        eligibility: AtxEligibility,
        now_ms: u64,
    ) -> Result<AtxDecision, AtxPolicyError> {
        let fingerprint = OperationFingerprint::from(operation);
        if let Some(existing) = self
            .document
            .records
            .iter()
            .find(|record| record.operation_id == operation.operation_id)
        {
            if existing.fingerprint != fingerprint {
                return Ok(AtxDecision::Reject(AtxRejection::OperationIdConflict));
            }
            return Ok(match existing.status {
                OperationStatus::Pending => AtxDecision::Reject(AtxRejection::IndeterminatePending),
                OperationStatus::Applied => AtxDecision::ReplayApplied,
                OperationStatus::Rejected => AtxDecision::ReplayRejected,
            });
        }
        if !eligibility.capability_enabled {
            return Ok(AtxDecision::Reject(AtxRejection::CapabilityDisabled));
        }
        if !eligibility.backend_ready {
            return Ok(AtxDecision::Reject(AtxRejection::BackendUnavailable));
        }
        if !eligibility.physical_actions_allowed {
            return Ok(AtxDecision::Reject(AtxRejection::PhysicalActionsDisabled));
        }
        if !eligibility.confirmation_verified {
            return Ok(AtxDecision::Reject(AtxRejection::ConfirmationInvalid));
        }
        if now_ms < operation.confirmation_issued_at_ms
            || now_ms.saturating_sub(operation.confirmation_issued_at_ms) > MAX_CONFIRMATION_AGE_MS
        {
            return Ok(AtxDecision::Reject(AtxRejection::ConfirmationExpired));
        }
        if self
            .document
            .records
            .iter()
            .any(|record| record.status == OperationStatus::Pending)
        {
            return Ok(AtxDecision::Reject(AtxRejection::IndeterminatePending));
        }
        if self.cooldown_active(operation.action, now_ms) {
            return Ok(AtxDecision::Reject(AtxRejection::CooldownActive));
        }
        self.document.records.push(OperationRecord {
            operation_id: operation.operation_id,
            fingerprint,
            status: OperationStatus::Pending,
            prepared_at_ms: now_ms,
            completed_at_ms: None,
        });
        self.prune();
        self.journal.store(&self.document)?;
        Ok(AtxDecision::Permit)
    }

    pub fn complete(
        &mut self,
        operation_id: [u8; 16],
        completion: AtxCompletion,
        now_ms: u64,
    ) -> Result<(), AtxPolicyError> {
        let record = self
            .document
            .records
            .iter_mut()
            .find(|record| record.operation_id == operation_id)
            .ok_or(AtxPolicyError::NotPending)?;
        if record.status != OperationStatus::Pending {
            return Err(AtxPolicyError::NotPending);
        }
        if now_ms < record.prepared_at_ms {
            return Err(AtxPolicyError::InvalidOperation);
        }
        record.status = match completion {
            AtxCompletion::Applied => OperationStatus::Applied,
            AtxCompletion::Rejected => OperationStatus::Rejected,
        };
        record.completed_at_ms = Some(now_ms);
        self.journal.store(&self.document)
    }

    pub fn resolve_pending(
        &mut self,
        operation_id: [u8; 16],
        completion: AtxCompletion,
        reconciled_at_ms: u64,
    ) -> Result<(), AtxPolicyError> {
        self.complete(operation_id, completion, reconciled_at_ms)
    }

    pub fn pending_count(&self) -> usize {
        self.document
            .records
            .iter()
            .filter(|record| record.status == OperationStatus::Pending)
            .count()
    }

    fn cooldown_active(&self, action: AtxControlAction, now_ms: u64) -> bool {
        self.document.records.iter().any(|record| {
            if record.status != OperationStatus::Applied {
                return false;
            }
            let Some(completed_at_ms) = record.completed_at_ms else {
                return true;
            };
            let cooldown =
                action_cooldown_ms(record.fingerprint.action).max(action_cooldown_ms(action));
            now_ms < completed_at_ms.saturating_add(cooldown)
        })
    }

    fn prune(&mut self) {
        while self.document.records.len() > MAX_RECORDS {
            if let Some(index) = self
                .document
                .records
                .iter()
                .position(|record| record.status != OperationStatus::Pending)
            {
                self.document.records.remove(index);
            } else {
                break;
            }
        }
    }
}

fn action_cooldown_ms(action: AtxControlAction) -> u64 {
    match action {
        AtxControlAction::PowerShort => 5_000,
        AtxControlAction::PowerLong | AtxControlAction::Reset => 30_000,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct OperationFingerprint {
    session_binding: u64,
    confirmation_binding: [u8; 32],
    action: AtxControlAction,
}

impl From<AtxOperation> for OperationFingerprint {
    fn from(operation: AtxOperation) -> Self {
        Self {
            session_binding: operation.session_binding,
            confirmation_binding: operation.confirmation_binding,
            action: operation.action,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OperationStatus {
    Pending,
    Applied,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationRecord {
    operation_id: [u8; 16],
    fingerprint: OperationFingerprint,
    status: OperationStatus,
    prepared_at_ms: u64,
    completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JournalDocument {
    schema_version: u16,
    records: Vec<OperationRecord>,
}

impl Default for JournalDocument {
    fn default() -> Self {
        Self {
            schema_version: JOURNAL_SCHEMA_VERSION,
            records: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct AtxJournal {
    path: PathBuf,
}

impl AtxJournal {
    fn new(path: PathBuf) -> Result<Self, AtxPolicyError> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(AtxPolicyError::InvalidPath);
        }
        validate_directory(path.parent().ok_or(AtxPolicyError::InvalidPath)?)?;
        let _ = validate_existing_file(&path)?;
        Ok(Self { path })
    }

    fn load(&self) -> Result<JournalDocument, AtxPolicyError> {
        if !validate_existing_file(&self.path)? {
            return Ok(JournalDocument::default());
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(AtxPolicyError::TooLarge);
        }
        let mut file = File::open(&self.path)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(AtxPolicyError::TooLarge);
        }
        let document: JournalDocument =
            serde_json::from_slice(&bytes).map_err(|_| AtxPolicyError::InvalidJournal)?;
        if document.schema_version != JOURNAL_SCHEMA_VERSION
            || document.records.len() > MAX_RECORDS
            || document.records.iter().enumerate().any(|(index, record)| {
                record.operation_id.iter().all(|byte| *byte == 0)
                    || record.fingerprint.session_binding == 0
                    || record
                        .fingerprint
                        .confirmation_binding
                        .iter()
                        .all(|byte| *byte == 0)
                    || record.prepared_at_ms == 0
                    || match record.status {
                        OperationStatus::Pending => record.completed_at_ms.is_some(),
                        OperationStatus::Applied | OperationStatus::Rejected => record
                            .completed_at_ms
                            .is_none_or(|completed| completed < record.prepared_at_ms),
                    }
                    || document.records[index + 1..]
                        .iter()
                        .any(|other| other.operation_id == record.operation_id)
            })
        {
            return Err(AtxPolicyError::InvalidJournal);
        }
        Ok(document)
    }

    fn store(&self, document: &JournalDocument) -> Result<(), AtxPolicyError> {
        validate_directory(self.path.parent().ok_or(AtxPolicyError::InvalidPath)?)?;
        let _ = validate_existing_file(&self.path)?;
        let bytes = serde_json::to_vec(document).map_err(|_| AtxPolicyError::InvalidJournal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(AtxPolicyError::TooLarge);
        }
        let parent = self.path.parent().ok_or(AtxPolicyError::InvalidPath)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| AtxPolicyError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        validate_file(&self.path)?;
        Ok(())
    }
}

fn validate_directory(path: &Path) -> Result<(), AtxPolicyError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(AtxPolicyError::UnsafeDirectory);
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), AtxPolicyError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(AtxPolicyError::UnsafeFile);
    }
    Ok(())
}

fn validate_existing_file(path: &Path) -> Result<bool, AtxPolicyError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_file(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(AtxPolicyError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn operation(seed: u8, action: AtxControlAction, confirmed_at: u64) -> AtxOperation {
        AtxOperation::new(
            [seed; 16],
            u64::from(seed),
            [seed; 32],
            confirmed_at,
            action,
        )
        .unwrap()
    }

    fn open_engine(directory: &Path) -> AtxPolicyEngine {
        AtxPolicyEngine::open(directory.join("atx-journal.json")).unwrap()
    }

    #[test]
    fn public_eligibility_is_denied_and_does_not_create_pending_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = open_engine(directory.path());
        assert_eq!(
            engine
                .prepare(
                    operation(1, AtxControlAction::PowerShort, 1_000),
                    AtxEligibility::disabled(),
                    1_001
                )
                .unwrap(),
            AtxDecision::Reject(AtxRejection::CapabilityDisabled)
        );
        assert_eq!(engine.pending_count(), 0);
        assert!(!directory.path().join("atx-journal.json").exists());
    }

    #[test]
    fn pending_survives_restart_and_is_never_replayed_automatically() {
        let directory = tempfile::tempdir().unwrap();
        let request = operation(1, AtxControlAction::PowerShort, 1_000);
        {
            let mut engine = open_engine(directory.path());
            assert_eq!(
                engine
                    .prepare(request, AtxEligibility::verified_for_test(), 1_001)
                    .unwrap(),
                AtxDecision::Permit
            );
        }
        let mut restarted = open_engine(directory.path());
        assert_eq!(restarted.pending_count(), 1);
        assert_eq!(
            restarted
                .prepare(request, AtxEligibility::verified_for_test(), 1_002)
                .unwrap(),
            AtxDecision::Reject(AtxRejection::IndeterminatePending)
        );
        restarted
            .resolve_pending([1; 16], AtxCompletion::Rejected, 1_003)
            .unwrap();
        assert_eq!(restarted.pending_count(), 0);
    }

    #[test]
    fn applied_operation_replays_result_and_enforces_global_cooldown() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = open_engine(directory.path());
        let first = operation(1, AtxControlAction::PowerShort, 100_000);
        assert_eq!(
            engine
                .prepare(first, AtxEligibility::verified_for_test(), 100_001)
                .unwrap(),
            AtxDecision::Permit
        );
        engine
            .complete([1; 16], AtxCompletion::Applied, 100_100)
            .unwrap();
        assert_eq!(
            engine
                .prepare(first, AtxEligibility::disabled(), 200_000)
                .unwrap(),
            AtxDecision::ReplayApplied
        );
        assert_eq!(
            engine
                .prepare(
                    operation(2, AtxControlAction::Reset, 100_200),
                    AtxEligibility::verified_for_test(),
                    100_201
                )
                .unwrap(),
            AtxDecision::Reject(AtxRejection::CooldownActive)
        );
        assert_eq!(
            engine
                .prepare(
                    operation(2, AtxControlAction::Reset, 130_100),
                    AtxEligibility::verified_for_test(),
                    130_100
                )
                .unwrap(),
            AtxDecision::Permit
        );
    }

    #[test]
    fn operation_conflicts_and_expired_confirmations_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let mut engine = open_engine(directory.path());
        let first = operation(1, AtxControlAction::PowerShort, 1_000);
        assert_eq!(
            engine
                .prepare(first, AtxEligibility::verified_for_test(), 1_001)
                .unwrap(),
            AtxDecision::Permit
        );
        assert_eq!(
            engine
                .prepare(
                    operation(1, AtxControlAction::Reset, 1_000),
                    AtxEligibility::verified_for_test(),
                    1_002
                )
                .unwrap(),
            AtxDecision::Reject(AtxRejection::OperationIdConflict)
        );
        let other_directory = tempfile::tempdir().unwrap();
        assert_eq!(
            open_engine(other_directory.path())
                .prepare(
                    operation(2, AtxControlAction::Reset, 1_000),
                    AtxEligibility::verified_for_test(),
                    11_001
                )
                .unwrap(),
            AtxDecision::Reject(AtxRejection::ConfirmationExpired)
        );
    }

    #[test]
    fn journal_rejects_unsafe_paths_and_dangling_symlinks() {
        assert!(matches!(
            AtxPolicyEngine::open("journal.json"),
            Err(AtxPolicyError::InvalidPath)
        ));
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            AtxPolicyEngine::open(directory.path().join("journal.json")),
            Err(AtxPolicyError::UnsafeDirectory)
        ));
        fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        let dangling = directory.path().join("dangling.json");
        symlink(directory.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            AtxPolicyEngine::open(dangling),
            Err(AtxPolicyError::UnsafeFile)
        ));
    }
}
