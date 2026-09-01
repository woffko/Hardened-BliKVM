#![forbid(unsafe_code)]

//! Default-off composition of the fixed diagnostic store, loaded typed keys,
//! bounded export client, and accepted write-before-ack coordinator.
//!
//! The public constructor accepts only already validated fixed-boundary types.
//! This crate has no path, credential name, key bytes, floor source, retry
//! policy, task spawn, timer, logging, HTTP, UI, unit, or package surface.

use std::fmt;

use bli_kvm_diagnostic_coordinator::{
    DiagnosticAcknowledgementCompletion, DiagnosticAcknowledgementOutcome,
    DiagnosticCollectorObservation, DiagnosticCoordinator, DiagnosticCoordinatorError,
    DiagnosticGenerationFloor, DiagnosticJournalRepository, DiagnosticObservationOutcome,
    DiagnosticRecoveryOutcome,
};
use bli_kvm_diagnostic_export::DiagnosticBundle;
use bli_kvm_diagnostic_export_client::{
    DiagnosticExportAcknowledgementError, DiagnosticExportAcknowledgementOutcome,
    DiagnosticExportBinding, DiagnosticExportClient, DiagnosticExportReadError,
    DiagnosticExportReadOutcome,
};
use bli_kvm_diagnostic_journal::{
    DiagnosticBinding, DiagnosticJournalEntryState, DiagnosticJournalError, DiagnosticSnapshot,
    JournalMutation, SelectedDiagnosticJournal,
};
use bli_kvm_diagnostic_key_loader::LoadedDiagnosticJournalKeys;
use bli_kvm_diagnostic_store::{DiagnosticJournalStore, DiagnosticStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPersistenceStep {
    Idle {
        generation: u64,
    },
    CollectorUnavailable {
        generation: u64,
    },
    ReadNotSent,
    ReadRetryable,
    AdvanceFloor {
        generation: u64,
    },
    AcknowledgementNotSent {
        generation: u64,
    },
    ReadCollector {
        generation: u64,
    },
    RecoveryTerminal {
        generation: u64,
        state: DiagnosticJournalEntryState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticPersistenceError {
    Store(DiagnosticStoreError),
    Journal(DiagnosticJournalError),
    StoreDegraded {
        generation: u64,
    },
    RollbackFloorViolation {
        selected_generation: u64,
        minimum_generation: u64,
    },
    ConcurrentFloorAdvance {
        generation: u64,
    },
    PersistenceVerificationFailed,
    AcknowledgementRecoveryRequired,
    AcknowledgementNotReady,
    RecoveryConflict,
    InvalidCollectorSnapshot,
    InvariantViolation,
}

impl fmt::Display for DiagnosticPersistenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Store(_) => "diagnostic persistence store failed",
            Self::Journal(_) => "diagnostic persistence journal transition failed",
            Self::StoreDegraded { .. } => "diagnostic persistence requires explicit repair",
            Self::RollbackFloorViolation { .. } => {
                "diagnostic persistence rollback floor was violated"
            }
            Self::ConcurrentFloorAdvance { .. } => {
                "diagnostic persistence floor changed during acknowledgement"
            }
            Self::PersistenceVerificationFailed => "diagnostic persistence verification failed",
            Self::AcknowledgementRecoveryRequired => {
                "diagnostic acknowledgement recovery is required"
            }
            Self::AcknowledgementNotReady => "diagnostic acknowledgement is not ready",
            Self::RecoveryConflict => "diagnostic acknowledgement recovery conflicted",
            Self::InvalidCollectorSnapshot => "diagnostic collector snapshot is invalid",
            Self::InvariantViolation => "diagnostic persistence invariant failed",
        })
    }
}

impl std::error::Error for DiagnosticPersistenceError {}

/// Exact default-off runtime. It intentionally implements neither `Clone` nor
/// `Debug`.
pub struct DiagnosticPersistenceRuntime {
    core: RuntimeCore<FixedDiagnosticRepository, DiagnosticExportClient>,
}

impl DiagnosticPersistenceRuntime {
    /// Compose only already validated fixed-boundary objects.
    ///
    /// Callers can obtain `LoadedDiagnosticJournalKeys` only through the fixed
    /// key loader and `DiagnosticJournalStore` only through the fixed store
    /// API. This constructor accepts no path, raw credential, key, socket,
    /// generation, or retry configuration.
    pub fn new(keys: LoadedDiagnosticJournalKeys, store: DiagnosticJournalStore) -> Self {
        Self {
            core: RuntimeCore::new(
                FixedDiagnosticRepository { keys, store },
                DiagnosticExportClient::new(),
            ),
        }
    }

