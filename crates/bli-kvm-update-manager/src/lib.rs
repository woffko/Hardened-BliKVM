mod journal;
mod model;
mod pointer;

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
};

use bli_kvm_update_stager::{
    ApplicationSlotStager, ApplicationStageError, StagedApplication, StagingCleanup,
};
use bli_kvm_update_verifier::ApplicationTrustRoot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use journal::StateStore;
use model::{
    JournalDocument, OperationOutcome, PendingActivation, PendingPhase, SlotReference, encode_hex,
};
pub use pointer::PointerStore;

pub const DEFAULT_HEALTH_TIMEOUT_MS: u64 = 60_000;
pub const MINIMUM_HEALTH_TIMEOUT_MS: u64 = 10_000;
pub const MAXIMUM_HEALTH_TIMEOUT_MS: u64 = 10 * 60_000;
pub const MAX_UNREFERENCED_SLOT_RETENTION: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackReason {
    InterruptedPrepare,
    BootChanged,
    HealthTimeout,
    ExplicitHealthFailure,
    PointerMismatch,
    TargetInvalid,
    HealthChallengeLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockSnapshot {
    pub boot_id: [u8; 16],
    pub monotonic_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationRequest {
    pub operation_id: [u8; 16],
    pub target_slot_id: String,
    pub health_binding: [u8; 32],
    pub clock: ClockSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationOutcome {
    AwaitingHealth {
        operation_id: [u8; 16],
        slot_id: String,
        deadline_monotonic_ms: u64,
    },
    AlreadyActive,
    ReplayConfirmed,
    ReplayRolledBack {
        reason: RollbackReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmationOutcome {
    Confirmed,
    ReplayConfirmed,
    RolledBack { reason: RollbackReason },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeConfirmationOutcome {
    Confirmed,
    ReplayConfirmed,
    /// Intent is durable, but selection and services have not been restored.
    RecoveryPending {
        intent: NativeRollbackIntent,
    },
}

/// An exact journal-bound credential inspection; mismatches must be errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeCredentialAvailability {
    Intact,
    Lost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Clean,
    AwaitingHealth {
        slot_id: String,
        deadline_monotonic_ms: u64,
    },
    RepairedPointer {
        slot_id: String,
    },
    DroppedInvalidPrevious {
        slot_id: String,
    },
    RolledBack {
        active_slot_id: String,
        rejected_slot_id: String,
        reason: RollbackReason,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingPhaseStatus {
    Prepared,
    AwaitingHealth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingStatus {
    pub operation_id: String,
    pub target_slot_id: String,
    pub previous_slot_id: String,
    pub boot_id: String,
    pub deadline_monotonic_ms: u64,
    pub phase: PendingPhaseStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagerStatus {
    pub generation: u64,
    pub floor_sequence: u64,
    pub active_slot_id: String,
    pub confirmed_slot_id: String,
    pub previous_slot_id: Option<String>,
    pub pending: Option<PendingStatus>,
    pub last_rollback: Option<RollbackReason>,
}

/// Pointer classification only; PendingTarget/Other do not verify that target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverySelection {
    Confirmed,
    PendingTarget,
    Other,
    Missing,
    Unsafe,
}

/// Read-only recovery context with an independently verified confirmed anchor.
/// This is not recovery intent, service-start authority or recovery completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryInspection {
    pub schema_version: u32,
    pub generation: u64,
    pub confirmed_slot_id: String,
    pub pending: Option<PendingStatus>,
    pub selection: RecoverySelection,
    pub rollback_intent: Option<NativeRollbackIntent>,
    pub completion: Option<NativeRecoveryCompletion>,
}

/// Historical restored-readiness receipt, not current health or cleanup authority
/// without an exact journal generation check under the manager locks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeRecoveryCompletion {
    pub intent: NativeRollbackIntent,
    pub completion_generation: u64,
    pub failed_target_slot_id: String,
    pub confirmed_slot_id: String,
    pub health_binding: String,
}

/// Durable pre-teardown rejection. Native restoration must finish before this
/// intent can be cleared; legacy mutation APIs refuse the schema-2 journal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeRollbackIntent {
    pub operation_id: String,
    pub intent_generation: u64,
    pub reason: RollbackReason,
    #[serde(default)]
    pub phase: NativeRollbackPhase,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeRollbackPhase {
    #[default]
    RollbackPending,
    Restoring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StorageRecovery {
    pub abandoned_staging_removed: usize,
    pub slot_deletions_completed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GarbageCollection {
    pub recovery: StorageRecovery,
    pub deleted_slot_ids: Vec<String>,
    pub retained_unreferenced: usize,
}

#[derive(Debug, Error)]
pub enum UpdateManagerError {
    #[error("application slot root path is invalid")]
    InvalidSlotRoot,
    #[error("application slot root is unsafe")]
    UnsafeSlotRoot,
    #[error("application state root path is invalid")]
    InvalidStateRoot,
    #[error("application state root is unsafe")]
    UnsafeStateRoot,
    #[error("application state and slot roots must be distinct")]
    OverlappingRoots,
    #[error("application state and slots must share one filesystem")]
    DifferentFilesystems,
    #[error("another application update manager owns the state lock")]
    ManagerBusy,
    #[error("application update journal is unsafe")]
    UnsafeJournal,
    #[error("application update journal is invalid")]
    InvalidJournal,
    #[error("application update journal exceeded its size limit")]
    JournalTooLarge,
    #[error("application update state changed concurrently")]
    ConcurrentStateChange,
    #[error("application update manager is not initialized")]
    NotInitialized,
    #[error("application update manager is already initialized")]
    AlreadyInitialized,
    #[error("application current-slot pointer is unsafe")]
    UnsafePointer,
    #[error("application slot identifier is invalid")]
    InvalidSlotId,
    #[error("application update generation is exhausted")]
    GenerationExhausted,
    #[error("application operation history is full")]
    HistoryFull,
    #[error("application update clock snapshot is invalid")]
    InvalidClock,
    #[error("application update operation binding is invalid")]
    InvalidOperation,
    #[error("application update operation ID conflicts with persisted history")]
    OperationConflict,
    #[error("another application activation is pending")]
    PendingActivation,
    #[error("application activation requires recovery before continuing")]
    RecoveryRequired,
    #[error("application update requires a newer system image")]
    MinimumSystemSequence,
    #[error("application anti-rollback policy rejected the slot")]
    AntiRollback,
    #[error("application health confirmation does not match the pending activation")]
    ConfirmationMismatch,
    #[error("application slot proof does not match persisted state")]
    SlotProofMismatch,
    #[error("application slot retention policy is invalid")]
    InvalidRetention,
    #[error("application slot staging or proof validation failed")]
    Stager(#[from] ApplicationStageError),
    #[error("application update I/O failed")]
    Io(#[from] io::Error),
}

#[derive(Debug)]
pub struct ApplicationUpdateManager {
    stager: ApplicationSlotStager,
    trust_root: ApplicationTrustRoot,
    state: StateStore,
    pointer: PointerStore,
    system_sequence: u64,
    health_timeout_ms: u64,
}

pub use bli_kvm_update_stager::VerifiedSlotFiles;

#[derive(Debug)]
enum PrepareResult {
    Immediate(ActivationOutcome),
    Prepared,
}

/// Durable admission is not candidate activation or health readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationPreparation {
    Immediate(ActivationOutcome),
    Prepared,
}

impl ApplicationUpdateManager {
    pub fn open(
        slot_root: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
        trust_root: ApplicationTrustRoot,
        system_sequence: u64,
        health_timeout_ms: u64,
    ) -> Result<Self, UpdateManagerError> {
        if system_sequence == 0
            || !(MINIMUM_HEALTH_TIMEOUT_MS..=MAXIMUM_HEALTH_TIMEOUT_MS).contains(&health_timeout_ms)
        {
            return Err(UpdateManagerError::InvalidClock);
        }
        let slot_root = slot_root.into();
        let state_root = state_root.into();
        let state = StateStore::open(state_root)?;
        let stager = ApplicationSlotStager::open(slot_root.clone())?;
        let pointer = PointerStore::open(slot_root)?;
        if pointer.root_path().starts_with(state.root_path())
            || state.root_path().starts_with(pointer.root_path())
            || stager.root_path().starts_with(state.root_path())
            || state.root_path().starts_with(stager.root_path())
        {
            return Err(UpdateManagerError::OverlappingRoots);
        }
        if pointer.device() != state.device() {
            return Err(UpdateManagerError::DifferentFilesystems);
        }
        Ok(Self {
            stager,
            trust_root,
            state,
            pointer,
            system_sequence,
            health_timeout_ms,
        })
    }

    pub fn initialize(&self, active_slot_id: &str) -> Result<ManagerStatus, UpdateManagerError> {
        if self.state.load()?.is_some() {
            return Err(UpdateManagerError::AlreadyInitialized);
        }
        if self.pointer.read()?.is_some() {
            return Err(UpdateManagerError::UnsafePointer);
        }
        let staged = self.stager.inspect_slot(active_slot_id, &self.trust_root)?;
        self.enforce_system_compatibility(&staged)?;
        let document = JournalDocument::initial(SlotReference::from_staged(&staged));
        self.state.store(&document, None)?;
        self.pointer.switch(active_slot_id)?;
        self.status()
    }

    pub fn health_timeout_ms(&self) -> u64 {
        self.health_timeout_ms
    }

    pub fn cleanup_abandoned_staging(&self) -> Result<StagingCleanup, UpdateManagerError> {
        Ok(self.stager.cleanup_abandoned_staging()?)
    }

    pub fn recover_update_storage(&self) -> Result<StorageRecovery, UpdateManagerError> {
        let document = self.load_required()?;
        self.validate_referenced_slots(&document)?;
        let protected = protected_slot_ids(&document);
        let abandoned_staging_removed = self.stager.cleanup_abandoned_staging()?.removed;
        let slot_deletions_completed = self
            .stager
            .recover_slot_deletions(&protected, &self.trust_root)?
            .completed;
        Ok(StorageRecovery {
            abandoned_staging_removed,
            slot_deletions_completed,
        })
    }

    pub fn garbage_collect(
        &self,
        retain_unreferenced: usize,
    ) -> Result<GarbageCollection, UpdateManagerError> {
        if retain_unreferenced > MAX_UNREFERENCED_SLOT_RETENTION {
            return Err(UpdateManagerError::InvalidRetention);
        }
        let _ = self.status()?;
        let recovery = self.recover_update_storage()?;
        let document = self.load_required()?;
        let protected = protected_slot_ids(&document);
        let slots = self.stager.list_slots(&self.trust_root)?;
        let mut unreferenced = slots
            .into_iter()
            .filter(|slot| !protected.contains(&slot.slot_id))
            .collect::<Vec<_>>();
        unreferenced.sort_by(|left, right| {
            left.manifest
                .sequence
                .cmp(&right.manifest.sequence)
                .then_with(|| left.slot_id.cmp(&right.slot_id))
        });
        let delete_count = unreferenced.len().saturating_sub(retain_unreferenced);
        let mut deleted_slot_ids = Vec::with_capacity(delete_count);
        for slot in unreferenced.into_iter().take(delete_count) {
            let deleted = self.stager.delete_unreferenced_slot(
                &slot.slot_id,
                &protected,
                &self.trust_root,
            )?;
            deleted_slot_ids.push(deleted.slot_id);
        }
        Ok(GarbageCollection {
            recovery,
            deleted_slot_ids,
            retained_unreferenced: retain_unreferenced,
        })
    }

    /// Verify slot metadata under the manager's existing slot-root lock.
    /// Opening another stager while this manager lives would contend with itself.
    pub fn inspect_slot(&self, slot_id: &str) -> Result<StagedApplication, UpdateManagerError> {
        Ok(self.stager.inspect_slot(slot_id, &self.trust_root)?)
    }

    pub fn open_verified_slot_files(
        &self,
        slot_id: &str,
        paths: &[&str],
    ) -> Result<bli_kvm_update_stager::VerifiedSlotFiles, UpdateManagerError> {
        Ok(self
            .stager
            .open_verified_files(slot_id, paths, &self.trust_root)?)
    }

    pub fn status(&self) -> Result<ManagerStatus, UpdateManagerError> {
        let document = self.load_required()?;
        self.validate_referenced_slots(&document)?;
        if self.pointer.read()?.as_deref() != Some(document.active.slot_id.as_str()) {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(status_from_document(&document))
    }

    /// Active/idle schema-3 status only. Recovery intent or current completion
    /// requires inspect_recovery and cannot authorize ordinary candidate startup.
    pub fn native_status(&self) -> Result<ManagerStatus, UpdateManagerError> {
        let document = self.load_native_active()?;
        self.validate_referenced_slots(&document)?;
        if self.pointer.read()?.as_deref() != Some(document.active.slot_id.as_str()) {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(status_from_document(&document))
    }

    /// Read-only status for either profile: the legacy journal, or an idle or
    /// pending native journal. Native recovery states remain RecoveryRequired.
    pub fn observed_status(&self) -> Result<ManagerStatus, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.schema_version == model::NATIVE_LIFECYCLE_SCHEMA_VERSION {
            self.native_status()
        } else {
            self.status()
        }
    }

    /// Inspect recovery without repairing the pointer or requiring a possibly
    /// corrupt candidate/retired slot to pass verification. Journal validation
    /// binds pending.previous to confirmed. The confirmed proof and system
    /// compatibility are checked independently under the held manager locks.
    pub fn inspect_recovery(&self) -> Result<RecoveryInspection, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        self.validate_slot_reference(&document.confirmed)?;
        let selection = match self.pointer.read() {
            Ok(Some(slot)) if slot == document.confirmed.slot_id => RecoverySelection::Confirmed,
            Ok(Some(slot))
                if document
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.target.slot_id == slot) =>
            {
                RecoverySelection::PendingTarget
            }
            Ok(Some(_)) => RecoverySelection::Other,
            Ok(None) => RecoverySelection::Missing,
            Err(UpdateManagerError::UnsafePointer) => RecoverySelection::Unsafe,
            Err(error) => return Err(error),
        };
        let status = status_from_document(&document);
        Ok(RecoveryInspection {
            schema_version: document.schema_version,
            generation: status.generation,
            confirmed_slot_id: status.confirmed_slot_id,
            pending: status.pending,
            selection,
            rollback_intent: document.native_rollback,
            completion: document.native_completion,
        })
    }

    /// Enter native idle without changing selection or raising the rollback floor.
    /// The trusted, bounded callback must reconcile exact completion credentials
    /// (when supplied) and reject conflicting runtime markers under these locks.
    /// It must not re-enter a manager or dispatch services. Its cleanup must replay
    /// safely if persistence fails, including an ambiguous final directory fsync.
    /// Archived receipts retain evidence but never authorize later cleanup.
    pub fn migrate_native_idle(
        &self,
        expected_generation: u64,
        cleanup: impl FnOnce(Option<&NativeRecoveryCompletion>) -> Result<(), UpdateManagerError>,
    ) -> Result<u64, UpdateManagerError> {
        let before = self.validate_native_idle_migration(expected_generation)?;
        if before.schema_version == model::NATIVE_LIFECYCLE_SCHEMA_VERSION
            && before.native_completion.is_none()
        {
            return Ok(before.generation);
        }
        let mut next = before.clone();
        next.schema_version = model::NATIVE_LIFECYCLE_SCHEMA_VERSION;
        if let Some(completion) = next.native_completion.take() {
            let record = next
                .history
                .iter_mut()
                .find(|record| record.operation_id == completion.intent.operation_id)
                .ok_or(UpdateManagerError::InvalidJournal)?;
            if record.native_completion.is_some() {
                return Err(UpdateManagerError::InvalidJournal);
            }
            record.native_completion = Some(completion);
        }
        next.advance_generation()?;
        // Refuse capacity exhaustion before consuming any runtime material.
        let _ = journal::encode_document(&next)?;
        cleanup(before.native_completion.as_ref())?;
        if self.validate_native_idle_migration(expected_generation)? != before {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.state.store(&next, Some(expected_generation))?;
        Ok(next.generation)
    }

    fn validate_native_idle_migration(
        &self,
        expected_generation: u64,
    ) -> Result<JournalDocument, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.generation != expected_generation
            || document.pending.is_some()
            || document.native_rollback.is_some()
            || self.pointer.read()?.as_deref() != Some(document.confirmed.slot_id.as_str())
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.validate_referenced_slots(&document)?;
        Ok(document)
    }

    /// Persist explicit native rollback intent before any service teardown.
    /// Generation and operation are exact fences, not refreshed on retry.
    /// The pointer/floor/activation phase are unchanged. This opts the journal
    /// into schema 2; an authorized native restoration executor is then required.
    pub fn begin_native_rejection(
        &self,
        expected_generation: u64,
        operation_id: [u8; 16],
    ) -> Result<NativeRollbackIntent, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.generation != expected_generation || document.native_rollback.is_some() {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let pending = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if pending.operation_id != encode_hex(&operation_id) {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        self.validate_rollback_anchor(&document, pending)?;
        self.persist_native_intent(document, RollbackReason::ExplicitHealthFailure)
    }

    /// Persist a boot/watchdog recovery decision without service or pointer writes.
    /// Existing intent is retained exactly, never relabeled after a later fault.
    /// None means no new rollback decision; callers must inspect the state to
    /// distinguish healthy pending, completed recovery and an idle journal.
    pub fn begin_native_recovery(
        &self,
        expected_generation: u64,
        clock: ClockSnapshot,
    ) -> Result<Option<NativeRollbackIntent>, UpdateManagerError> {
        self.begin_native_recovery_with_credentials(expected_generation, clock, |_, _, _| {
            Ok(NativeCredentialAvailability::Intact)
        })
    }

    /// Inspect credentials only for an otherwise viable schema-3 pending
    /// activation. The trusted callback receives hexadecimal journal identity,
    /// never cleanup authority. Legacy schemas and existing reasons bypass it.
    pub fn begin_native_recovery_with_credentials(
        &self,
        expected_generation: u64,
        clock: ClockSnapshot,
        inspect: impl FnOnce(
            &str,
            &str,
            &str,
        ) -> Result<NativeCredentialAvailability, UpdateManagerError>,
    ) -> Result<Option<NativeRollbackIntent>, UpdateManagerError> {
        validate_clock(clock)?;
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.generation != expected_generation {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.validate_slot_reference(&document.confirmed)?;
        if let Some(intent) = &document.native_rollback {
            return Ok(Some(intent.clone()));
        }
        let Some(pending) = &document.pending else {
            return Ok(None);
        };
        let current = match self.pointer.read() {
            Ok(current) => current,
            Err(UpdateManagerError::UnsafePointer) => None,
            Err(error) => return Err(error),
        };
        let reason = if self.validate_slot_reference(&pending.target).is_err() {
            RollbackReason::TargetInvalid
        } else if pending.phase == PendingPhase::Prepared {
            RollbackReason::InterruptedPrepare
        } else if current.as_deref() != Some(pending.target.slot_id.as_str()) {
            RollbackReason::PointerMismatch
        } else if pending.boot_id != encode_hex(&clock.boot_id) {
            RollbackReason::BootChanged
        } else if clock.monotonic_ms > pending.deadline_monotonic_ms {
            RollbackReason::HealthTimeout
        } else {
            if clock.monotonic_ms < pending.prepared_monotonic_ms {
                return Err(UpdateManagerError::RecoveryRequired);
            }
            if document.schema_version != model::NATIVE_LIFECYCLE_SCHEMA_VERSION {
                return Ok(None);
            }
            let availability = inspect(
                &pending.operation_id,
                &pending.health_binding,
                &pending.target.slot_id,
            )?;
            if self.state.load()?.as_ref() != Some(&document) {
                return Err(UpdateManagerError::ConcurrentStateChange);
            }
            self.validate_rollback_anchor(&document, pending)?;
            if self.pointer.read()? != current {
                return Err(UpdateManagerError::RecoveryRequired);
            }
            if availability == NativeCredentialAvailability::Intact {
                return Ok(None);
            }
            RollbackReason::HealthChallengeLost
        };
        self.persist_native_intent(document, reason).map(Some)
    }

    fn persist_native_intent(
        &self,
        mut document: JournalDocument,
        reason: RollbackReason,
    ) -> Result<NativeRollbackIntent, UpdateManagerError> {
        let operation_id = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?
            .operation_id
            .clone();
        let previous_generation = document.advance_generation()?;
        let intent = NativeRollbackIntent {
            operation_id,
            intent_generation: document.generation,
            reason,
            phase: NativeRollbackPhase::RollbackPending,
        };
        if document.schema_version == model::JOURNAL_SCHEMA_VERSION {
            document.schema_version = model::NATIVE_JOURNAL_SCHEMA_VERSION;
        }
        document.native_rollback = Some(intent.clone());
        self.state.store(&document, Some(previous_generation))?;
        Ok(intent)
    }

    /// Restore only after the trusted callback observes complete, bounded
    /// quiescence. Re-entry always quiesces again: durable state cannot prove
    /// that systemd has not restarted a process. Failure retains native intent.
    /// This selects the verified confirmed slot but does not start services,
    /// complete the rollback record, or acknowledge restored readiness.
    pub fn restore_native_selection(
        &self,
        expected_generation: u64,
        intent: &NativeRollbackIntent,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<RecoveryInspection, UpdateManagerError> {
        self.validate_native_restoration(expected_generation, intent)?;
        quiesce()?;
        let mut document = self.validate_native_restoration(expected_generation, intent)?;
        let previous_generation = document.advance_generation()?;
        self.pointer.switch(&document.confirmed.slot_id)?;
        document.active = document.confirmed.clone();
        document.previous = None;
        document
            .native_rollback
            .as_mut()
            .ok_or(UpdateManagerError::RecoveryRequired)?
            .phase = NativeRollbackPhase::Restoring;
        self.state.store(&document, Some(previous_generation))?;
        self.inspect_recovery()
    }

    fn validate_native_restoration(
        &self,
        expected_generation: u64,
        intent: &NativeRollbackIntent,
    ) -> Result<JournalDocument, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if ![
            model::NATIVE_JOURNAL_SCHEMA_VERSION,
            model::NATIVE_LIFECYCLE_SCHEMA_VERSION,
        ]
        .contains(&document.schema_version)
            || document.generation != expected_generation
            || document.native_rollback.as_ref() != Some(intent)
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.validate_slot_reference(&document.confirmed)?;
        Ok(document)
    }

    /// Complete restoration only after a fresh, bounded, trusted readiness gate.
    /// The caller must observe the full confirmed composition, with executable
    /// identity fences, within its recovery deadline. The callback runs under the
    /// held manager locks and must not re-enter a manager. Failure preserves intent.
    /// Completion preserves its native schema; legacy mutation remains blocked.
    pub fn acknowledge_native_restoration(
        &self,
        expected_generation: u64,
        intent: &NativeRollbackIntent,
        readiness: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<NativeRecoveryCompletion, UpdateManagerError> {
        let before = self.validate_native_acknowledgement(expected_generation, intent)?;
        readiness()?;
        let mut document = self.validate_native_acknowledgement(expected_generation, intent)?;
        if document != before {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let pending = document
            .pending
            .take()
            .ok_or(UpdateManagerError::RecoveryRequired)?;
        let previous_generation = document.advance_generation()?;
        let completion = NativeRecoveryCompletion {
            intent: intent.clone(),
            completion_generation: document.generation,
            failed_target_slot_id: pending.target.slot_id,
            confirmed_slot_id: document.confirmed.slot_id.clone(),
            health_binding: pending.health_binding,
        };
        document.complete_operation(
            &intent.operation_id,
            OperationOutcome::RolledBack {
                reason: intent.reason,
            },
        )?;
        document.last_rollback = Some(intent.reason);
        document.native_rollback = None;
        document.native_completion = Some(completion.clone());
        self.state.store(&document, Some(previous_generation))?;
        Ok(completion)
    }

    fn validate_native_acknowledgement(
        &self,
        expected_generation: u64,
        intent: &NativeRollbackIntent,
    ) -> Result<JournalDocument, UpdateManagerError> {
        let document = self.validate_native_restoration(expected_generation, intent)?;
        if intent.phase != NativeRollbackPhase::Restoring
            || self.pointer.read()?.as_deref() != Some(document.confirmed.slot_id.as_str())
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(document)
    }

    pub fn activate(
        &self,
        request: ActivationRequest,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        self.activate_with_quiescence(request, || Ok(()))
    }

    /// Persist intent without stopping services or switching the pointer.
    /// Network callers must validate their generation fence under these same
    /// manager locks before admission. Prepared work may be recovered on crash.
    pub fn admit_activation(
        &self,
        request: &ActivationRequest,
    ) -> Result<ActivationPreparation, UpdateManagerError> {
        Ok(match self.prepare_activation(request)? {
            PrepareResult::Immediate(outcome) => ActivationPreparation::Immediate(outcome),
            PrepareResult::Prepared => ActivationPreparation::Prepared,
        })
    }

    /// Native admission only: exact generation, durable Prepared, no service or
    /// pointer mutations. Runtime must reconcile credentials before calling and
    /// deliver admission EOF before executing this operation.
    pub fn admit_native_activation(
        &self,
        expected_generation: u64,
        request: &ActivationRequest,
    ) -> Result<ActivationPreparation, UpdateManagerError> {
        let document = self.load_native_active()?;
        if document.generation != expected_generation {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(match self.prepare_activation_document(request, document)? {
            PrepareResult::Immediate(outcome) => ActivationPreparation::Immediate(outcome),
            PrepareResult::Prepared => ActivationPreparation::Prepared,
        })
    }

    /// Continue native Prepared only after admission EOF. Callbacks are trusted
    /// and bounded and run under the held manager locks. Failure leaves native
    /// recovery responsibility intact, including after partial quiescence.
    pub fn execute_native_prepared_activation(
        &self,
        expected_generation: u64,
        mut request: ActivationRequest,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
        clock_after: impl FnOnce() -> Result<ClockSnapshot, UpdateManagerError>,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        let before = self.validate_prepared_document(&request, self.load_native_active()?)?;
        if before.generation != expected_generation {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        quiesce()?;
        let after = clock_after()?;
        if after.monotonic_ms < request.clock.monotonic_ms {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        request.clock = after;
        let document = self.validate_prepared_document(&request, self.load_native_active()?)?;
        if document != before {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.select_prepared_document(&request, document)
    }

    /// Continue an exactly bound Prepared operation after admission delivery.
    /// The manager owns state/slot locks throughout validation and quiescence.
    /// Both request.clock and clock_after must come from trusted kernel evidence.
    /// Failure retains intent; this does not start services or confirm health.
    pub fn execute_prepared_activation(
        &self,
        mut request: ActivationRequest,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
        clock_after: impl FnOnce() -> Result<ClockSnapshot, UpdateManagerError>,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        self.validate_prepared_request(&request)?;
        quiesce()?;
        let after = clock_after()?;
        if after.monotonic_ms < request.clock.monotonic_ms {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        request.clock = after;
        self.switch_prepared_activation(&request)
    }

    /// Quiesce the previous composition after durable preparation and before
    /// selecting the candidate. The caller must bound its quiescence operation.
    /// A failure leaves Prepared intent for recovery, including when components
    /// were partially stopped. Replay/no-op outcomes do not invoke the hook.
    /// This is not a readiness or health-confirmation hook.
    pub fn activate_with_quiescence(
        &self,
        request: ActivationRequest,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        match self.prepare_activation(&request)? {
            PrepareResult::Immediate(outcome) => Ok(outcome),
            PrepareResult::Prepared => {
                quiesce()?;
                self.switch_prepared_activation(&request)
            }
        }
    }

    /// Confirm schema-3 activation only after fresh whole-composition readiness.
    /// Trusted callbacks must be bounded, identity-fenced and must not re-enter a
    /// manager. An error from persistence is not proof that nothing committed;
    /// callers must inspect durable state before choosing any subsequent action.
    pub fn confirm_native_activation(
        &self,
        expected_generation: u64,
        request: &ActivationRequest,
        readiness: impl FnOnce() -> Result<(), UpdateManagerError>,
        clock_after: impl FnOnce() -> Result<ClockSnapshot, UpdateManagerError>,
    ) -> Result<NativeConfirmationOutcome, UpdateManagerError> {
        validate_operation_material(&request.operation_id, &request.health_binding)?;
        validate_clock(request.clock)?;
        let operation_id = encode_hex(&request.operation_id);
        let before = self.load_native_active()?;
        if before.generation != expected_generation {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let record = before
            .find_record(&operation_id)
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if record.health_binding != encode_hex(&request.health_binding)
            || record.target_slot_id != request.target_slot_id
        {
            return Err(UpdateManagerError::OperationConflict);
        }
        if record.outcome == OperationOutcome::Confirmed {
            self.validate_referenced_slots(&before)?;
            if self.pointer.read()?.as_deref() != Some(before.active.slot_id.as_str()) {
                return Err(UpdateManagerError::RecoveryRequired);
            }
            return Ok(NativeConfirmationOutcome::ReplayConfirmed);
        }
        if record.outcome != OperationOutcome::Pending {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        if let Some(reason) = self.native_confirmation_failure(&before, request.clock)? {
            let intent = self.persist_native_intent(before, reason)?;
            return Ok(NativeConfirmationOutcome::RecoveryPending { intent });
        }
        let ready = readiness();
        let after = clock_after()?;
        validate_clock(after)?;
        if after.boot_id == request.clock.boot_id && after.monotonic_ms < request.clock.monotonic_ms
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let mut document = self.load_native_active()?;
        if document != before {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let reason = self
            .native_confirmation_failure(&document, after)?
            .or_else(|| ready.err().map(|_| RollbackReason::ExplicitHealthFailure));
        if let Some(reason) = reason {
            let intent = self.persist_native_intent(document, reason)?;
            return Ok(NativeConfirmationOutcome::RecoveryPending { intent });
        }
        let pending = document
            .pending
            .take()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        let previous_generation = document.advance_generation()?;
        document.active = pending.target.clone();
        document.confirmed = pending.target;
        document.floor_sequence = document.confirmed.sequence;
        document.last_rollback = None;
        document.complete_operation(&operation_id, OperationOutcome::Confirmed)?;
        self.state.store(&document, Some(previous_generation))?;
        Ok(NativeConfirmationOutcome::Confirmed)
    }

    fn native_confirmation_failure(
        &self,
        document: &JournalDocument,
        clock: ClockSnapshot,
    ) -> Result<Option<RollbackReason>, UpdateManagerError> {
        let pending = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if pending.phase != PendingPhase::AwaitingHealth {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        if pending.boot_id == encode_hex(&clock.boot_id)
            && clock.monotonic_ms < pending.prepared_monotonic_ms
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.validate_rollback_anchor(document, pending)?;
        Ok(if self.validate_slot_reference(&pending.target).is_err() {
            Some(RollbackReason::TargetInvalid)
        } else if !matches!(self.pointer.read(), Ok(Some(slot)) if slot == pending.target.slot_id) {
            Some(RollbackReason::PointerMismatch)
        } else if pending.boot_id != encode_hex(&clock.boot_id) {
            Some(RollbackReason::BootChanged)
        } else if clock.monotonic_ms > pending.deadline_monotonic_ms {
            Some(RollbackReason::HealthTimeout)
        } else {
            None
        })
    }

    /// Authorize exact cleanup from durable confirmation, never merely from a
    /// selected slot or historical replay outcome. The trusted callback receives
    /// the stored binding hash, not a credential, and must replay idempotently.
    pub fn cleanup_native_confirmation(
        &self,
        expected_generation: u64,
        operation_id: [u8; 16],
        target_slot_id: &str,
        cleanup: impl FnOnce(&str) -> Result<(), UpdateManagerError>,
    ) -> Result<(), UpdateManagerError> {
        let before = self.load_native_active()?;
        if before.generation != expected_generation
            || before.pending.is_some()
            || before.confirmed.slot_id != target_slot_id
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let record = before
            .find_record(&encode_hex(&operation_id))
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if record.outcome != OperationOutcome::Confirmed || record.target_slot_id != target_slot_id
        {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        let _ = self.native_status()?;
        cleanup(&record.health_binding)?;
        if self.load_native_active()? != before {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        let _ = self.native_status()?;
        Ok(())
    }

    pub fn confirm_health(
        &self,
        operation_id: [u8; 16],
        health_binding: [u8; 32],
        clock: ClockSnapshot,
    ) -> Result<ConfirmationOutcome, UpdateManagerError> {
        validate_operation_material(&operation_id, &health_binding)?;
        validate_clock(clock)?;
        let operation_id = encode_hex(&operation_id);
        let health_binding = encode_hex(&health_binding);
        let mut document = self.load_required()?;

        if let Some(record) = document.find_record(&operation_id) {
            if record.health_binding != health_binding {
                return Err(UpdateManagerError::OperationConflict);
            }
            match record.outcome {
                OperationOutcome::Confirmed => {
                    self.validate_referenced_slots(&document)?;
                    return Ok(ConfirmationOutcome::ReplayConfirmed);
                }
                OperationOutcome::RolledBack { reason } => {
                    self.validate_referenced_slots(&document)?;
                    return Ok(ConfirmationOutcome::RolledBack { reason });
                }
                OperationOutcome::Noop => return Err(UpdateManagerError::ConfirmationMismatch),
                OperationOutcome::Pending => {}
            }
        }

        let pending = document
            .pending
            .clone()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if pending.operation_id != operation_id
            || pending.health_binding != health_binding
            || pending.phase != PendingPhase::AwaitingHealth
        {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        if pending.boot_id == encode_hex(&clock.boot_id)
            && clock.monotonic_ms < pending.prepared_monotonic_ms
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.validate_rollback_anchor(&document, &pending)?;
        if self.validate_slot_reference(&pending.target).is_err() {
            return self
                .rollback_document(document, RollbackReason::TargetInvalid)
                .map(confirmation_from_recovery);
        }
        if pending.boot_id != encode_hex(&clock.boot_id) {
            return self
                .rollback_document(document, RollbackReason::BootChanged)
                .map(confirmation_from_recovery);
        }
        if clock.monotonic_ms > pending.deadline_monotonic_ms {
            return self
                .rollback_document(document, RollbackReason::HealthTimeout)
                .map(confirmation_from_recovery);
        }
        let current_matches = matches!(
            self.pointer.read(),
            Ok(Some(current)) if current == pending.target.slot_id
        );
        if !current_matches {
            return self
                .rollback_document(document, RollbackReason::PointerMismatch)
                .map(confirmation_from_recovery);
        }

        let previous_generation = document.advance_generation()?;
        document.active = pending.target.clone();
        document.confirmed = pending.target;
        document.floor_sequence = document.confirmed.sequence;
        document.pending = None;
        document.last_rollback = None;
        document.complete_operation(&operation_id, OperationOutcome::Confirmed)?;
        self.state.store(&document, Some(previous_generation))?;
        Ok(ConfirmationOutcome::Confirmed)
    }

    pub fn reject_health(
        &self,
        operation_id: [u8; 16],
        health_binding: [u8; 32],
    ) -> Result<RecoveryOutcome, UpdateManagerError> {
        validate_operation_material(&operation_id, &health_binding)?;
        let operation_id = encode_hex(&operation_id);
        let health_binding = encode_hex(&health_binding);
        let document = self.load_required()?;
        if let Some(record) = document.find_record(&operation_id) {
            if record.health_binding != health_binding {
                return Err(UpdateManagerError::OperationConflict);
            }
            match record.outcome {
                OperationOutcome::RolledBack { reason } => {
                    self.validate_referenced_slots(&document)?;
                    return Ok(RecoveryOutcome::RolledBack {
                        active_slot_id: document.active.slot_id.clone(),
                        rejected_slot_id: record.target_slot_id.clone(),
                        reason,
                    });
                }
                OperationOutcome::Confirmed | OperationOutcome::Noop => {
                    return Err(UpdateManagerError::ConfirmationMismatch);
                }
                OperationOutcome::Pending => {}
            }
        }
        let pending = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        if pending.operation_id != operation_id || pending.health_binding != health_binding {
            return Err(UpdateManagerError::ConfirmationMismatch);
        }
        self.validate_rollback_anchor(&document, pending)?;
        self.rollback_document(document, RollbackReason::ExplicitHealthFailure)
    }

    pub fn abort_missing_health_challenge(&self) -> Result<RecoveryOutcome, UpdateManagerError> {
        let document = self.load_required()?;
        let pending = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::ConfirmationMismatch)?;
        self.validate_rollback_anchor(&document, pending)?;
        self.rollback_document(document, RollbackReason::HealthChallengeLost)
    }

    pub fn recover(&self, clock: ClockSnapshot) -> Result<RecoveryOutcome, UpdateManagerError> {
        validate_clock(clock)?;
        let mut document = self.load_required()?;
        let current = match self.pointer.read() {
            Ok(current) => current,
            Err(UpdateManagerError::UnsafePointer) => None,
            Err(error) => return Err(error),
        };
        let Some(pending) = document.pending.clone() else {
            self.validate_slot_reference(&document.active)?;
            self.validate_slot_reference(&document.confirmed)?;
            let dropped_previous = document
                .previous
                .as_ref()
                .is_some_and(|previous| self.validate_slot_reference(previous).is_err());
            if dropped_previous {
                let previous_generation = document.advance_generation()?;
                document.previous = None;
                self.state.store(&document, Some(previous_generation))?;
            }
            if current.as_deref() == Some(document.active.slot_id.as_str()) {
                if dropped_previous {
                    return Ok(RecoveryOutcome::DroppedInvalidPrevious {
                        slot_id: document.active.slot_id,
                    });
                }
                return Ok(RecoveryOutcome::Clean);
            }
            self.pointer.switch(&document.active.slot_id)?;
            return Ok(RecoveryOutcome::RepairedPointer {
                slot_id: document.active.slot_id,
            });
        };
        self.validate_rollback_anchor(&document, &pending)?;
        if self.validate_slot_reference(&pending.target).is_err() {
            return self.rollback_document(document, RollbackReason::TargetInvalid);
        }
        match pending.phase {
            PendingPhase::Prepared => {
                self.rollback_document(document, RollbackReason::InterruptedPrepare)
            }
            PendingPhase::AwaitingHealth => {
                if current.as_deref() != Some(pending.target.slot_id.as_str()) {
                    return self.rollback_document(document, RollbackReason::PointerMismatch);
                }
                if pending.boot_id != encode_hex(&clock.boot_id) {
                    return self.rollback_document(document, RollbackReason::BootChanged);
                }
                if clock.monotonic_ms > pending.deadline_monotonic_ms {
                    return self.rollback_document(document, RollbackReason::HealthTimeout);
                }
                Ok(RecoveryOutcome::AwaitingHealth {
                    slot_id: pending.target.slot_id,
                    deadline_monotonic_ms: pending.deadline_monotonic_ms,
                })
            }
        }
    }

    fn prepare_activation(
        &self,
        request: &ActivationRequest,
    ) -> Result<PrepareResult, UpdateManagerError> {
        self.prepare_activation_document(request, self.load_required()?)
    }

    fn prepare_activation_document(
        &self,
        request: &ActivationRequest,
        mut document: JournalDocument,
    ) -> Result<PrepareResult, UpdateManagerError> {
        validate_operation_material(&request.operation_id, &request.health_binding)?;
        validate_clock(request.clock)?;
        let operation_id = encode_hex(&request.operation_id);
        let health_binding = encode_hex(&request.health_binding);
        self.validate_referenced_slots(&document)?;
        if self.pointer.read()?.as_deref() != Some(document.active.slot_id.as_str()) {
            return Err(UpdateManagerError::RecoveryRequired);
        }

        if let Some(record) = document.find_record(&operation_id) {
            if record.target_slot_id != request.target_slot_id
                || record.health_binding != health_binding
            {
                return Err(UpdateManagerError::OperationConflict);
            }
            return match record.outcome {
                OperationOutcome::Confirmed => {
                    Ok(PrepareResult::Immediate(ActivationOutcome::ReplayConfirmed))
                }
                OperationOutcome::RolledBack { reason } => Ok(PrepareResult::Immediate(
                    ActivationOutcome::ReplayRolledBack { reason },
                )),
                OperationOutcome::Noop => {
                    Ok(PrepareResult::Immediate(ActivationOutcome::AlreadyActive))
                }
                OperationOutcome::Pending => {
                    let pending = document
                        .pending
                        .as_ref()
                        .ok_or(UpdateManagerError::InvalidJournal)?;
                    if pending.phase == PendingPhase::AwaitingHealth {
                        if pending.boot_id != encode_hex(&request.clock.boot_id)
                            || request.clock.monotonic_ms < pending.prepared_monotonic_ms
                            || request.clock.monotonic_ms > pending.deadline_monotonic_ms
                        {
                            return Err(UpdateManagerError::RecoveryRequired);
                        }
                        Ok(PrepareResult::Immediate(awaiting_outcome(
                            request.operation_id,
                            pending,
                        )))
                    } else {
                        Err(UpdateManagerError::RecoveryRequired)
                    }
                }
            };
        }
        if document.pending.is_some() {
            return Err(UpdateManagerError::PendingActivation);
        }

        let target = self
            .stager
            .inspect_slot(&request.target_slot_id, &self.trust_root)?;
        self.enforce_system_compatibility(&target)?;
        let target_reference = SlotReference::from_staged(&target);
        if target_reference == document.active {
            document.begin_operation(
                operation_id.clone(),
                target_reference.slot_id,
                health_binding,
            )?;
            document.complete_operation(&operation_id, OperationOutcome::Noop)?;
            let previous_generation = document.advance_generation()?;
            self.state.store(&document, Some(previous_generation))?;
            return Ok(PrepareResult::Immediate(ActivationOutcome::AlreadyActive));
        }
        if target_reference.sequence < document.floor_sequence
            || (target_reference.sequence == document.floor_sequence
                && target_reference != document.confirmed)
        {
            return Err(UpdateManagerError::AntiRollback);
        }
        if target_reference.sequence == document.floor_sequence
            && target_reference == document.confirmed
            && target_reference == document.active
        {
            return Err(UpdateManagerError::InvalidJournal);
        }
        let deadline = request
            .clock
            .monotonic_ms
            .checked_add(self.health_timeout_ms)
            .ok_or(UpdateManagerError::InvalidClock)?;
        let pending = PendingActivation {
            operation_id: operation_id.clone(),
            health_binding: health_binding.clone(),
            boot_id: encode_hex(&request.clock.boot_id),
            prepared_monotonic_ms: request.clock.monotonic_ms,
            deadline_monotonic_ms: deadline,
            previous: document.active.clone(),
            target: target_reference,
            phase: PendingPhase::Prepared,
        };
        document.begin_operation(operation_id, pending.target.slot_id.clone(), health_binding)?;
        document.pending = Some(pending);
        let previous_generation = document.advance_generation()?;
        if document.schema_version == model::NATIVE_LIFECYCLE_SCHEMA_VERSION {
            document.validate_native_activation_capacity()?;
        }
        self.state.store(&document, Some(previous_generation))?;
        Ok(PrepareResult::Prepared)
    }

    fn switch_prepared_activation(
        &self,
        request: &ActivationRequest,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        self.select_prepared_document(request, self.validate_prepared_request(request)?)
    }

    fn select_prepared_document(
        &self,
        request: &ActivationRequest,
        mut document: JournalDocument,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        let pending = document
            .pending
            .clone()
            .ok_or(UpdateManagerError::InvalidJournal)?;
        let previous_generation = document.advance_generation()?;
        document.active = pending.target.clone();
        document.previous = Some(pending.previous.clone());
        document
            .pending
            .as_mut()
            .ok_or(UpdateManagerError::InvalidJournal)?
            .phase = PendingPhase::AwaitingHealth;
        let _ = journal::encode_document(&document)?;
        self.pointer.switch(&pending.target.slot_id)?;
        self.state.store(&document, Some(previous_generation))?;
        Ok(awaiting_outcome(request.operation_id, &pending))
    }

    fn validate_prepared_request(
        &self,
        request: &ActivationRequest,
    ) -> Result<JournalDocument, UpdateManagerError> {
        self.validate_prepared_document(request, self.load_required()?)
    }

    fn validate_prepared_document(
        &self,
        request: &ActivationRequest,
        document: JournalDocument,
    ) -> Result<JournalDocument, UpdateManagerError> {
        validate_operation_material(&request.operation_id, &request.health_binding)?;
        validate_clock(request.clock)?;
        self.validate_referenced_slots(&document)?;
        let pending = document
            .pending
            .as_ref()
            .ok_or(UpdateManagerError::RecoveryRequired)?;
        if pending.operation_id != encode_hex(&request.operation_id)
            || pending.health_binding != encode_hex(&request.health_binding)
            || pending.target.slot_id != request.target_slot_id
            || pending.boot_id != encode_hex(&request.clock.boot_id)
            || request.clock.monotonic_ms < pending.prepared_monotonic_ms
            || request.clock.monotonic_ms > pending.deadline_monotonic_ms
            || pending.phase != PendingPhase::Prepared
            || self.pointer.read()?.as_deref() != Some(pending.previous.slot_id.as_str())
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(document)
    }

    fn rollback_document(
        &self,
        mut document: JournalDocument,
        reason: RollbackReason,
    ) -> Result<RecoveryOutcome, UpdateManagerError> {
        let pending = document
            .pending
            .clone()
            .ok_or(UpdateManagerError::InvalidJournal)?;
        self.validate_slot_reference(&pending.previous)?;
        self.pointer.switch(&pending.previous.slot_id)?;
        let previous_generation = document.advance_generation()?;
        document.active = pending.previous.clone();
        document.previous = self
            .validate_slot_reference(&pending.target)
            .is_ok()
            .then_some(pending.target.clone());
        document.pending = None;
        document.last_rollback = Some(reason);
        document.complete_operation(
            &pending.operation_id,
            OperationOutcome::RolledBack { reason },
        )?;
        self.state.store(&document, Some(previous_generation))?;
        Ok(RecoveryOutcome::RolledBack {
            active_slot_id: pending.previous.slot_id,
            rejected_slot_id: pending.target.slot_id,
            reason,
        })
    }

    fn load_required(&self) -> Result<JournalDocument, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.schema_version != model::JOURNAL_SCHEMA_VERSION {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(document)
    }

    fn load_native_active(&self) -> Result<JournalDocument, UpdateManagerError> {
        let document = self
            .state
            .load()?
            .ok_or(UpdateManagerError::NotInitialized)?;
        if document.schema_version != model::NATIVE_LIFECYCLE_SCHEMA_VERSION
            || document.native_rollback.is_some()
            || document.native_completion.is_some()
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(document)
    }

    fn validate_referenced_slots(
        &self,
        document: &JournalDocument,
    ) -> Result<(), UpdateManagerError> {
        let mut references = BTreeMap::new();
        for reference in [
            Some(&document.active),
            Some(&document.confirmed),
            document.previous.as_ref(),
        ]
        .into_iter()
        .flatten()
        .chain(
            document
                .pending
                .as_ref()
                .into_iter()
                .flat_map(|pending| [&pending.previous, &pending.target]),
        ) {
            references
                .entry(reference.slot_id.as_str())
                .or_insert(reference);
        }
        for reference in references.values() {
            let staged = self
                .stager
                .inspect_slot(&reference.slot_id, &self.trust_root)?;
            if !reference.matches_staged(&staged) {
                return Err(UpdateManagerError::SlotProofMismatch);
            }
            self.enforce_system_compatibility(&staged)?;
        }
        Ok(())
    }

    fn validate_rollback_anchor(
        &self,
        document: &JournalDocument,
        pending: &PendingActivation,
    ) -> Result<(), UpdateManagerError> {
        self.validate_slot_reference(&pending.previous)?;
        self.validate_slot_reference(&document.confirmed)?;
        Ok(())
    }

    fn validate_slot_reference(&self, reference: &SlotReference) -> Result<(), UpdateManagerError> {
        let staged = self
            .stager
            .inspect_slot(&reference.slot_id, &self.trust_root)?;
        if !reference.matches_staged(&staged) {
            return Err(UpdateManagerError::SlotProofMismatch);
        }
        self.enforce_system_compatibility(&staged)
    }

    fn enforce_system_compatibility(
        &self,
        staged: &StagedApplication,
    ) -> Result<(), UpdateManagerError> {
        if staged.manifest.minimum_system_sequence > self.system_sequence {
            return Err(UpdateManagerError::MinimumSystemSequence);
        }
        Ok(())
    }
}

fn validate_clock(clock: ClockSnapshot) -> Result<(), UpdateManagerError> {
    if clock.boot_id.iter().all(|byte| *byte == 0) {
        return Err(UpdateManagerError::InvalidClock);
    }
    Ok(())
}

fn validate_operation_material(
    operation_id: &[u8; 16],
    health_binding: &[u8; 32],
) -> Result<(), UpdateManagerError> {
    if operation_id.iter().all(|byte| *byte == 0) || health_binding.iter().all(|byte| *byte == 0) {
        return Err(UpdateManagerError::InvalidOperation);
    }
    Ok(())
}

fn awaiting_outcome(operation_id: [u8; 16], pending: &PendingActivation) -> ActivationOutcome {
    ActivationOutcome::AwaitingHealth {
        operation_id,
        slot_id: pending.target.slot_id.clone(),
        deadline_monotonic_ms: pending.deadline_monotonic_ms,
    }
}

fn confirmation_from_recovery(outcome: RecoveryOutcome) -> ConfirmationOutcome {
    match outcome {
        RecoveryOutcome::RolledBack { reason, .. } => ConfirmationOutcome::RolledBack { reason },
        RecoveryOutcome::Clean
        | RecoveryOutcome::AwaitingHealth { .. }
        | RecoveryOutcome::RepairedPointer { .. }
        | RecoveryOutcome::DroppedInvalidPrevious { .. } => {
            unreachable!("confirmation rollback always returns RolledBack")
        }
    }
}

fn protected_slot_ids(document: &JournalDocument) -> BTreeSet<String> {
    let mut protected = BTreeSet::from([
        document.active.slot_id.clone(),
        document.confirmed.slot_id.clone(),
    ]);
    if let Some(previous) = &document.previous {
        protected.insert(previous.slot_id.clone());
    }
    if let Some(pending) = &document.pending {
        protected.insert(pending.previous.slot_id.clone());
        protected.insert(pending.target.slot_id.clone());
    }
    protected
}

fn status_from_document(document: &JournalDocument) -> ManagerStatus {
    ManagerStatus {
        generation: document.generation,
        floor_sequence: document.floor_sequence,
        active_slot_id: document.active.slot_id.clone(),
        confirmed_slot_id: document.confirmed.slot_id.clone(),
        previous_slot_id: document.previous.as_ref().map(|slot| slot.slot_id.clone()),
        pending: document.pending.as_ref().map(|pending| PendingStatus {
            operation_id: pending.operation_id.clone(),
            target_slot_id: pending.target.slot_id.clone(),
            previous_slot_id: pending.previous.slot_id.clone(),
            boot_id: pending.boot_id.clone(),
            deadline_monotonic_ms: pending.deadline_monotonic_ms,
            phase: match pending.phase {
                PendingPhase::Prepared => PendingPhaseStatus::Prepared,
                PendingPhase::AwaitingHealth => PendingPhaseStatus::AwaitingHealth,
            },
        }),
        last_rollback: document.last_rollback,
    }
}

#[cfg(test)]
mod tests;
