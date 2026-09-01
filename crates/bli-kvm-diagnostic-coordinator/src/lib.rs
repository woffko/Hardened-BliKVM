#![no_std]
#![forbid(unsafe_code)]

//! Bounded write-before-ack coordination for sealed diagnostic windows.
//!
//! This crate owns no runtime, transport, key, path, clock, retry loop, or
//! background task. A caller supplies one fresh collector observation and one
//! externally retained nonzero generation floor per step. The coordinator
//! returns an acknowledgement permit only after the exact pending journal has
//! been durably written, authenticated, re-selected without degradation, and
//! covered by that external floor.

use core::{fmt, num::NonZeroU64};

use bli_kvm_diagnostic_journal::{
    AcknowledgementResult, CollectorObservation, DiagnosticBinding, DiagnosticJournalEntryState,
    DiagnosticJournalError, DiagnosticSnapshot, JournalMutation, ReconciliationAction,
    SelectedDiagnosticJournal, plan_acknowledgement_result, plan_reconcile,
};

/// Storage boundary required by the coordinator.
///
/// `persist` must synchronously complete the repository's write and sync
/// protocol. The coordinator always calls `load` afterwards and independently
/// verifies the authenticated selected generation and complete journal state.
/// Implementations must not repair degraded copies implicitly.
pub trait DiagnosticJournalRepository {
    type Error;

    fn load(&mut self, minimum_generation: u64) -> Result<SelectedDiagnosticJournal, Self::Error>;

    fn persist(
        &mut self,
        minimum_generation: u64,
        mutation: &JournalMutation,
    ) -> Result<(), Self::Error>;
}

/// A generation value retained outside the diagnostic journal's filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DiagnosticGenerationFloor(NonZeroU64);

impl DiagnosticGenerationFloor {
    pub const fn new(generation: u64) -> Option<Self> {
        match NonZeroU64::new(generation) {
            Some(generation) => Some(Self(generation)),
            None => None,
        }
    }

    pub const fn get(self) -> u64 {
        self.0.get()
    }
}

/// One caller-observed collector result. No operation is retried internally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCollectorObservation<'a> {
    NoSealed,
    Sealed(DiagnosticSnapshot<'a>),
    Unavailable,
}

/// Result of exactly one acknowledgement transport attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticAcknowledgementCompletion {
    Applied,
    NoSealed,
    Mismatch,
    CollectorUnavailable,
    NotSent,
    DeliveryAmbiguous,
}

/// Fail-closed coordinator errors.
///
/// The custom `Debug` implementation deliberately does not format a
/// repository error because it could contain a path or other private context.
pub enum DiagnosticCoordinatorError<E> {
    Repository(E),
    Journal(DiagnosticJournalError),
    StoreDegraded {
        generation: u64,
    },
    RollbackFloorViolation {
        selected_generation: u64,
        minimum_generation: u64,
    },
    FloorAdvanceRequired {
        generation: u64,
    },
    PersistenceVerificationFailed,
    AcknowledgementRecoveryRequired,
    AcknowledgementNotReady,
    RecoveryConflict,
    InvariantViolation,
}

impl<E> DiagnosticCoordinatorError<E> {
    pub const fn repository_error(&self) -> Option<&E> {
        match self {
            Self::Repository(error) => Some(error),
            _ => None,
        }
    }

    pub const fn required_generation(&self) -> Option<u64> {
        match self {
            Self::FloorAdvanceRequired { generation } => Some(*generation),
            _ => None,
        }
    }
}

impl<E> fmt::Debug for DiagnosticCoordinatorError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(_) => formatter.write_str("Repository"),
            Self::Journal(error) => formatter.debug_tuple("Journal").field(error).finish(),
            Self::StoreDegraded { generation } => formatter
                .debug_struct("StoreDegraded")
                .field("generation", generation)
                .finish(),
            Self::RollbackFloorViolation {
                selected_generation,
                minimum_generation,
            } => formatter
                .debug_struct("RollbackFloorViolation")
                .field("selected_generation", selected_generation)
                .field("minimum_generation", minimum_generation)
                .finish(),
            Self::FloorAdvanceRequired { generation } => formatter
                .debug_struct("FloorAdvanceRequired")
                .field("generation", generation)
                .finish(),
            Self::PersistenceVerificationFailed => {
                formatter.write_str("PersistenceVerificationFailed")
            }
            Self::AcknowledgementRecoveryRequired => {
                formatter.write_str("AcknowledgementRecoveryRequired")
            }
            Self::AcknowledgementNotReady => formatter.write_str("AcknowledgementNotReady"),
            Self::RecoveryConflict => formatter.write_str("RecoveryConflict"),
            Self::InvariantViolation => formatter.write_str("InvariantViolation"),
        }
    }
}

impl<E> fmt::Display for DiagnosticCoordinatorError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Repository(_) => formatter.write_str("diagnostic repository operation failed"),
            Self::Journal(_) => formatter.write_str("diagnostic journal transition failed"),
            Self::StoreDegraded { .. } => {
                formatter.write_str("diagnostic repository requires explicit repair")
            }
            Self::RollbackFloorViolation { .. } => {
                formatter.write_str("diagnostic rollback floor was violated")
            }
            Self::FloorAdvanceRequired { .. } => {
                formatter.write_str("external diagnostic rollback floor must advance")
            }
            Self::PersistenceVerificationFailed => {
                formatter.write_str("diagnostic persistence verification failed")
            }
            Self::AcknowledgementRecoveryRequired => {
                formatter.write_str("diagnostic acknowledgement recovery is required")
            }
            Self::AcknowledgementNotReady => {
                formatter.write_str("fresh diagnostic acknowledgement readiness is required")
            }
            Self::RecoveryConflict => {
                formatter.write_str("diagnostic acknowledgement recovery conflicted")
            }
            Self::InvariantViolation => {
                formatter.write_str("diagnostic coordinator invariant failed")
            }
        }
    }
}