    /// Perform one bounded explicit coordination step.
    ///
    /// This method never loops or retries. Cancellation during its read is
    /// non-mutating. Cancellation while the acknowledgement future is pending
    /// drops the linear permit and forces durable ambiguity recovery on the
    /// next call.
    pub async fn poll(
        &mut self,
        floor: DiagnosticGenerationFloor,
    ) -> Result<DiagnosticPersistenceStep, DiagnosticPersistenceError> {
        self.core.poll(floor).await.map_err(map_public_error)
    }
}

struct FixedDiagnosticRepository {
    keys: LoadedDiagnosticJournalKeys,
    store: DiagnosticJournalStore,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FixedRepositoryError(DiagnosticStoreError);

impl DiagnosticJournalRepository for FixedDiagnosticRepository {
    type Error = FixedRepositoryError;

    fn load(&mut self, minimum_generation: u64) -> Result<SelectedDiagnosticJournal, Self::Error> {
        self.store
            .load_read_only(&self.keys.key_ring(), minimum_generation)
            .map_err(FixedRepositoryError)
    }

    fn persist(
        &mut self,
        minimum_generation: u64,
        mutation: &JournalMutation,
    ) -> Result<(), Self::Error> {
        let persisted = self
            .store
            .update(&self.keys.key_ring(), minimum_generation, mutation)
            .map_err(FixedRepositoryError)?;
        if persisted.generation != mutation.generation() {
            return Err(FixedRepositoryError(
                DiagnosticStoreError::VerificationFailed,
            ));
        }
        Ok(())
    }
}

struct RuntimeCore<R, T> {
    coordinator: DiagnosticCoordinator<R>,
    transport: T,
}

impl<R: DiagnosticJournalRepository, T: CollectorTransport> RuntimeCore<R, T> {
    const fn new(repository: R, transport: T) -> Self {
        Self {
            coordinator: DiagnosticCoordinator::new(repository),
            transport,
        }
    }

