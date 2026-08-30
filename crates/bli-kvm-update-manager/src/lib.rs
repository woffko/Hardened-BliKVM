mod journal;
mod model;
mod pointer;

use std::{collections::BTreeMap, io, path::PathBuf};

use bli_kvm_update_stager::{ApplicationSlotStager, ApplicationStageError, StagedApplication};
use bli_kvm_update_verifier::ApplicationTrustRoot;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use journal::StateStore;
use model::{
    JournalDocument, OperationOutcome, PendingActivation, PendingPhase, SlotReference, encode_hex,
};
use pointer::PointerStore;

pub const DEFAULT_HEALTH_TIMEOUT_MS: u64 = 60_000;
pub const MINIMUM_HEALTH_TIMEOUT_MS: u64 = 10_000;
pub const MAXIMUM_HEALTH_TIMEOUT_MS: u64 = 10 * 60_000;

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

#[derive(Debug)]
enum PrepareResult {
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
        let stager = ApplicationSlotStager::open(slot_root.clone())?;
        let pointer = PointerStore::open(slot_root)?;
        let state = StateStore::open(state_root)?;
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

    pub fn status(&self) -> Result<ManagerStatus, UpdateManagerError> {
        let document = self.load_required()?;
        self.validate_referenced_slots(&document)?;
        if self.pointer.read()?.as_deref() != Some(document.active.slot_id.as_str()) {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        Ok(status_from_document(&document))
    }

    pub fn activate(
        &self,
        request: ActivationRequest,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        match self.prepare_activation(&request)? {
            PrepareResult::Immediate(outcome) => Ok(outcome),
            PrepareResult::Prepared => self.switch_prepared_activation(&request),
        }
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
        validate_operation_material(&request.operation_id, &request.health_binding)?;
        validate_clock(request.clock)?;
        let operation_id = encode_hex(&request.operation_id);
        let health_binding = encode_hex(&request.health_binding);
        let mut document = self.load_required()?;
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
        self.state.store(&document, Some(previous_generation))?;
        Ok(PrepareResult::Prepared)
    }

    fn switch_prepared_activation(
        &self,
        request: &ActivationRequest,
    ) -> Result<ActivationOutcome, UpdateManagerError> {
        let operation_id = encode_hex(&request.operation_id);
        let mut document = self.load_required()?;
        self.validate_referenced_slots(&document)?;
        let pending = document
            .pending
            .clone()
            .ok_or(UpdateManagerError::InvalidJournal)?;
        if pending.operation_id != operation_id
            || pending.health_binding != encode_hex(&request.health_binding)
            || pending.phase != PendingPhase::Prepared
            || self.pointer.read()?.as_deref() != Some(pending.previous.slot_id.as_str())
        {
            return Err(UpdateManagerError::RecoveryRequired);
        }
        self.pointer.switch(&pending.target.slot_id)?;
        let previous_generation = document.advance_generation()?;
        document.active = pending.target.clone();
        document.previous = Some(pending.previous.clone());
        document
            .pending
            .as_mut()
            .ok_or(UpdateManagerError::InvalidJournal)?
            .phase = PendingPhase::AwaitingHealth;
        self.state.store(&document, Some(previous_generation))?;
        Ok(awaiting_outcome(request.operation_id, &pending))
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
        self.state.load()?.ok_or(UpdateManagerError::NotInitialized)
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