/// Result of reconciling one fresh collector observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticObservationOutcome {
    Idle { generation: u64 },
    CollectorUnavailable { generation: u64 },
    ReadCollector { generation: u64 },
    AdvanceFloor { generation: u64 },
    AcknowledgementReady { generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticAcknowledgementOutcome {
    Pending { generation: u64 },
    ReadCollector { generation: u64 },
    AdvanceFloor { generation: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticRecoveryOutcome {
    NoCancellation,
    AdvanceFloor {
        generation: u64,
    },
    ReadCollector {
        generation: u64,
    },
    Terminal {
        generation: u64,
        state: DiagnosticJournalEntryState,
    },
}

/// Single-process coordinator. It intentionally implements neither `Clone`
/// nor `Debug`.
pub struct DiagnosticCoordinator<R> {
    repository: R,
    ready_acknowledgement: Option<ReadyAcknowledgement>,
    cancelled_acknowledgement: Option<DiagnosticBinding>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ReadyAcknowledgement {
    binding: DiagnosticBinding,
    generation: u64,
}

impl<R: DiagnosticJournalRepository> DiagnosticCoordinator<R> {
    pub const fn new(repository: R) -> Self {
        Self {
            repository,
            ready_acknowledgement: None,
            cancelled_acknowledgement: None,
        }
    }

    pub const fn has_cancelled_acknowledgement(&self) -> bool {
        self.cancelled_acknowledgement.is_some()
    }

    pub const fn has_acknowledgement_ready(&self) -> bool {
        self.ready_acknowledgement.is_some()
    }

    pub fn into_repository(self) -> R {
        self.repository
    }

    /// Reconciles one fresh observation and performs no collector I/O.
    ///
    /// One call performs at most one initial load, one persistence operation,
    /// and one authenticated post-persistence load. It never repairs or
    /// retries. A newly persisted generation is returned as `AdvanceFloor`;
    /// the caller must durably advance its external floor and make a fresh
    /// collector observation before any acknowledgement can be permitted.
    pub fn observe<'snapshot>(
        &mut self,
        floor: DiagnosticGenerationFloor,
        observation: DiagnosticCollectorObservation<'snapshot>,
    ) -> Result<DiagnosticObservationOutcome, DiagnosticCoordinatorError<R::Error>> {
        if self.cancelled_acknowledgement.is_some() {
            return Err(DiagnosticCoordinatorError::AcknowledgementRecoveryRequired);
        }
        self.ready_acknowledgement = None;

        let selected = self.load_clean(floor)?;
        if selected.generation() > floor.get() {
            return Ok(DiagnosticObservationOutcome::AdvanceFloor {
                generation: selected.generation(),
            });
        }

        let generation = selected.generation();
        let journal_observation = match observation {
            DiagnosticCollectorObservation::NoSealed => CollectorObservation::NoSealed,
            DiagnosticCollectorObservation::Sealed(snapshot) => {
                CollectorObservation::Sealed(snapshot)
            }
            DiagnosticCollectorObservation::Unavailable => {
                return Ok(DiagnosticObservationOutcome::CollectorUnavailable { generation });
            }
        };
        let plan = plan_reconcile(&selected, journal_observation)
            .map_err(DiagnosticCoordinatorError::Journal)?;

        match plan.action() {
            ReconciliationAction::Idle => Ok(DiagnosticObservationOutcome::Idle { generation }),
            ReconciliationAction::ReadCollector => {
                Ok(DiagnosticObservationOutcome::ReadCollector { generation })
            }
            ReconciliationAction::Persist => {
                let mutation = plan
                    .mutation()
                    .ok_or(DiagnosticCoordinatorError::InvariantViolation)?;
                let generation = self.persist_verified(floor, mutation)?;
                Ok(DiagnosticObservationOutcome::AdvanceFloor { generation })
            }
            ReconciliationAction::Acknowledge(binding) => {
                if plan.mutation().is_some()
                    || selected.journal().entry(binding).is_none_or(|entry| {
                        entry.state() != DiagnosticJournalEntryState::PendingAcknowledgement
                    })
                {
                    return Err(DiagnosticCoordinatorError::InvariantViolation);
                }
                self.ready_acknowledgement = Some(ReadyAcknowledgement {
                    binding,
                    generation,
                });
                Ok(DiagnosticObservationOutcome::AcknowledgementReady { generation })
            }
        }
    }

    /// Re-loads the journal immediately before issuing one linear permit.
    ///
    /// Readiness is produced only by the preceding fresh collector observation
    /// and is consumed on success. Any error retains readiness, but callers
    /// must not turn the error into an automatic retry loop.
    pub fn prepare_acknowledgement(
        &mut self,
        floor: DiagnosticGenerationFloor,
    ) -> Result<DiagnosticAcknowledgementPermit<'_, R>, DiagnosticCoordinatorError<R::Error>> {
        if self.cancelled_acknowledgement.is_some() {
            return Err(DiagnosticCoordinatorError::AcknowledgementRecoveryRequired);
        }
        let ready = self
            .ready_acknowledgement
            .ok_or(DiagnosticCoordinatorError::AcknowledgementNotReady)?;
        let selected = self.load_clean(floor)?;
        if selected.generation() > floor.get() {
            return Err(DiagnosticCoordinatorError::FloorAdvanceRequired {
                generation: selected.generation(),
            });
        }
        if selected.generation() != ready.generation
            || selected.journal().entry(ready.binding).is_none_or(|entry| {
                entry.state() != DiagnosticJournalEntryState::PendingAcknowledgement
            })
        {
            return Err(DiagnosticCoordinatorError::AcknowledgementNotReady);
        }
        self.ready_acknowledgement = None;
        Ok(DiagnosticAcknowledgementPermit {
            coordinator: self,
            binding: ready.binding,
            floor,
            authorizing_generation: ready.generation,
            armed: true,
        })
    }

    /// Persists cancellation ambiguity before any collector reread or retry.
    pub fn recover_cancelled_acknowledgement(
        &mut self,
        floor: DiagnosticGenerationFloor,
    ) -> Result<DiagnosticRecoveryOutcome, DiagnosticCoordinatorError<R::Error>> {
        let Some(binding) = self.cancelled_acknowledgement else {
            return Ok(DiagnosticRecoveryOutcome::NoCancellation);
        };
        let selected = self.load_clean(floor)?;
        if selected.generation() > floor.get() {
            return Ok(DiagnosticRecoveryOutcome::AdvanceFloor {
                generation: selected.generation(),
            });
        }
        let generation = selected.generation();
        let state = selected
            .journal()
            .entry(binding)
            .map(|entry| entry.state())
            .ok_or(DiagnosticCoordinatorError::RecoveryConflict)?;
        match state {
            DiagnosticJournalEntryState::PendingAcknowledgement => {
                let plan = plan_acknowledgement_result(
                    &selected,
                    binding,
                    AcknowledgementResult::DeliveryAmbiguous,
                )
                .map_err(DiagnosticCoordinatorError::Journal)?;
                if plan.action() != ReconciliationAction::Persist {
                    return Err(DiagnosticCoordinatorError::InvariantViolation);
                }
                let mutation = plan
                    .mutation()
                    .ok_or(DiagnosticCoordinatorError::InvariantViolation)?;
                let generation = self.persist_verified(floor, mutation)?;
                self.cancelled_acknowledgement = None;
                Ok(DiagnosticRecoveryOutcome::AdvanceFloor { generation })
            }
            DiagnosticJournalEntryState::AcknowledgementAmbiguous => {
                self.cancelled_acknowledgement = None;
                Ok(DiagnosticRecoveryOutcome::ReadCollector { generation })
            }
            DiagnosticJournalEntryState::Acknowledged | DiagnosticJournalEntryState::Orphaned => {
                self.cancelled_acknowledgement = None;
                Ok(DiagnosticRecoveryOutcome::Terminal { generation, state })
            }
        }
    }

    fn complete_acknowledgement(
        &mut self,
        binding: DiagnosticBinding,
        floor: DiagnosticGenerationFloor,
        authorizing_generation: u64,
        completion: DiagnosticAcknowledgementCompletion,
    ) -> Result<DiagnosticAcknowledgementOutcome, DiagnosticCoordinatorError<R::Error>> {
        if completion == DiagnosticAcknowledgementCompletion::NotSent {
            return Ok(DiagnosticAcknowledgementOutcome::Pending {
                generation: authorizing_generation,
            });
        }

        let selected = self.load_clean(floor)?;
        if selected.generation() > floor.get() {
            return Err(DiagnosticCoordinatorError::FloorAdvanceRequired {
                generation: selected.generation(),
            });
        }
        if selected.generation() != authorizing_generation {
            return Err(DiagnosticCoordinatorError::InvariantViolation);
        }
        let result = match completion {
            DiagnosticAcknowledgementCompletion::Applied => AcknowledgementResult::Applied,
            DiagnosticAcknowledgementCompletion::NoSealed => AcknowledgementResult::NoSealed,
            DiagnosticAcknowledgementCompletion::Mismatch => AcknowledgementResult::Mismatch,
            DiagnosticAcknowledgementCompletion::CollectorUnavailable
            | DiagnosticAcknowledgementCompletion::DeliveryAmbiguous => {
                AcknowledgementResult::DeliveryAmbiguous
            }
            DiagnosticAcknowledgementCompletion::NotSent => {
                return Err(DiagnosticCoordinatorError::InvariantViolation);
            }
        };
        let plan = plan_acknowledgement_result(&selected, binding, result)
            .map_err(DiagnosticCoordinatorError::Journal)?;
        match plan.action() {
            ReconciliationAction::ReadCollector if plan.mutation().is_none() => {
                Ok(DiagnosticAcknowledgementOutcome::ReadCollector {
                    generation: selected.generation(),
                })
            }
            ReconciliationAction::Persist => {
                let mutation = plan
                    .mutation()
                    .ok_or(DiagnosticCoordinatorError::InvariantViolation)?;
                let generation = self.persist_verified(floor, mutation)?;
                Ok(DiagnosticAcknowledgementOutcome::AdvanceFloor { generation })
            }
            _ => Err(DiagnosticCoordinatorError::InvariantViolation),
        }
    }

    fn load_clean(
        &mut self,
        floor: DiagnosticGenerationFloor,
    ) -> Result<SelectedDiagnosticJournal, DiagnosticCoordinatorError<R::Error>> {
        let selected = self
            .repository
            .load(floor.get())
            .map_err(DiagnosticCoordinatorError::Repository)?;
        if selected.generation() < floor.get() {
            return Err(DiagnosticCoordinatorError::RollbackFloorViolation {
                selected_generation: selected.generation(),
                minimum_generation: floor.get(),
            });
        }
        if selected.degraded() {
            return Err(DiagnosticCoordinatorError::StoreDegraded {
                generation: selected.generation(),
            });
        }
        Ok(selected)
    }

    fn persist_verified(
        &mut self,
        floor: DiagnosticGenerationFloor,
        mutation: &JournalMutation,
    ) -> Result<u64, DiagnosticCoordinatorError<R::Error>> {
        self.repository
            .persist(floor.get(), mutation)
            .map_err(DiagnosticCoordinatorError::Repository)?;
        let selected = self.load_clean(floor)?;
        if selected.generation() != mutation.generation()
            || selected.journal() != mutation.journal()
        {
            return Err(DiagnosticCoordinatorError::PersistenceVerificationFailed);
        }
        Ok(selected.generation())
    }

    fn note_cancelled_acknowledgement(&mut self, binding: DiagnosticBinding) {
        match self.cancelled_acknowledgement {
            None => self.cancelled_acknowledgement = Some(binding),
            Some(existing) => debug_assert_eq!(existing, binding),
        }
    }
}

/// Linear capability for one exact acknowledgement attempt.
///
/// Dropping the permit without reporting a completion records in-process
/// delivery uncertainty. The coordinator then refuses all observations until
/// `recover_cancelled_acknowledgement` durably records ambiguity or proves that
/// a terminal state was already selected.
pub struct DiagnosticAcknowledgementPermit<'a, R: DiagnosticJournalRepository> {
    coordinator: &'a mut DiagnosticCoordinator<R>,
    binding: DiagnosticBinding,
    floor: DiagnosticGenerationFloor,
    authorizing_generation: u64,
    armed: bool,
}

impl<R: DiagnosticJournalRepository> DiagnosticAcknowledgementPermit<'_, R> {
    pub const fn binding(&self) -> DiagnosticBinding {
        self.binding
    }

    pub const fn authorizing_generation(&self) -> u64 {
        self.authorizing_generation
    }

    pub fn complete(
        mut self,
        completion: DiagnosticAcknowledgementCompletion,
    ) -> Result<DiagnosticAcknowledgementOutcome, DiagnosticCoordinatorError<R::Error>> {
        let outcome = self.coordinator.complete_acknowledgement(
            self.binding,
            self.floor,
            self.authorizing_generation,
            completion,
        );
        if outcome.is_ok() {
            self.armed = false;
        }
        outcome
    }
}