    async fn poll(
        &mut self,
        floor: DiagnosticGenerationFloor,
    ) -> Result<DiagnosticPersistenceStep, CoreError<R::Error>> {
        if self.coordinator.has_cancelled_acknowledgement() {
            let recovery = self
                .coordinator
                .recover_cancelled_acknowledgement(floor)
                .map_err(CoreError::Coordinator)?;
            return map_recovery(recovery);
        }

        let outcome = match self.transport.read_once().await {
            TransportRead::NoSealed => self
                .coordinator
                .observe(floor, DiagnosticCollectorObservation::NoSealed)
                .map_err(CoreError::Coordinator)?,
            TransportRead::Sealed(snapshot) => {
                let snapshot = DiagnosticSnapshot::new(
                    snapshot.binding,
                    snapshot.omitted_lifecycle_events,
                    &snapshot.bundle,
                )
                .map_err(|_| CoreError::InvalidCollectorSnapshot)?;
                self.coordinator
                    .observe(floor, DiagnosticCollectorObservation::Sealed(snapshot))
                    .map_err(CoreError::Coordinator)?
            }
            TransportRead::CollectorUnavailable => self
                .coordinator
                .observe(floor, DiagnosticCollectorObservation::Unavailable)
                .map_err(CoreError::Coordinator)?,
            TransportRead::NotSent => return Ok(DiagnosticPersistenceStep::ReadNotSent),
            TransportRead::Retryable => return Ok(DiagnosticPersistenceStep::ReadRetryable),
        };
        match outcome {
            DiagnosticObservationOutcome::Idle { generation } => {
                Ok(DiagnosticPersistenceStep::Idle { generation })
            }
            DiagnosticObservationOutcome::CollectorUnavailable { generation } => {
                Ok(DiagnosticPersistenceStep::CollectorUnavailable { generation })
            }
            DiagnosticObservationOutcome::ReadCollector { generation } => {
                Ok(DiagnosticPersistenceStep::ReadCollector { generation })
            }
            DiagnosticObservationOutcome::AdvanceFloor { generation } => {
                Ok(DiagnosticPersistenceStep::AdvanceFloor { generation })
            }
            DiagnosticObservationOutcome::AcknowledgementReady { .. } => {
                let (coordinator, transport) = (&mut self.coordinator, &self.transport);
                let permit = coordinator
                    .prepare_acknowledgement(floor)
                    .map_err(CoreError::Coordinator)?;
                let binding = permit.binding();
                let completion = transport.acknowledge_once(binding).await;
                let outcome = permit
                    .complete(completion)
                    .map_err(CoreError::Coordinator)?;
                Ok(map_acknowledgement(outcome))
            }
        }
    }
}

fn map_recovery<E>(
    recovery: DiagnosticRecoveryOutcome,
) -> Result<DiagnosticPersistenceStep, CoreError<E>> {
    match recovery {
        DiagnosticRecoveryOutcome::NoCancellation => Err(CoreError::InvariantViolation),
        DiagnosticRecoveryOutcome::AdvanceFloor { generation } => {
            Ok(DiagnosticPersistenceStep::AdvanceFloor { generation })
        }
        DiagnosticRecoveryOutcome::ReadCollector { generation } => {
            Ok(DiagnosticPersistenceStep::ReadCollector { generation })
        }
        DiagnosticRecoveryOutcome::Terminal { generation, state } => {
            Ok(DiagnosticPersistenceStep::RecoveryTerminal { generation, state })
        }
    }
}

const fn map_acknowledgement(
    outcome: DiagnosticAcknowledgementOutcome,
) -> DiagnosticPersistenceStep {
    match outcome {
        DiagnosticAcknowledgementOutcome::Pending { generation } => {
            DiagnosticPersistenceStep::AcknowledgementNotSent { generation }
        }
        DiagnosticAcknowledgementOutcome::ReadCollector { generation } => {
            DiagnosticPersistenceStep::ReadCollector { generation }
        }
        DiagnosticAcknowledgementOutcome::AdvanceFloor { generation } => {
            DiagnosticPersistenceStep::AdvanceFloor { generation }
        }
    }
}

#[derive(Debug)]
enum CoreError<E> {
    Coordinator(DiagnosticCoordinatorError<E>),
    InvalidCollectorSnapshot,
    InvariantViolation,
}

fn map_public_error(error: CoreError<FixedRepositoryError>) -> DiagnosticPersistenceError {
    match error {
        CoreError::InvalidCollectorSnapshot => DiagnosticPersistenceError::InvalidCollectorSnapshot,
        CoreError::InvariantViolation => DiagnosticPersistenceError::InvariantViolation,
        CoreError::Coordinator(error) => match error {
            DiagnosticCoordinatorError::Repository(FixedRepositoryError(error)) => {
                DiagnosticPersistenceError::Store(error)
            }
            DiagnosticCoordinatorError::Journal(error) => {
                DiagnosticPersistenceError::Journal(error)
            }
            DiagnosticCoordinatorError::StoreDegraded { generation } => {
                DiagnosticPersistenceError::StoreDegraded { generation }
            }
            DiagnosticCoordinatorError::RollbackFloorViolation {
                selected_generation,
                minimum_generation,
            } => DiagnosticPersistenceError::RollbackFloorViolation {
                selected_generation,
                minimum_generation,
            },
            DiagnosticCoordinatorError::FloorAdvanceRequired { generation } => {
                DiagnosticPersistenceError::ConcurrentFloorAdvance { generation }
            }
            DiagnosticCoordinatorError::PersistenceVerificationFailed => {
                DiagnosticPersistenceError::PersistenceVerificationFailed
            }
            DiagnosticCoordinatorError::AcknowledgementRecoveryRequired => {
                DiagnosticPersistenceError::AcknowledgementRecoveryRequired
            }
            DiagnosticCoordinatorError::AcknowledgementNotReady => {
                DiagnosticPersistenceError::AcknowledgementNotReady
            }
            DiagnosticCoordinatorError::RecoveryConflict => {
                DiagnosticPersistenceError::RecoveryConflict
            }
            DiagnosticCoordinatorError::InvariantViolation => {
                DiagnosticPersistenceError::InvariantViolation
            }
        },
    }
}

struct TransportSnapshot {
    binding: DiagnosticBinding,
    omitted_lifecycle_events: u8,
    bundle: DiagnosticBundle,
}

// Keep the already bounded 8 KiB typed snapshot inline so the local trust
// boundary cannot fail solely because a heap allocation was unavailable.
#[allow(clippy::large_enum_variant)]
enum TransportRead {
    NoSealed,
    Sealed(TransportSnapshot),
    CollectorUnavailable,
    NotSent,
    Retryable,
}

trait CollectorTransport {
    async fn read_once(&self) -> TransportRead;

    async fn acknowledge_once(
        &self,
        binding: DiagnosticBinding,
    ) -> DiagnosticAcknowledgementCompletion;
}

impl CollectorTransport for DiagnosticExportClient {
    async fn read_once(&self) -> TransportRead {
        map_export_read(DiagnosticExportClient::read_sealed(self).await)
    }

