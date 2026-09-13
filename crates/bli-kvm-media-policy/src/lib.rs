use std::{
    fs::{self, File, Permissions},
    io::{Read, Write},
    os::unix::fs::{FileTypeExt, PermissionsExt},
    path::{Path, PathBuf},
};

use bli_kvm_core::VirtualMediaCommand;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const JOURNAL_SCHEMA_VERSION: u16 = 1;
const MAX_JOURNAL_BYTES: u64 = 128 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_CONFIRMATION_AGE_MS: u64 = 30_000;
const MUTATION_COOLDOWN_MS: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaOperation {
    operation_id: [u8; 16],
    session_binding: u64,
    confirmation_binding: [u8; 32],
    confirmation_issued_at_ms: u64,
    command: VirtualMediaCommand,
}

impl MediaOperation {
    pub fn new(
        operation_id: [u8; 16],
        session_binding: u64,
        confirmation_binding: [u8; 32],
        confirmation_issued_at_ms: u64,
        command: VirtualMediaCommand,
    ) -> Result<Self, MediaPolicyError> {
        if operation_id.iter().all(|byte| *byte == 0)
            || session_binding == 0
            || confirmation_binding.iter().all(|byte| *byte == 0)
            || confirmation_issued_at_ms == 0
            || command.validate().is_err()
        {
            return Err(MediaPolicyError::InvalidOperation);
        }
        Ok(Self {
            operation_id,
            session_binding,
            confirmation_binding,
            confirmation_issued_at_ms,
            command,
        })
    }

    pub const fn operation_id(self) -> [u8; 16] {
        self.operation_id
    }