impl<R: DiagnosticJournalRepository> fmt::Debug for DiagnosticAcknowledgementPermit<'_, R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticAcknowledgementPermit")
            .field("authorizing_generation", &self.authorizing_generation)
            .field("armed", &self.armed)
            .finish_non_exhaustive()
    }
}

impl<R: DiagnosticJournalRepository> Drop for DiagnosticAcknowledgementPermit<'_, R> {
    fn drop(&mut self) {
        if self.armed {
            self.coordinator
                .note_cancelled_acknowledgement(self.binding);
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use bli_kvm_diagnostic_export::{
        DiagnosticBundle, DiagnosticComponent, DiagnosticEvent, DiagnosticExporter,
        DiagnosticRecord,
    };
    use bli_kvm_diagnostic_journal::{
        CopyIndex, DiagnosticJournal, DiagnosticJournalKey, DiagnosticJournalKeyRing,
        DiagnosticJournalNonce, RawDiagnosticJournalCopy, plan_initialize, plan_update,
        select_copies,
    };
    use std::{boxed::Box, format};

    const FLOOR_ONE: DiagnosticGenerationFloor = match DiagnosticGenerationFloor::new(1) {
        Some(floor) => floor,
        None => panic!("test generation must be nonzero"),
    };

    const fn copy_offset(index: CopyIndex) -> usize {
        match index {
            CopyIndex::A => 0,
            CopyIndex::B => 1,
        }
    }

    const fn checked_test_floor(value: u64) -> DiagnosticGenerationFloor {
        match DiagnosticGenerationFloor::new(value) {
            Some(floor) => floor,
            None => panic!("test generation must be nonzero"),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Journal(DiagnosticJournalError),
        Injected(&'static str),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PersistFault {
        None,
        BeforeWrite,
        AfterFirst,
        AfterBoth,
    }

    struct FakeRepository {
        key: DiagnosticJournalKey,
        copies: [Option<Box<RawDiagnosticJournalCopy>>; 2],
        nonce_counter: u64,
        fault: PersistFault,
        loads: usize,
        persists: usize,
    }

    impl FakeRepository {
        fn initialized() -> Self {
            let key = DiagnosticJournalKey::new(7, [0x5a; 32]).expect("valid key");
            let ring = DiagnosticJournalKeyRing::new(&key, None).expect("valid ring");
            let nonce = DiagnosticJournalNonce::new(nonce_bytes(1)).expect("nonzero nonce");
            let plan = plan_initialize(&DiagnosticJournal::empty(), &ring, nonce)
                .expect("initialization plan");
            let bytes = *plan.bytes();
            Self {
                key,
                copies: [Some(Box::new(bytes)), Some(Box::new(bytes))],
                nonce_counter: 2,
                fault: PersistFault::None,
                loads: 0,
                persists: 0,
            }
        }

        fn select(&self, minimum_generation: u64) -> Result<SelectedDiagnosticJournal, FakeError> {
            let ring =
                DiagnosticJournalKeyRing::new(&self.key, None).map_err(FakeError::Journal)?;
            select_copies(
                self.copies[0].as_deref().map(|copy| copy.as_slice()),
                self.copies[1].as_deref().map(|copy| copy.as_slice()),
                &ring,
                minimum_generation,
            )
            .map_err(FakeError::Journal)
        }

        fn replace(&mut self, index: CopyIndex, bytes: RawDiagnosticJournalCopy) {
            self.copies[copy_offset(index)] = Some(Box::new(bytes));
        }

        fn state(&self, binding: DiagnosticBinding) -> Option<DiagnosticJournalEntryState> {
            self.select(1)
                .ok()
                .and_then(|selected| selected.journal().entry(binding).map(|entry| entry.state()))
        }

        fn generation(&self) -> u64 {
            self.select(1).expect("selected journal").generation()
        }
    }

    impl DiagnosticJournalRepository for FakeRepository {
        type Error = FakeError;

        fn load(
            &mut self,
            minimum_generation: u64,
        ) -> Result<SelectedDiagnosticJournal, Self::Error> {
            self.loads += 1;
            self.select(minimum_generation)
        }

        fn persist(
            &mut self,
            minimum_generation: u64,
            mutation: &JournalMutation,
        ) -> Result<(), Self::Error> {
            self.persists += 1;
            let fault = core::mem::replace(&mut self.fault, PersistFault::None);
            if fault == PersistFault::BeforeWrite {
                return Err(FakeError::Injected("private-before-write-path"));
            }
            let selected = self.select(minimum_generation)?;
            let ring =
                DiagnosticJournalKeyRing::new(&self.key, None).map_err(FakeError::Journal)?;
            let nonce = DiagnosticJournalNonce::new(nonce_bytes(self.nonce_counter))
                .expect("nonzero nonce");
            self.nonce_counter += 1;
            let plan =
                plan_update(&selected, mutation, &ring, nonce).map_err(FakeError::Journal)?;
            let bytes = *plan.bytes();
            self.replace(plan.first(), bytes);
            if fault == PersistFault::AfterFirst {
                return Err(FakeError::Injected("private-after-first-path"));
            }
            self.replace(plan.second(), bytes);
            if fault == PersistFault::AfterBoth {
                return Err(FakeError::Injected("private-after-both-path"));
            }
            Ok(())
        }
    }

    fn nonce_bytes(counter: u64) -> [u8; 24] {
        let mut bytes = [0_u8; 24];
        bytes[..8].copy_from_slice(&counter.to_le_bytes());
        bytes
    }

    fn floor(generation: u64) -> DiagnosticGenerationFloor {
        checked_test_floor(generation)
    }

    fn binding(instance_id: u64, window_id: u32) -> DiagnosticBinding {
        DiagnosticBinding::new(instance_id, window_id).expect("valid binding")
    }

    fn bundle(offset_ms: u32) -> DiagnosticBundle {
        let mut exporter = DiagnosticExporter::new();
        exporter
            .push(
                DiagnosticRecord::new(
                    offset_ms,
                    DiagnosticComponent::PlatformBroker,
                    DiagnosticEvent::Ready,
                )
                .expect("valid record"),
            )
            .expect("bundle capacity");
        exporter.export().expect("valid bundle")
    }

    fn snapshot<'a>(
        binding: DiagnosticBinding,
        bundle: &'a DiagnosticBundle,
    ) -> DiagnosticSnapshot<'a> {
        DiagnosticSnapshot::new(binding, 0, bundle).expect("valid snapshot")
    }

    fn coordinator_with_pending(
        binding: DiagnosticBinding,
        bundle: &DiagnosticBundle,
    ) -> DiagnosticCoordinator<FakeRepository> {
        let mut coordinator = DiagnosticCoordinator::new(FakeRepository::initialized());
        let result = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(binding, bundle)),
            )
            .expect("persist pending");
        assert!(matches!(
            result,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 2 }
        ));
        coordinator
    }

    fn prepare_permit<'a>(
        coordinator: &'a mut DiagnosticCoordinator<FakeRepository>,
        floor: DiagnosticGenerationFloor,
        binding: DiagnosticBinding,
        bundle: &DiagnosticBundle,
    ) -> DiagnosticAcknowledgementPermit<'a, FakeRepository> {
        assert_eq!(
            coordinator
                .observe(
                    floor,
                    DiagnosticCollectorObservation::Sealed(snapshot(binding, bundle)),
                )
                .expect("ack readiness"),
            DiagnosticObservationOutcome::AcknowledgementReady {
                generation: floor.get(),
            }
        );
        coordinator
            .prepare_acknowledgement(floor)
            .expect("ack permit")
    }

    #[test]
    fn generation_floor_rejects_zero() {
        assert_eq!(DiagnosticGenerationFloor::new(0), None);
        assert_eq!(
            DiagnosticGenerationFloor::new(9).map(|value| value.get()),
            Some(9)
        );
    }

    #[test]
    fn empty_journal_and_no_sealed_are_idle() {
        let mut coordinator = DiagnosticCoordinator::new(FakeRepository::initialized());
        let result = coordinator
            .observe(FLOOR_ONE, DiagnosticCollectorObservation::NoSealed)
            .expect("idle");
        assert!(matches!(
            result,
            DiagnosticObservationOutcome::Idle { generation: 1 }
        ));
        let repository = coordinator.into_repository();
        assert_eq!((repository.loads, repository.persists), (1, 0));
    }

    #[test]
    fn unavailable_collector_does_not_mutate() {
        let mut coordinator = DiagnosticCoordinator::new(FakeRepository::initialized());
        let result = coordinator
            .observe(FLOOR_ONE, DiagnosticCollectorObservation::Unavailable)
            .expect("unavailable");
        assert!(matches!(
            result,
            DiagnosticObservationOutcome::CollectorUnavailable { generation: 1 }
        ));
        let repository = coordinator.into_repository();
        assert_eq!((repository.loads, repository.persists), (1, 0));
    }

    #[test]
    fn new_snapshot_is_reselected_then_requires_external_floor() {
        let selected_binding = binding(11, 3);
        let selected_bundle = bundle(4);
        let coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[test]
    fn floor_must_catch_up_before_acknowledgement_permit() {
        let selected_binding = binding(12, 4);
        let selected_bundle = bundle(5);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        let behind = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect("floor request");
        assert!(matches!(
            behind,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 2 }
        ));

        let permit = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        );
        assert_eq!(permit.binding(), selected_binding);
        assert_eq!(permit.authorizing_generation(), 2);
        let completion = permit
            .complete(DiagnosticAcknowledgementCompletion::NotSent)
            .expect("not sent");
        assert_eq!(
            completion,
            DiagnosticAcknowledgementOutcome::Pending { generation: 2 }
        );
    }

    #[test]
    fn permit_requires_fresh_readiness_and_a_second_authenticated_load() {
        let selected_binding = binding(120, 4);
        let selected_bundle = bundle(5);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        assert!(matches!(
            coordinator.prepare_acknowledgement(floor(2)),
            Err(DiagnosticCoordinatorError::AcknowledgementNotReady)
        ));
        let loads_before_observation = coordinator.repository.loads;
        assert_eq!(
            coordinator
                .observe(
                    floor(2),
                    DiagnosticCollectorObservation::Sealed(snapshot(
                        selected_binding,
                        &selected_bundle,
                    )),
                )
                .expect("ack readiness"),
            DiagnosticObservationOutcome::AcknowledgementReady { generation: 2 }
        );
        assert!(coordinator.has_acknowledgement_ready());
        let permit = coordinator
            .prepare_acknowledgement(floor(2))
            .expect("second authenticated load");
        assert_eq!(permit.authorizing_generation(), 2);
        permit
            .complete(DiagnosticAcknowledgementCompletion::NotSent)
            .expect("not sent");
        assert_eq!(coordinator.repository.loads, loads_before_observation + 2);
        assert!(!coordinator.has_acknowledgement_ready());
        assert!(matches!(
            coordinator.prepare_acknowledgement(floor(2)),
            Err(DiagnosticCoordinatorError::AcknowledgementNotReady)
        ));
    }

    #[test]
    fn pending_entry_and_no_sealed_are_durably_orphaned() {
        let selected_binding = binding(121, 5);
        let selected_bundle = bundle(6);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        assert_eq!(
            coordinator
                .observe(floor(2), DiagnosticCollectorObservation::NoSealed)
                .expect("orphan pending"),
            DiagnosticObservationOutcome::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::Orphaned)
        );
    }

    #[test]
    fn applied_acknowledgement_is_persisted_before_progress() {
        let selected_binding = binding(13, 5);
        let selected_bundle = bundle(6);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        let completion = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        )
        .complete(DiagnosticAcknowledgementCompletion::Applied)
        .expect("record applied");
        assert_eq!(
            completion,
            DiagnosticAcknowledgementOutcome::AdvanceFloor { generation: 3 }
        );
        let repository = coordinator.into_repository();
        assert_eq!(repository.generation(), 3);
        assert_eq!(
            repository.state(selected_binding),
            Some(DiagnosticJournalEntryState::Acknowledged)
        );
    }

    #[test]
    fn explicit_no_sealed_acknowledgement_orphans_pending_entry() {
        let selected_binding = binding(14, 6);
        let selected_bundle = bundle(7);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        let completion = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        )
        .complete(DiagnosticAcknowledgementCompletion::NoSealed)
        .expect("record orphan");
        assert_eq!(
            completion,
            DiagnosticAcknowledgementOutcome::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::Orphaned)
        );
    }

    #[test]
    fn mismatch_requests_fresh_read_without_mutation() {
        let selected_binding = binding(15, 7);
        let selected_bundle = bundle(8);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        let before_persists = coordinator.repository.persists;
        let completion = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        )
        .complete(DiagnosticAcknowledgementCompletion::Mismatch)
        .expect("read required");
        assert_eq!(
            completion,
            DiagnosticAcknowledgementOutcome::ReadCollector { generation: 2 }
        );
        assert_eq!(coordinator.repository.persists, before_persists);
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[test]
    fn transport_ambiguity_is_durable_before_reread() {
        let selected_binding = binding(16, 8);
        let selected_bundle = bundle(9);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        let completion = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        )
        .complete(DiagnosticAcknowledgementCompletion::DeliveryAmbiguous)
        .expect("record ambiguity");
        assert_eq!(
            completion,
            DiagnosticAcknowledgementOutcome::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            coordinator.repository.state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );

        let reread = coordinator
            .observe(
                floor(3),
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect("same sealed persists pending");
        assert!(matches!(
            reread,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 4 }
        ));
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[test]
    fn collector_unavailable_acknowledgement_is_conservative_ambiguity() {
        let selected_binding = binding(17, 9);
        let selected_bundle = bundle(10);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        assert_eq!(
            prepare_permit(
                &mut coordinator,
                floor(2),
                selected_binding,
                &selected_bundle,
            )
            .complete(DiagnosticAcknowledgementCompletion::CollectorUnavailable)
            .expect("record ambiguity"),
            DiagnosticAcknowledgementOutcome::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );
    }

    #[test]
    fn dropped_permit_blocks_observation_until_ambiguity_is_persisted() {
        let selected_binding = binding(18, 10);
        let selected_bundle = bundle(11);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        drop(prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        ));
        assert!(coordinator.has_cancelled_acknowledgement());
        assert!(matches!(
            coordinator.observe(floor(2), DiagnosticCollectorObservation::NoSealed),
            Err(DiagnosticCoordinatorError::AcknowledgementRecoveryRequired)
        ));
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(2))
                .expect("persist cancellation ambiguity"),
            DiagnosticRecoveryOutcome::AdvanceFloor { generation: 3 }
        );
        assert!(!coordinator.has_cancelled_acknowledgement());
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );
    }

    #[test]
    fn recovery_keeps_marker_while_floor_is_behind() {
        let selected_binding = binding(19, 11);
        let selected_bundle = bundle(12);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        drop(prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        ));
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(FLOOR_ONE)
                .expect("advance floor first"),
            DiagnosticRecoveryOutcome::AdvanceFloor { generation: 2 }
        );
        assert!(coordinator.has_cancelled_acknowledgement());
    }

    #[test]
    fn recovery_accepts_already_ambiguous_and_terminal_state() {
        let selected_binding = binding(20, 12);
        let selected_bundle = bundle(13);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        coordinator.cancelled_acknowledgement = Some(selected_binding);
        let pending_recovery = coordinator
            .recover_cancelled_acknowledgement(floor(2))
            .expect("persist ambiguity");
        assert_eq!(
            pending_recovery,
            DiagnosticRecoveryOutcome::AdvanceFloor { generation: 3 }
        );
        coordinator.cancelled_acknowledgement = Some(selected_binding);
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(3))
                .expect("already ambiguous"),
            DiagnosticRecoveryOutcome::ReadCollector { generation: 3 }
        );

        let reread = coordinator
            .observe(floor(3), DiagnosticCollectorObservation::NoSealed)
            .expect("orphan ambiguity");
        assert!(matches!(
            reread,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 4 }
        ));
        coordinator.cancelled_acknowledgement = Some(selected_binding);
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(4))
                .expect("terminal orphan"),
            DiagnosticRecoveryOutcome::Terminal {
                generation: 4,
                state: DiagnosticJournalEntryState::Orphaned,
            }
        );
    }

    #[test]
    fn recovery_conflict_retains_cancellation_marker() {
        let mut coordinator = DiagnosticCoordinator::new(FakeRepository::initialized());
        coordinator.cancelled_acknowledgement = Some(binding(21, 13));
        assert!(matches!(
            coordinator.recover_cancelled_acknowledgement(FLOOR_ONE),
            Err(DiagnosticCoordinatorError::RecoveryConflict)
        ));
        assert!(coordinator.has_cancelled_acknowledgement());
    }

    #[test]
    fn snapshot_conflict_fails_without_write() {
        let selected_binding = binding(22, 14);
        let original_bundle = bundle(14);
        let conflicting_bundle = bundle(15);
        let mut coordinator = coordinator_with_pending(selected_binding, &original_bundle);
        let persists = coordinator.repository.persists;
        assert!(matches!(
            coordinator.observe(
                floor(2),
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &conflicting_bundle,
                )),
            ),
            Err(DiagnosticCoordinatorError::Journal(
                DiagnosticJournalError::SnapshotConflict
            ))
        ));
        assert_eq!(coordinator.repository.persists, persists);
    }

    #[test]
    fn different_binding_orphans_old_entry_before_new_append() {
        let old_binding = binding(23, 15);
        let new_binding = binding(23, 16);
        let old_bundle = bundle(16);
        let new_bundle = bundle(17);
        let mut coordinator = coordinator_with_pending(old_binding, &old_bundle);
        let orphan = coordinator
            .observe(
                floor(2),
                DiagnosticCollectorObservation::Sealed(snapshot(new_binding, &new_bundle)),
            )
            .expect("orphan old");
        assert!(matches!(
            orphan,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 3 }
        ));
        assert_eq!(
            coordinator.repository.state(old_binding),
            Some(DiagnosticJournalEntryState::Orphaned)
        );
        let append = coordinator
            .observe(
                floor(3),
                DiagnosticCollectorObservation::Sealed(snapshot(new_binding, &new_bundle)),
            )
            .expect("append new");
        assert!(matches!(
            append,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 4 }
        ));
        assert_eq!(
            coordinator.into_repository().state(new_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[test]
    fn write_failure_before_first_copy_is_retryable_only_by_new_call() {
        let selected_binding = binding(24, 17);
        let selected_bundle = bundle(18);
        let mut repository = FakeRepository::initialized();
        repository.fault = PersistFault::BeforeWrite;
        let mut coordinator = DiagnosticCoordinator::new(repository);
        let error = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect_err("injected failure");
        assert!(matches!(error, DiagnosticCoordinatorError::Repository(_)));
        assert_eq!(coordinator.repository.generation(), 1);
        let retry = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect("explicit retry");
        assert!(matches!(
            retry,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 2 }
        ));
    }

    #[test]
    fn failure_after_first_copy_requires_explicit_repair() {
        let selected_binding = binding(25, 18);
        let selected_bundle = bundle(19);
        let mut repository = FakeRepository::initialized();
        repository.fault = PersistFault::AfterFirst;
        let mut coordinator = DiagnosticCoordinator::new(repository);
        assert!(matches!(
            coordinator.observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            ),
            Err(DiagnosticCoordinatorError::Repository(_))
        ));
        assert!(matches!(
            coordinator.observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            ),
            Err(DiagnosticCoordinatorError::StoreDegraded { generation: 2 })
        ));
    }

    #[test]
    fn failure_after_both_copies_is_discovered_without_duplicate_write() {
        let selected_binding = binding(26, 19);
        let selected_bundle = bundle(20);
        let mut repository = FakeRepository::initialized();
        repository.fault = PersistFault::AfterBoth;
        let mut coordinator = DiagnosticCoordinator::new(repository);
        assert!(matches!(
            coordinator.observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            ),
            Err(DiagnosticCoordinatorError::Repository(_))
        ));
        let persists = coordinator.repository.persists;
        let result = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect("discover committed generation");
        assert!(matches!(
            result,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 2 }
        ));
        assert_eq!(coordinator.repository.persists, persists);
    }

    #[test]
    fn failed_ack_persistence_arms_recovery_and_terminal_write_is_discovered() {
        let selected_binding = binding(27, 20);
        let selected_bundle = bundle(21);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        coordinator.repository.fault = PersistFault::AfterBoth;
        let error = prepare_permit(
            &mut coordinator,
            floor(2),
            selected_binding,
            &selected_bundle,
        )
        .complete(DiagnosticAcknowledgementCompletion::Applied)
        .expect_err("injected post-write error");
        assert!(matches!(error, DiagnosticCoordinatorError::Repository(_)));
        assert!(coordinator.has_cancelled_acknowledgement());
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(2))
                .expect("floor first"),
            DiagnosticRecoveryOutcome::AdvanceFloor { generation: 3 }
        );
        assert!(coordinator.has_cancelled_acknowledgement());
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(3))
                .expect("discover terminal state"),
            DiagnosticRecoveryOutcome::Terminal {
                generation: 3,
                state: DiagnosticJournalEntryState::Acknowledged,
            }
        );
        assert!(!coordinator.has_cancelled_acknowledgement());
    }

    #[test]
    fn failed_ack_after_first_copy_retains_recovery_marker_until_repair() {
        let selected_binding = binding(127, 20);
        let selected_bundle = bundle(21);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        coordinator.repository.fault = PersistFault::AfterFirst;
        assert!(
            prepare_permit(
                &mut coordinator,
                floor(2),
                selected_binding,
                &selected_bundle,
            )
            .complete(DiagnosticAcknowledgementCompletion::Applied)
            .is_err()
        );
        assert!(coordinator.has_cancelled_acknowledgement());
        assert!(matches!(
            coordinator.recover_cancelled_acknowledgement(floor(2)),
            Err(DiagnosticCoordinatorError::StoreDegraded { generation: 3 })
        ));
        assert!(coordinator.has_cancelled_acknowledgement());
    }

    #[test]
    fn failed_ack_before_write_is_recovered_as_ambiguous() {
        let selected_binding = binding(28, 21);
        let selected_bundle = bundle(22);
        let mut coordinator = coordinator_with_pending(selected_binding, &selected_bundle);
        coordinator.repository.fault = PersistFault::BeforeWrite;
        assert!(
            prepare_permit(
                &mut coordinator,
                floor(2),
                selected_binding,
                &selected_bundle,
            )
            .complete(DiagnosticAcknowledgementCompletion::Applied)
            .is_err()
        );
        assert!(coordinator.has_cancelled_acknowledgement());
        assert_eq!(
            coordinator
                .recover_cancelled_acknowledgement(floor(2))
                .expect("record conservative ambiguity"),
            DiagnosticRecoveryOutcome::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );
    }

    #[test]
    fn repository_error_debug_and_display_are_redacted() {
        let error: DiagnosticCoordinatorError<FakeError> =
            DiagnosticCoordinatorError::Repository(FakeError::Injected("private-diagnostic-root"));
        assert_eq!(format!("{error:?}"), "Repository");
        assert_eq!(format!("{error}"), "diagnostic repository operation failed");
        assert!(error.repository_error().is_some());
    }

    #[test]
    fn bounded_repository_operations_have_no_internal_retry() {
        let selected_binding = binding(29, 22);
        let selected_bundle = bundle(23);
        let mut coordinator = DiagnosticCoordinator::new(FakeRepository::initialized());
        let result = coordinator
            .observe(
                FLOOR_ONE,
                DiagnosticCollectorObservation::Sealed(snapshot(
                    selected_binding,
                    &selected_bundle,
                )),
            )
            .expect("persist once");
        assert!(matches!(
            result,
            DiagnosticObservationOutcome::AdvanceFloor { generation: 2 }
        ));
        assert_eq!(
            (
                coordinator.repository.loads,
                coordinator.repository.persists,
            ),
            (2, 1)
        );
    }
}