    async fn acknowledge_once(
        &self,
        binding: DiagnosticBinding,
    ) -> DiagnosticAcknowledgementCompletion {
        let Some(binding) =
            DiagnosticExportBinding::new(binding.instance_id(), binding.window_id())
        else {
            return DiagnosticAcknowledgementCompletion::NotSent;
        };
        map_export_acknowledgement(DiagnosticExportClient::acknowledge(self, binding).await)
    }
}

fn map_export_read(
    result: Result<DiagnosticExportReadOutcome, DiagnosticExportReadError>,
) -> TransportRead {
    match result {
        Ok(DiagnosticExportReadOutcome::NoSealed) => TransportRead::NoSealed,
        Ok(DiagnosticExportReadOutcome::CollectorUnavailable) => {
            TransportRead::CollectorUnavailable
        }
        Ok(DiagnosticExportReadOutcome::Sealed(snapshot)) => {
            let Some(binding) = DiagnosticBinding::new(
                snapshot.binding().instance_id(),
                snapshot.binding().window_id(),
            ) else {
                return TransportRead::Retryable;
            };
            TransportRead::Sealed(TransportSnapshot {
                binding,
                omitted_lifecycle_events: snapshot.omitted_lifecycle_events(),
                bundle: snapshot.bundle().clone(),
            })
        }
        Err(DiagnosticExportReadError::NotSent { .. }) => TransportRead::NotSent,
        Err(DiagnosticExportReadError::Retryable { .. }) => TransportRead::Retryable,
    }
}

const fn map_export_acknowledgement(
    result: Result<DiagnosticExportAcknowledgementOutcome, DiagnosticExportAcknowledgementError>,
) -> DiagnosticAcknowledgementCompletion {
    match result {
        Ok(DiagnosticExportAcknowledgementOutcome::Acknowledged) => {
            DiagnosticAcknowledgementCompletion::Applied
        }
        Ok(DiagnosticExportAcknowledgementOutcome::NoSealed) => {
            DiagnosticAcknowledgementCompletion::NoSealed
        }
        Ok(DiagnosticExportAcknowledgementOutcome::Mismatch) => {
            DiagnosticAcknowledgementCompletion::Mismatch
        }
        Ok(DiagnosticExportAcknowledgementOutcome::CollectorUnavailable) => {
            DiagnosticAcknowledgementCompletion::CollectorUnavailable
        }
        Err(DiagnosticExportAcknowledgementError::NotSent { .. }) => {
            DiagnosticAcknowledgementCompletion::NotSent
        }
        Err(DiagnosticExportAcknowledgementError::DeliveryAmbiguous { .. }) => {
            DiagnosticAcknowledgementCompletion::DeliveryAmbiguous
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bli_kvm_diagnostic_export::{
        DiagnosticComponent, DiagnosticEvent, DiagnosticExporter, DiagnosticRecord,
    };
    use bli_kvm_diagnostic_export_client::{
        DiagnosticExportExchangeStage, DiagnosticExportNotSentReason,
    };
    use bli_kvm_diagnostic_journal::{
        CopyIndex, DiagnosticJournal, DiagnosticJournalKey, DiagnosticJournalKeyRing,
        DiagnosticJournalNonce, RawDiagnosticJournalCopy, plan_initialize, plan_update,
        select_copies,
    };
    use std::{
        boxed::Box,
        cell::{Cell, RefCell},
        collections::VecDeque,
        future,
        rc::Rc,
        time::Duration,
    };

    const FLOOR_ONE: DiagnosticGenerationFloor = match DiagnosticGenerationFloor::new(1) {
        Some(floor) => floor,
        None => panic!("test floor must be nonzero"),
    };

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeRepositoryError {
        Journal(DiagnosticJournalError),
        Injected,
    }

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    enum PersistFault {
        #[default]
        None,
        BeforeWrite,
        AfterFirst,
        AfterBoth,
    }

    struct FakeRepository {
        key: DiagnosticJournalKey,
        copies: [Option<Box<RawDiagnosticJournalCopy>>; 2],
        nonce_counter: u64,
        control: Rc<FakeRepositoryControl>,
    }

    #[derive(Default)]
    struct FakeRepositoryControl {
        fault: Cell<PersistFault>,
        loads: Cell<usize>,
        persists: Cell<usize>,
    }

    impl FakeRepository {
        fn initialized() -> Self {
            Self::initialized_with_control().0
        }

        fn initialized_with_control() -> (Self, Rc<FakeRepositoryControl>) {
            let key = DiagnosticJournalKey::new(91, [0xa5; 32]).expect("test key");
            let ring = DiagnosticJournalKeyRing::new(&key, None).expect("test ring");
            let plan = plan_initialize(
                &DiagnosticJournal::empty(),
                &ring,
                DiagnosticJournalNonce::new(nonce_bytes(1)).expect("test nonce"),
            )
            .expect("initial plan");
            let bytes = *plan.bytes();
            let control = Rc::new(FakeRepositoryControl::default());
            (
                Self {
                    key,
                    copies: [Some(Box::new(bytes)), Some(Box::new(bytes))],
                    nonce_counter: 2,
                    control: Rc::clone(&control),
                },
                control,
            )
        }

        fn select(
            &self,
            minimum_generation: u64,
        ) -> Result<SelectedDiagnosticJournal, FakeRepositoryError> {
            let ring = DiagnosticJournalKeyRing::new(&self.key, None)
                .map_err(FakeRepositoryError::Journal)?;
            select_copies(
                self.copies[0].as_deref().map(|copy| copy.as_slice()),
                self.copies[1].as_deref().map(|copy| copy.as_slice()),
                &ring,
                minimum_generation,
            )
            .map_err(FakeRepositoryError::Journal)
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
        type Error = FakeRepositoryError;

        fn load(
            &mut self,
            minimum_generation: u64,
        ) -> Result<SelectedDiagnosticJournal, Self::Error> {
            self.control.loads.set(self.control.loads.get() + 1);
            self.select(minimum_generation)
        }

        fn persist(
            &mut self,
            minimum_generation: u64,
            mutation: &JournalMutation,
        ) -> Result<(), Self::Error> {
            self.control.persists.set(self.control.persists.get() + 1);
            let fault = self.control.fault.replace(PersistFault::None);
            if fault == PersistFault::BeforeWrite {
                return Err(FakeRepositoryError::Injected);
            }
            let selected = self.select(minimum_generation)?;
            let ring = DiagnosticJournalKeyRing::new(&self.key, None)
                .map_err(FakeRepositoryError::Journal)?;
            let nonce =
                DiagnosticJournalNonce::new(nonce_bytes(self.nonce_counter)).expect("test nonce");
            self.nonce_counter += 1;
            let plan = plan_update(&selected, mutation, &ring, nonce)
                .map_err(FakeRepositoryError::Journal)?;
            let bytes = *plan.bytes();
            self.replace(plan.first(), bytes);
            if fault == PersistFault::AfterFirst {
                return Err(FakeRepositoryError::Injected);
            }
            self.replace(plan.second(), bytes);
            if fault == PersistFault::AfterBoth {
                return Err(FakeRepositoryError::Injected);
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeTransport {
        reads: RefCell<VecDeque<TransportRead>>,
        acknowledgements: RefCell<VecDeque<DiagnosticAcknowledgementCompletion>>,
        read_calls: Cell<usize>,
        acknowledgement_calls: Cell<usize>,
        stall_read: Cell<bool>,
        stall_acknowledgement: Cell<bool>,
    }

    impl FakeTransport {
        fn push_read(&self, read: TransportRead) {
            self.reads.borrow_mut().push_back(read);
        }

        fn push_snapshot(&self, binding: DiagnosticBinding, bundle: &DiagnosticBundle) {
            self.push_read(TransportRead::Sealed(TransportSnapshot {
                binding,
                omitted_lifecycle_events: 0,
                bundle: bundle.clone(),
            }));
        }

        fn push_acknowledgement(&self, completion: DiagnosticAcknowledgementCompletion) {
            self.acknowledgements.borrow_mut().push_back(completion);
        }
    }

    impl CollectorTransport for FakeTransport {
        async fn read_once(&self) -> TransportRead {
            self.read_calls.set(self.read_calls.get() + 1);
            if self.stall_read.get() {
                future::pending::<()>().await;
                unreachable!("pending read cannot complete");
            }
            self.reads.borrow_mut().pop_front().expect("queued read")
        }

        async fn acknowledge_once(
            &self,
            _binding: DiagnosticBinding,
        ) -> DiagnosticAcknowledgementCompletion {
            self.acknowledgement_calls
                .set(self.acknowledgement_calls.get() + 1);
            if self.stall_acknowledgement.get() {
                future::pending::<()>().await;
                unreachable!("pending acknowledgement cannot complete");
            }
            self.acknowledgements
                .borrow_mut()
                .pop_front()
                .expect("queued acknowledgement")
        }
    }

    const fn copy_offset(index: CopyIndex) -> usize {
        match index {
            CopyIndex::A => 0,
            CopyIndex::B => 1,
        }
    }

    fn nonce_bytes(counter: u64) -> [u8; 24] {
        let mut bytes = [0_u8; 24];
        bytes[..8].copy_from_slice(&counter.to_le_bytes());
        bytes
    }

    fn floor(generation: u64) -> DiagnosticGenerationFloor {
        DiagnosticGenerationFloor::new(generation).expect("nonzero floor")
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
        exporter.export().expect("bundle")
    }

    async fn core_with_pending(
        selected_binding: DiagnosticBinding,
        selected_bundle: &DiagnosticBundle,
    ) -> (
        RuntimeCore<FakeRepository, FakeTransport>,
        Rc<FakeRepositoryControl>,
    ) {
        let transport = FakeTransport::default();
        transport.push_snapshot(selected_binding, selected_bundle);
        let (repository, control) = FakeRepository::initialized_with_control();
        let mut core = RuntimeCore::new(repository, transport);
        assert_eq!(
            core.poll(FLOOR_ONE).await.expect("persist pending"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 2 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 0);
        (core, control)
    }

    #[tokio::test]
    async fn new_snapshot_is_persisted_without_ack_until_floor_advances() {
        let selected_binding = binding(301, 1);
        let selected_bundle = bundle(1);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        assert_eq!(
            core.poll(FLOOR_ONE).await.expect("floor remains behind"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 2 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 0);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[tokio::test]
    async fn applied_acknowledgement_is_exactly_once_and_durable() {
        let selected_binding = binding(302, 2);
        let selected_bundle = bundle(2);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::Applied);
        assert_eq!(
            core.poll(floor(2)).await.expect("applied"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(core.transport.read_calls.get(), 2);
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::Acknowledged)
        );
    }

    #[tokio::test]
    async fn acknowledgement_not_sent_never_retries_and_consumes_readiness() {
        let selected_binding = binding(303, 3);
        let selected_bundle = bundle(3);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::NotSent);
        assert_eq!(
            core.poll(floor(2)).await.expect("not sent"),
            DiagnosticPersistenceStep::AcknowledgementNotSent { generation: 2 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        assert!(!core.coordinator.has_acknowledgement_ready());
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[tokio::test]
    async fn acknowledgement_no_sealed_is_durably_orphaned() {
        let selected_binding = binding(310, 10);
        let selected_bundle = bundle(10);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::NoSealed);
        assert_eq!(
            core.poll(floor(2)).await.expect("orphan acknowledgement"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::Orphaned)
        );
    }

    #[tokio::test]
    async fn mismatch_returns_read_without_hidden_retry_or_write() {
        let selected_binding = binding(304, 4);
        let selected_bundle = bundle(4);
        let (mut core, control) = core_with_pending(selected_binding, &selected_bundle).await;
        let persists = control.persists.get();
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::Mismatch);
        assert_eq!(
            core.poll(floor(2)).await.expect("mismatch"),
            DiagnosticPersistenceStep::ReadCollector { generation: 2 }
        );
        assert_eq!(core.transport.read_calls.get(), 2);
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        assert_eq!(control.persists.get(), persists);
    }

    #[tokio::test]
    async fn ambiguous_ack_is_persisted_before_same_snapshot_can_return_pending() {
        let selected_binding = binding(305, 5);
        let selected_bundle = bundle(5);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::DeliveryAmbiguous);
        assert_eq!(
            core.poll(floor(2)).await.expect("persist ambiguity"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        assert_eq!(
            core.poll(floor(3)).await.expect("return pending"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 4 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::PendingAcknowledgement)
        );
    }

    #[tokio::test]
    async fn ack_collector_unavailable_is_treated_as_delivery_ambiguous() {
        let selected_binding = binding(306, 6);
        let selected_bundle = bundle(6);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::CollectorUnavailable);
        assert_eq!(
            core.poll(floor(2)).await.expect("persist ambiguity"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );
    }

    #[tokio::test]
    async fn read_failures_are_returned_without_repository_or_ack_retry() {
        for (read, expected) in [
            (
                TransportRead::NotSent,
                DiagnosticPersistenceStep::ReadNotSent,
            ),
            (
                TransportRead::Retryable,
                DiagnosticPersistenceStep::ReadRetryable,
            ),
        ] {
            let transport = FakeTransport::default();
            transport.push_read(read);
            let (repository, control) = FakeRepository::initialized_with_control();
            let mut core = RuntimeCore::new(repository, transport);
            assert_eq!(core.poll(FLOOR_ONE).await.expect("read failure"), expected);
            assert_eq!(control.loads.get(), 0);
            assert_eq!(core.transport.read_calls.get(), 1);
            assert_eq!(core.transport.acknowledgement_calls.get(), 0);
        }
    }

    #[tokio::test]
    async fn read_cancellation_is_nonmutating_and_needs_an_explicit_new_poll() {
        let transport = FakeTransport::default();
        transport.stall_read.set(true);
        let (repository, control) = FakeRepository::initialized_with_control();
        let mut core = RuntimeCore::new(repository, transport);
        assert!(
            tokio::time::timeout(Duration::from_millis(10), core.poll(FLOOR_ONE))
                .await
                .is_err()
        );
        assert!(!core.coordinator.has_cancelled_acknowledgement());
        assert_eq!(control.loads.get(), 0);
        assert_eq!(core.transport.read_calls.get(), 1);

        core.transport.stall_read.set(false);
        core.transport.push_read(TransportRead::NoSealed);
        assert_eq!(
            core.poll(FLOOR_ONE).await.expect("explicit new poll"),
            DiagnosticPersistenceStep::Idle { generation: 1 }
        );
        assert_eq!(core.transport.read_calls.get(), 2);
    }

    #[tokio::test]
    async fn collector_unavailable_is_checked_against_store_without_ack() {
        let transport = FakeTransport::default();
        transport.push_read(TransportRead::CollectorUnavailable);
        let (repository, control) = FakeRepository::initialized_with_control();
        let mut core = RuntimeCore::new(repository, transport);
        assert_eq!(
            core.poll(FLOOR_ONE).await.expect("unavailable"),
            DiagnosticPersistenceStep::CollectorUnavailable { generation: 1 }
        );
        assert_eq!(control.loads.get(), 1);
        assert_eq!(core.transport.acknowledgement_calls.get(), 0);
    }

    #[tokio::test]
    async fn no_sealed_orphans_pending_without_acknowledgement() {
        let selected_binding = binding(307, 7);
        let selected_bundle = bundle(7);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport.push_read(TransportRead::NoSealed);
        assert_eq!(
            core.poll(floor(2)).await.expect("orphan"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(core.transport.acknowledgement_calls.get(), 0);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::Orphaned)
        );
    }

    #[tokio::test]
    async fn cancellation_during_ack_blocks_read_until_ambiguity_is_durable() {
        let selected_binding = binding(308, 8);
        let selected_bundle = bundle(8);
        let (mut core, _control) = core_with_pending(selected_binding, &selected_bundle).await;
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport.stall_acknowledgement.set(true);
        let reads_before_cancel = core.transport.read_calls.get();
        assert!(
            tokio::time::timeout(Duration::from_millis(10), core.poll(floor(2)))
                .await
                .is_err()
        );
        assert!(core.coordinator.has_cancelled_acknowledgement());
        assert_eq!(core.transport.acknowledgement_calls.get(), 1);
        core.transport.stall_acknowledgement.set(false);
        assert_eq!(
            core.poll(floor(2)).await.expect("recover cancellation"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(core.transport.read_calls.get(), reads_before_cancel + 1);
        assert_eq!(
            core.coordinator.into_repository().state(selected_binding),
            Some(DiagnosticJournalEntryState::AcknowledgementAmbiguous)
        );
    }

    #[tokio::test]
    async fn ack_persistence_failure_arms_recovery_before_next_read() {
        let selected_binding = binding(309, 9);
        let selected_bundle = bundle(9);
        let (mut core, control) = core_with_pending(selected_binding, &selected_bundle).await;
        control.fault.set(PersistFault::BeforeWrite);
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::Applied);
        assert!(matches!(
            core.poll(floor(2)).await,
            Err(CoreError::Coordinator(
                DiagnosticCoordinatorError::Repository(FakeRepositoryError::Injected)
            ))
        ));
        assert!(core.coordinator.has_cancelled_acknowledgement());
        let reads = core.transport.read_calls.get();
        assert_eq!(
            core.poll(floor(2)).await.expect("persist ambiguity"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert_eq!(core.transport.read_calls.get(), reads);
    }

    #[tokio::test]
    async fn ack_failure_after_first_copy_requires_repair_and_keeps_marker() {
        let selected_binding = binding(311, 11);
        let selected_bundle = bundle(11);
        let (mut core, control) = core_with_pending(selected_binding, &selected_bundle).await;
        control.fault.set(PersistFault::AfterFirst);
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::Applied);
        assert!(matches!(
            core.poll(floor(2)).await,
            Err(CoreError::Coordinator(
                DiagnosticCoordinatorError::Repository(FakeRepositoryError::Injected)
            ))
        ));
        assert!(core.coordinator.has_cancelled_acknowledgement());
        let reads = core.transport.read_calls.get();
        assert!(matches!(
            core.poll(floor(2)).await,
            Err(CoreError::Coordinator(
                DiagnosticCoordinatorError::StoreDegraded { generation: 3 }
            ))
        ));
        assert!(core.coordinator.has_cancelled_acknowledgement());
        assert_eq!(core.transport.read_calls.get(), reads);
    }

    #[tokio::test]
    async fn ack_failure_after_both_copies_discovers_terminal_without_read() {
        let selected_binding = binding(312, 12);
        let selected_bundle = bundle(12);
        let (mut core, control) = core_with_pending(selected_binding, &selected_bundle).await;
        control.fault.set(PersistFault::AfterBoth);
        core.transport
            .push_snapshot(selected_binding, &selected_bundle);
        core.transport
            .push_acknowledgement(DiagnosticAcknowledgementCompletion::Applied);
        assert!(matches!(
            core.poll(floor(2)).await,
            Err(CoreError::Coordinator(
                DiagnosticCoordinatorError::Repository(FakeRepositoryError::Injected)
            ))
        ));
        let reads = core.transport.read_calls.get();
        assert_eq!(
            core.poll(floor(2)).await.expect("advance floor"),
            DiagnosticPersistenceStep::AdvanceFloor { generation: 3 }
        );
        assert!(core.coordinator.has_cancelled_acknowledgement());
        assert_eq!(
            core.poll(floor(3)).await.expect("discover terminal"),
            DiagnosticPersistenceStep::RecoveryTerminal {
                generation: 3,
                state: DiagnosticJournalEntryState::Acknowledged,
            }
        );
        assert!(!core.coordinator.has_cancelled_acknowledgement());
        assert_eq!(core.transport.read_calls.get(), reads);
    }

    #[test]
    fn export_client_outcomes_map_exhaustively_without_retry() {
        assert!(matches!(
            map_export_read(Ok(DiagnosticExportReadOutcome::NoSealed)),
            TransportRead::NoSealed
        ));
        assert!(matches!(
            map_export_read(Ok(DiagnosticExportReadOutcome::CollectorUnavailable)),
            TransportRead::CollectorUnavailable
        ));
        assert!(matches!(
            map_export_read(Err(DiagnosticExportReadError::NotSent {
                reason: DiagnosticExportNotSentReason::ConnectFailed,
            })),
            TransportRead::NotSent
        ));
        assert!(matches!(
            map_export_read(Err(DiagnosticExportReadError::Retryable {
                stage: DiagnosticExportExchangeStage::ResponseTimeout,
            })),
            TransportRead::Retryable
        ));

        let ack_cases = [
            (
                Ok(DiagnosticExportAcknowledgementOutcome::Acknowledged),
                DiagnosticAcknowledgementCompletion::Applied,
            ),
            (
                Ok(DiagnosticExportAcknowledgementOutcome::NoSealed),
                DiagnosticAcknowledgementCompletion::NoSealed,
            ),
            (
                Ok(DiagnosticExportAcknowledgementOutcome::Mismatch),
                DiagnosticAcknowledgementCompletion::Mismatch,
            ),
            (
                Ok(DiagnosticExportAcknowledgementOutcome::CollectorUnavailable),
                DiagnosticAcknowledgementCompletion::CollectorUnavailable,
            ),
            (
                Err(DiagnosticExportAcknowledgementError::NotSent {
                    reason: DiagnosticExportNotSentReason::ConnectTimeout,
                }),
                DiagnosticAcknowledgementCompletion::NotSent,
            ),
            (
                Err(DiagnosticExportAcknowledgementError::DeliveryAmbiguous {
                    stage: DiagnosticExportExchangeStage::ResponseClosed,
                }),
                DiagnosticAcknowledgementCompletion::DeliveryAmbiguous,
            ),
        ];
        for (input, expected) in ack_cases {
            assert_eq!(map_export_acknowledgement(input), expected);
        }
    }

    #[test]
    fn public_errors_are_bounded_and_do_not_include_paths_or_payloads() {
        assert_eq!(
            DiagnosticPersistenceError::Store(DiagnosticStoreError::Io).to_string(),
            "diagnostic persistence store failed"
        );
        assert_eq!(
            DiagnosticPersistenceError::InvalidCollectorSnapshot.to_string(),
            "diagnostic collector snapshot is invalid"
        );
    }

    #[test]
    fn fake_fault_modes_cover_every_durable_completion_shape() {
        assert_ne!(PersistFault::AfterFirst, PersistFault::AfterBoth);
        let repository = FakeRepository::initialized();
        assert_eq!(repository.generation(), 1);
    }

    #[test]
    fn poll_future_size_is_bounded_for_fake_and_exact_transport() {
        let transport = FakeTransport::default();
        transport.push_read(TransportRead::NoSealed);
        let mut core = RuntimeCore::new(FakeRepository::initialized(), transport);
        let future = core.poll(FLOOR_ONE);
        assert!(std::mem::size_of_val(&future) <= 16 * 1024);
        drop(future);

        let mut exact_transport_core =
            RuntimeCore::new(FakeRepository::initialized(), DiagnosticExportClient::new());
        let exact_future = exact_transport_core.poll(FLOOR_ONE);
        assert!(std::mem::size_of_val(&exact_future) <= 32 * 1024);
    }
}