    pub const fn command(self) -> VirtualMediaCommand {
        self.command
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaObservedState {
    pub enabled: bool,
    pub online: bool,
    pub busy: bool,
    pub connected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaEligibility {
    capability_enabled: bool,
    backend_ready: bool,
    confirmation_verified: bool,
}

impl MediaEligibility {
    pub const fn disabled() -> Self {
        Self {
            capability_enabled: false,
            backend_ready: false,
            confirmation_verified: false,
        }
    }

    #[cfg(feature = "media-test-profile")]
    pub const fn verified_for_test_profile() -> Self {
        Self::verified()
    }

    #[cfg(test)]
    const fn verified_for_test() -> Self {
        Self::verified()
    }

    #[cfg(any(test, feature = "media-test-profile"))]
    const fn verified() -> Self {
        Self {
            capability_enabled: true,
            backend_ready: true,
            confirmation_verified: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDecision {
    Permit,
    Noop,
    ReplayApplied,
    ReplayRejected,
    Reject(MediaRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaRejection {
    CapabilityDisabled,
    BackendUnavailable,
    ConfirmationInvalid,
    ConfirmationExpired,
    OperationIdConflict,
    IndeterminatePending,
    Busy,
    MustDisconnect,
    CooldownActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCompletion {
    Applied,
    Rejected,
}

#[derive(Debug, Error)]
pub enum MediaPolicyError {
    #[error("media operation is invalid")]
    InvalidOperation,
    #[error("media journal path is invalid")]
    InvalidPath,
    #[error("media journal directory is unsafe")]
    UnsafeDirectory,
    #[error("media journal file is unsafe")]
    UnsafeFile,
    #[error("media journal is too large")]
    TooLarge,
    #[error("media journal is invalid")]
    InvalidJournal,
    #[error("media operation is not pending")]
    NotPending,
    #[error("media journal I/O failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
pub struct MediaPolicyEngine {
    journal: MediaJournal,
    document: JournalDocument,
}

impl MediaPolicyEngine {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, MediaPolicyError> {
        let journal = MediaJournal::new(path.into())?;
        let document = journal.load()?;
        Ok(Self { journal, document })
    }

    pub fn prepare(
        &mut self,
        operation: MediaOperation,
        eligibility: MediaEligibility,
        observed: MediaObservedState,
        now_ms: u64,
    ) -> Result<MediaDecision, MediaPolicyError> {
        let fingerprint = OperationFingerprint::from(operation);
        if let Some(existing) = self
            .document
            .records
            .iter()
            .find(|record| record.operation_id == operation.operation_id)
        {
            if existing.fingerprint != fingerprint {
                return Ok(MediaDecision::Reject(MediaRejection::OperationIdConflict));
            }
            return Ok(match existing.status {
                OperationStatus::Pending => {
                    MediaDecision::Reject(MediaRejection::IndeterminatePending)
                }
                OperationStatus::Applied => MediaDecision::ReplayApplied,
                OperationStatus::Rejected => MediaDecision::ReplayRejected,
            });
        }
        if !eligibility.capability_enabled {
            return Ok(MediaDecision::Reject(MediaRejection::CapabilityDisabled));
        }
        if !eligibility.backend_ready || !observed.enabled || !observed.online {
            return Ok(MediaDecision::Reject(MediaRejection::BackendUnavailable));
        }
        if !eligibility.confirmation_verified {
            return Ok(MediaDecision::Reject(MediaRejection::ConfirmationInvalid));
        }
        if now_ms < operation.confirmation_issued_at_ms
            || now_ms.saturating_sub(operation.confirmation_issued_at_ms) > MAX_CONFIRMATION_AGE_MS
        {
            return Ok(MediaDecision::Reject(MediaRejection::ConfirmationExpired));
        }
        if observed.busy {
            return Ok(MediaDecision::Reject(MediaRejection::Busy));
        }
        if self
            .document
            .records
            .iter()
            .any(|record| record.status == OperationStatus::Pending)
        {
            return Ok(MediaDecision::Reject(MediaRejection::IndeterminatePending));
        }
        match operation.command {
            VirtualMediaCommand::Select { .. } if observed.connected => {
                return Ok(MediaDecision::Reject(MediaRejection::MustDisconnect));
            }
            VirtualMediaCommand::SetConnected { connected } if connected == observed.connected => {
                return Ok(MediaDecision::Noop);
            }
            VirtualMediaCommand::Select { .. } | VirtualMediaCommand::SetConnected { .. } => {}
        }
        if self.document.records.iter().any(|record| {
            record.status == OperationStatus::Applied
                && record
                    .completed_at_ms
                    .is_none_or(|completed| now_ms < completed.saturating_add(MUTATION_COOLDOWN_MS))
        }) {
            return Ok(MediaDecision::Reject(MediaRejection::CooldownActive));
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
        Ok(MediaDecision::Permit)
    }

    pub fn complete(
        &mut self,
        operation_id: [u8; 16],
        completion: MediaCompletion,
        now_ms: u64,
    ) -> Result<(), MediaPolicyError> {
        let record = self
            .document
            .records
            .iter_mut()
            .find(|record| record.operation_id == operation_id)
            .ok_or(MediaPolicyError::NotPending)?;
        if record.status != OperationStatus::Pending {
            return Err(MediaPolicyError::NotPending);
        }
        if now_ms < record.prepared_at_ms {
            return Err(MediaPolicyError::InvalidOperation);
        }
        record.status = match completion {
            MediaCompletion::Applied => OperationStatus::Applied,
            MediaCompletion::Rejected => OperationStatus::Rejected,
        };
        record.completed_at_ms = Some(now_ms);
        self.journal.store(&self.document)
    }

    pub fn resolve_pending(
        &mut self,
        operation_id: [u8; 16],
        completion: MediaCompletion,
        reconciled_at_ms: u64,
    ) -> Result<(), MediaPolicyError> {
        self.complete(operation_id, completion, reconciled_at_ms)
    }

    pub fn pending_count(&self) -> usize {
        self.document
            .records
            .iter()
            .filter(|record| record.status == OperationStatus::Pending)
            .count()
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct OperationFingerprint {
    session_binding: u64,
    confirmation_binding: [u8; 32],
    command: VirtualMediaCommand,
}

impl From<MediaOperation> for OperationFingerprint {
    fn from(operation: MediaOperation) -> Self {
        Self {
            session_binding: operation.session_binding,
            confirmation_binding: operation.confirmation_binding,
            command: operation.command,
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
struct MediaJournal {
    path: PathBuf,
}

impl MediaJournal {
    fn new(path: PathBuf) -> Result<Self, MediaPolicyError> {
        if !path.is_absolute() || path.file_name().is_none() {
            return Err(MediaPolicyError::InvalidPath);
        }
        validate_directory(path.parent().ok_or(MediaPolicyError::InvalidPath)?)?;
        let _ = validate_existing_file(&path)?;
        Ok(Self { path })
    }

    fn load(&self) -> Result<JournalDocument, MediaPolicyError> {
        if !validate_existing_file(&self.path)? {
            return Ok(JournalDocument::default());
        }
        let metadata = fs::symlink_metadata(&self.path)?;
        if metadata.len() > MAX_JOURNAL_BYTES {
            return Err(MediaPolicyError::TooLarge);
        }
        let mut file = File::open(&self.path)?;
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        Read::by_ref(&mut file)
            .take(MAX_JOURNAL_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(MediaPolicyError::TooLarge);
        }
        let document: JournalDocument =
            serde_json::from_slice(&bytes).map_err(|_| MediaPolicyError::InvalidJournal)?;
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
                    || record.fingerprint.command.validate().is_err()
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
            return Err(MediaPolicyError::InvalidJournal);
        }
        Ok(document)
    }

    fn store(&self, document: &JournalDocument) -> Result<(), MediaPolicyError> {
        validate_directory(self.path.parent().ok_or(MediaPolicyError::InvalidPath)?)?;
        let _ = validate_existing_file(&self.path)?;
        let bytes = serde_json::to_vec(document).map_err(|_| MediaPolicyError::InvalidJournal)?;
        if bytes.len() as u64 > MAX_JOURNAL_BYTES {
            return Err(MediaPolicyError::TooLarge);
        }
        let parent = self.path.parent().ok_or(MediaPolicyError::InvalidPath)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        fs::set_permissions(temporary.path(), Permissions::from_mode(0o600))?;
        temporary.write_all(&bytes)?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&self.path)
            .map_err(|error| MediaPolicyError::Io(error.error))?;
        File::open(parent)?.sync_all()?;
        validate_file(&self.path)?;
        Ok(())
    }
}

fn validate_directory(path: &Path) -> Result<(), MediaPolicyError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.permissions().mode() & 0o022 != 0
    {
        return Err(MediaPolicyError::UnsafeDirectory);
    }
    Ok(())
}

fn validate_file(path: &Path) -> Result<(), MediaPolicyError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err(MediaPolicyError::UnsafeFile);
    }
    Ok(())
}

fn validate_existing_file(path: &Path) -> Result<bool, MediaPolicyError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_file(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(MediaPolicyError::Io(error)),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    fn observed(connected: bool) -> MediaObservedState {
        MediaObservedState {
            enabled: true,
            online: true,
            busy: false,
            connected,
        }
    }

    fn operation(seed: u8, command: VirtualMediaCommand, confirmed_at: u64) -> MediaOperation {
        MediaOperation::new(
            [seed; 16],
            u64::from(seed),
            [seed; 32],
            confirmed_at,
            command,
        )
        .unwrap()
    }

    fn engine(directory: &Path) -> MediaPolicyEngine {
        MediaPolicyEngine::open(directory.join("media-journal.json")).unwrap()
    }

    #[test]
    fn public_policy_is_disabled_and_same_connection_state_is_noop_internally() {
        let directory = tempfile::tempdir().unwrap();
        let request = operation(
            1,
            VirtualMediaCommand::SetConnected { connected: false },
            1_000,
        );
        assert_eq!(
            engine(directory.path())
                .prepare(
                    request,
                    MediaEligibility::disabled(),
                    observed(false),
                    1_001
                )
                .unwrap(),
            MediaDecision::Reject(MediaRejection::CapabilityDisabled)
        );
        assert_eq!(
            engine(directory.path())
                .prepare(
                    request,
                    MediaEligibility::verified_for_test(),
                    observed(false),
                    1_001
                )
                .unwrap(),
            MediaDecision::Noop
        );
    }

    #[test]
    fn selection_requires_disconnected_idle_online_state() {
        let directory = tempfile::tempdir().unwrap();
        let request = operation(
            1,
            VirtualMediaCommand::Select {
                image_id: 7,
                mode: bli_kvm_core::VirtualMediaControlMode::CdRom,
                writable: false,
            },
            1_000,
        );
        assert_eq!(
            engine(directory.path())
                .prepare(
                    request,
                    MediaEligibility::verified_for_test(),
                    observed(true),
                    1_001
                )
                .unwrap(),
            MediaDecision::Reject(MediaRejection::MustDisconnect)
        );
        let mut busy = observed(false);
        busy.busy = true;
        assert_eq!(
            engine(directory.path())
                .prepare(request, MediaEligibility::verified_for_test(), busy, 1_001)
                .unwrap(),
            MediaDecision::Reject(MediaRejection::Busy)
        );
    }

    #[test]
    fn pending_survives_restart_and_duplicate_applied_is_not_repeated() {
        let directory = tempfile::tempdir().unwrap();
        let request = operation(
            1,
            VirtualMediaCommand::SetConnected { connected: true },
            10_000,
        );
        {
            let mut first = engine(directory.path());
            assert_eq!(
                first
                    .prepare(
                        request,
                        MediaEligibility::verified_for_test(),
                        observed(false),
                        10_001
                    )
                    .unwrap(),
                MediaDecision::Permit
            );
        }
        let mut restarted = engine(directory.path());
        assert_eq!(restarted.pending_count(), 1);
        assert_eq!(
            restarted
                .prepare(
                    request,
                    MediaEligibility::disabled(),
                    observed(false),
                    99_999
                )
                .unwrap(),
            MediaDecision::Reject(MediaRejection::IndeterminatePending)
        );
        assert_eq!(
            restarted
                .prepare(
                    operation(
                        2,
                        VirtualMediaCommand::SetConnected { connected: false },
                        10_000
                    ),
                    MediaEligibility::verified_for_test(),
                    observed(false),
                    10_002
                )
                .unwrap(),
            MediaDecision::Reject(MediaRejection::IndeterminatePending)
        );
        restarted
            .resolve_pending([1; 16], MediaCompletion::Applied, 100_000)
            .unwrap();
        assert_eq!(
            restarted
                .prepare(
                    request,
                    MediaEligibility::disabled(),
                    observed(true),
                    200_000
                )
                .unwrap(),
            MediaDecision::ReplayApplied
        );
    }

    #[test]
    fn completed_mutation_enforces_short_global_cooldown() {
        let directory = tempfile::tempdir().unwrap();
        let mut policy = engine(directory.path());
        let first = operation(
            1,
            VirtualMediaCommand::SetConnected { connected: true },
            10_000,
        );
        assert_eq!(
            policy
                .prepare(
                    first,
                    MediaEligibility::verified_for_test(),
                    observed(false),
                    10_001
                )
                .unwrap(),
            MediaDecision::Permit
        );
        policy
            .complete([1; 16], MediaCompletion::Applied, 10_100)
            .unwrap();
        assert_eq!(
            policy
                .prepare(
                    operation(
                        2,
                        VirtualMediaCommand::SetConnected { connected: false },
                        10_200
                    ),
                    MediaEligibility::verified_for_test(),
                    observed(true),
                    10_201
                )
                .unwrap(),
            MediaDecision::Reject(MediaRejection::CooldownActive)
        );
    }

    #[test]
    fn journal_rejects_relative_unsafe_and_dangling_paths() {
        assert!(matches!(
            MediaPolicyEngine::open("journal.json"),
            Err(MediaPolicyError::InvalidPath)
        ));
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(directory.path(), Permissions::from_mode(0o770)).unwrap();
        assert!(matches!(
            MediaPolicyEngine::open(directory.path().join("journal.json")),
            Err(MediaPolicyError::UnsafeDirectory)
        ));
        fs::set_permissions(directory.path(), Permissions::from_mode(0o700)).unwrap();
        let dangling = directory.path().join("dangling.json");
        symlink(directory.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            MediaPolicyEngine::open(dangling),
            Err(MediaPolicyError::UnsafeFile)
        ));
    }
}
