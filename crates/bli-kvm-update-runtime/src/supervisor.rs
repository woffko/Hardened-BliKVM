use std::{io::Read, path::PathBuf};

use bli_kvm_update_manager::{
    ActivationOutcome, ActivationRequest, ApplicationUpdateManager, ConfirmationOutcome,
    ManagerStatus, RecoveryOutcome, UpdateManagerError,
};
use zeroize::Zeroize;

fn validate_quiescence_clock(
    before: bli_kvm_update_manager::ClockSnapshot,
    after: bli_kvm_update_manager::ClockSnapshot,
    deadline: u64,
) -> Result<(), UpdateManagerError> {
    if before.boot_id != after.boot_id
        || after.monotonic_ms < before.monotonic_ms
        || after.monotonic_ms > deadline
    {
        return Err(UpdateManagerError::RecoveryRequired);
    }
    Ok(())
}

use crate::{
    ConfirmedApplyRequest, HealthChallengeStatus, HealthChallengeStore, LinuxKernelEvidence,
    MAX_APPLY_REQUEST_BYTES, UpdateRuntimeError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeReconciliation {
    None,
    Ready,
    DiscardedStale,
    RolledBackMissing,
    RolledBackMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorActivation {
    pub outcome: ActivationOutcome,
    pub credential_path: Option<PathBuf>,
}

/// Durable intent receipt; the candidate has not been selected or started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorAdmission {
    pub operation_id: [u8; 16],
    pub target_slot_id: String,
    pub prepared_generation: u64,
    pub deadline_monotonic_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SupervisorRecovery {
    pub manager: RecoveryOutcome,
    pub challenge: ChallengeReconciliation,
    pub abandoned_staging_removed: usize,
    pub slot_deletions_completed: usize,
}

#[derive(Debug)]
pub struct UpdateSupervisor {
    manager: ApplicationUpdateManager,
    challenges: HealthChallengeStore,
    kernel: LinuxKernelEvidence,
}

/// A live pending operation is not evidence of current service readiness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NativeBootOutcome {
    Ready(crate::native_identity::NativeIdentityObservation),
    AwaitingHealth(bli_kvm_update_manager::PendingStatus),
}

impl UpdateSupervisor {
    /// Reconcile native boot/restart under the existing manager locks. A live
    /// pending operation keeps its original health window; this does not restart
    /// or confirm it. Callers must continue native health/watchdog supervision.
    /// Legacy journals require explicit provisioning, never implicit migration.
    pub fn boot_native_composition(
        &self,
        expected_generation: u64,
        controller: &mut impl crate::native_identity::NativeStartController,
        deadline: std::time::Instant,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<NativeBootOutcome, UpdateRuntimeError> {
        use crate::composition::CompositionError;
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline.into());
        }
        let before = self.manager.inspect_recovery()?;
        if before.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        // Schema 2 supports native recovery; schema 3 adds the idle lifecycle.
        if !matches!(before.schema_version, 2 | 3) {
            return Err(UpdateManagerError::RecoveryRequired.into());
        }
        if let Some(completion) = before.completion.as_ref() {
            return self
                .start_completed_composition(
                    expected_generation,
                    completion,
                    controller,
                    deadline,
                    probe,
                )
                .map(NativeBootOutcome::Ready);
        }
        if before.pending.is_some() || before.rollback_intent.is_some() {
            if let Some(intent) = self.begin_native_credential_recovery(expected_generation)? {
                let recovery = self.manager.inspect_recovery()?;
                return self
                    .restore_native_composition(
                        recovery.generation,
                        &intent,
                        controller,
                        deadline,
                        probe,
                    )
                    .map(|(_, ready)| NativeBootOutcome::Ready(ready));
            }
            if std::time::Instant::now() >= deadline {
                return Err(CompositionError::Deadline.into());
            }
            if self.manager.inspect_recovery()? != before {
                return Err(CompositionError::SelectionMismatch.into());
            }
            return before
                .pending
                .map(NativeBootOutcome::AwaitingHealth)
                .ok_or_else(|| UpdateManagerError::RecoveryRequired.into());
        }
        self.start_idle_composition(expected_generation, controller, deadline, probe)
            .map(NativeBootOutcome::Ready)
    }

    /// Record a recovery decision from exact pending credentials, without
    /// stopping services, selecting a slot, or deleting runtime evidence.
    pub fn begin_native_credential_recovery(
        &self,
        expected_generation: u64,
    ) -> Result<Option<bli_kvm_update_manager::NativeRollbackIntent>, UpdateRuntimeError> {
        // Check for competing legacy authority even when Prepared/timeout/boot
        // recovery takes precedence over credential inspection callbacks.
        if self.challenges.confirmed_apply_request()?.is_some() {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        let clock = self.kernel.snapshot()?;
        let mut inspection_error = None;
        let result = self.manager.begin_native_recovery_with_credentials(
            expected_generation,
            clock,
            |operation, binding, target| {
                let inspect = || -> Result<_, UpdateRuntimeError> {
                    if self.challenges.confirmed_apply_request()?.is_some() {
                        return Err(UpdateRuntimeError::SupervisorStateMismatch);
                    }
                    self.challenges.inspect_exact(
                        crate::challenge::decode_array(operation)?,
                        crate::challenge::decode_array(binding)?,
                        target,
                    )
                };
                match inspect() {
                    Ok(crate::ChallengePresence::Complete) => {
                        Ok(bli_kvm_update_manager::NativeCredentialAvailability::Intact)
                    }
                    Ok(_) => Ok(bli_kvm_update_manager::NativeCredentialAvailability::Lost),
                    Err(error) => {
                        inspection_error = Some(error);
                        Err(UpdateManagerError::RecoveryRequired)
                    }
                }
            },
        );
        if let Some(error) = inspection_error {
            return Err(error);
        }
        result.map_err(Into::into)
    }

    pub fn new(manager: ApplicationUpdateManager, challenges: HealthChallengeStore) -> Self {
        Self {
            manager,
            challenges,
            kernel: LinuxKernelEvidence::new(),
        }
    }

    pub fn initialize(&self, active_slot_id: &str) -> Result<ManagerStatus, UpdateRuntimeError> {
        let _ = self.challenges.clear_stale_after_rollback()?;
        Ok(self.manager.initialize(active_slot_id)?)
    }

    pub fn status(&self) -> Result<ManagerStatus, UpdateRuntimeError> {
        Ok(self.manager.status()?)
    }

    /// Read-only recovery inspection under the held manager locks.
    pub fn inspect_recovery(
        &self,
    ) -> Result<bli_kvm_update_manager::RecoveryInspection, UpdateRuntimeError> {
        Ok(self.manager.inspect_recovery()?)
    }

    pub fn native_status(&self) -> Result<ManagerStatus, UpdateRuntimeError> {
        Ok(self.manager.native_status()?)
    }

    /// Opt into native idle only with a complete verified confirmed composition.
    /// No service is started here. An interrupted exact cleanup replays against
    /// the still-current completion; unrelated credentials are never discarded.
    pub fn migrate_native_idle(&self, expected_generation: u64) -> Result<u64, UpdateRuntimeError> {
        let before = self.manager.inspect_recovery()?;
        if before.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        let _inventory = crate::native_identity::NativeSlotIdentity::open(
            &self.manager,
            &before.confirmed_slot_id,
        )?;
        // Also enforce emptiness on already-migrated idle replay, where the
        // manager intentionally does not invoke its cleanup callback.
        if before.completion.is_none() {
            self.challenges.require_empty_native_runtime()?;
        }
        let mut cleanup_error = None;
        let result = self
            .manager
            .migrate_native_idle(expected_generation, |completion| {
                let result = (|| {
                    if let Some(completion) = completion {
                        self.cleanup_native_completion(expected_generation, completion)?;
                    }
                    self.challenges.require_empty_native_runtime()
                })();
                result.map_err(|error| {
                    cleanup_error = Some(error);
                    UpdateManagerError::RecoveryRequired
                })
            });
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        Ok(result?)
    }

    fn request_confirmed_apply(&self) -> Result<(), UpdateRuntimeError> {
        let status = self.manager.status()?;
        if status.pending.is_some() || status.active_slot_id != status.confirmed_slot_id {
            return Err(UpdateRuntimeError::SupervisorStateMismatch);
        }
        self.challenges
            .request_confirmed_apply(status.generation, &status.confirmed_slot_id)
    }

    pub fn acknowledge_confirmed_apply(
        &self,
        reader: &mut impl Read,
    ) -> Result<bool, UpdateRuntimeError> {
        let mut bytes = Vec::new();
        reader
            .take((MAX_APPLY_REQUEST_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        let request = ConfirmedApplyRequest::from_bytes(&bytes)?;
        let status = self.manager.status()?;
        if status.pending.is_some()
            || status.generation != request.generation
            || status.active_slot_id != request.slot_id
            || status.confirmed_slot_id != request.slot_id
        {
            return Ok(false);
        }
        self.challenges.acknowledge_confirmed_apply(&request)
    }

    pub fn activate(
        &self,
        target_slot_id: &str,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        self.activate_inner(target_slot_id, None, false, |_| Ok(()), || Ok(()))
    }

    /// Network confirmation is bound to the exact observed journal generation.
    /// The manager's state/slot locks live for this entire supervisor lifetime.
    /// Repeating an old confirmation never creates another operation; callers
    /// must reconcile through status rather than silently refresh the fence.
    pub fn activate_if_generation(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        self.activate_if_generation_with_quiescence(target_slot_id, expected_generation, || Ok(()))
    }

    /// Native coordinators supply bounded quiescence while the manager holds
    /// the state/slot locks. Stale or rejected requests never invoke this hook.
    pub fn activate_if_generation_with_quiescence(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        self.activate_inner(
            target_slot_id,
            Some(expected_generation),
            false,
            |_| Ok(()),
            quiesce,
        )
    }

    /// Deliver and close a bounded admission response before stopping its server.
    /// Locks remain held through delivery and execution, excluding watchdog
    /// recovery of Prepared during handoff. The callback must not report success
    /// until response delivery and stream shutdown finish. Failure keeps recovery
    /// intent and never invokes quiescence. The receipt is not AwaitingHealth.
    pub fn activate_if_generation_with_admission(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
        deliver: impl FnOnce(&SupervisorAdmission) -> Result<(), UpdateManagerError>,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        self.activate_inner(
            target_slot_id,
            Some(expected_generation),
            true,
            deliver,
            quiesce,
        )
    }

    /// Drive admission, quiescence, selected startup and readiness under the same
    /// held manager locks and caller deadline. Delivery must publish EOF before
    /// returning. Controllers/probes must enforce that deadline or an earlier one.
    /// This does not confirm health or advance the rollback floor. On failure,
    /// durable intent/challenge remain for coordinated stop and recovery; callers
    /// must not assume that a failed dispatch left every service stopped.
    pub fn activate_native_candidate(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
        controller: &mut impl crate::native_identity::NativeStartController,
        deadline: std::time::Instant,
        deliver: impl FnOnce(&SupervisorAdmission) -> Result<(), UpdateManagerError>,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<
        (
            SupervisorActivation,
            crate::native_identity::NativeIdentityObservation,
        ),
        UpdateRuntimeError,
    > {
        use crate::{
            composition::{self, CompositionError},
            native_identity::{NativeSlotIdentity, NativeStartUnit, SlotServer},
        };
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        // Reject a known incompatible composition before issuing a challenge or
        // disrupting the existing services. The generation fence precedes even
        // these read-only controller calls; activation rechecks it under locks.
        self.check_native_activation_precondition(target_slot_id, expected_generation)?;
        for unit in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(SlotServer::Candidate),
        ] {
            if std::time::Instant::now() >= deadline {
                return Err(CompositionError::Deadline.into());
            }
            controller.validate_start(unit)?;
        }
        controller.prepare(target_slot_id)?;
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        let mut stop_error = None;
        let activation = self.activate_native_with_admission(
            target_slot_id,
            expected_generation,
            |admission| {
                let observed_at = std::time::Instant::now();
                let clock = self
                    .kernel
                    .snapshot()
                    .map_err(|_| UpdateManagerError::InvalidClock)?;
                let remaining = admission
                    .deadline_monotonic_ms
                    .checked_sub(clock.monotonic_ms)
                    .filter(|remaining| *remaining > 0)
                    .ok_or(UpdateManagerError::RecoveryRequired)?;
                let latest = observed_at
                    .checked_add(std::time::Duration::from_millis(remaining))
                    .ok_or(UpdateManagerError::InvalidClock)?;
                if deadline > latest || observed_at >= deadline {
                    return Err(UpdateManagerError::RecoveryRequired);
                }
                deliver(admission)
            },
            || {
                let result = if std::time::Instant::now() >= deadline {
                    Err(CompositionError::Deadline)
                } else {
                    composition::quiesce(controller)
                };
                result
                    .and_then(|()| {
                        if std::time::Instant::now() >= deadline {
                            Err(CompositionError::Deadline)
                        } else {
                            Ok(())
                        }
                    })
                    .map_err(|error| {
                        stop_error = Some(error);
                        UpdateManagerError::RecoveryRequired
                    })
            },
        );
        if let Some(error) = stop_error {
            return Err(error.into());
        }
        let activation = activation?;
        let context = NativeSlotIdentity::open(&self.manager, target_slot_id)?;
        let started = context.start_and_observe(controller, SlotServer::Candidate, deadline)?;
        let ready = context.check_readiness(controller, SlotServer::Candidate, deadline, probe)?;
        if started != ready {
            return Err(CompositionError::Controller.into());
        }
        Ok((activation, ready))
    }

    /// Confirm only the operation and live identities observed during startup.
    /// This trusted coordinator replaces the generic token-only health reporter.
    /// Failure retains native recovery responsibility and credentials, never legacy apply.
    pub fn confirm_native_candidate(
        &self,
        expected_generation: u64,
        operation_id: [u8; 16],
        started: &crate::native_identity::NativeIdentityObservation,
        controller: &mut impl crate::native_identity::NativeIdentityController,
        deadline: std::time::Instant,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<bli_kvm_update_manager::NativeConfirmationOutcome, UpdateRuntimeError> {
        use crate::{
            composition::CompositionError,
            native_identity::{NativeSlotIdentity, SlotServer},
        };
        let observed_at = std::time::Instant::now();
        let status = self.manager.native_status()?;
        if status.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        let pending = status
            .pending
            .as_ref()
            .ok_or(UpdateRuntimeError::SupervisorStateMismatch)?;
        if pending.target_slot_id != started.slot_id {
            return Err(UpdateRuntimeError::SupervisorStateMismatch);
        }
        if self.challenges.confirmed_apply_request()?.is_some() {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        let clock = self.kernel.snapshot()?;
        let request =
            self.challenges
                .native_confirmation_request(operation_id, &started.slot_id, clock)?;
        let context = NativeSlotIdentity::open(&self.manager, &started.slot_id)?;
        let outcome = self.manager.confirm_native_activation(
            expected_generation,
            &request,
            || {
                let remaining = pending
                    .deadline_monotonic_ms
                    .checked_sub(clock.monotonic_ms)
                    .and_then(|remaining| {
                        observed_at.checked_add(std::time::Duration::from_millis(remaining))
                    })
                    .ok_or(UpdateManagerError::RecoveryRequired)?;
                if deadline <= std::time::Instant::now() || deadline > remaining {
                    return Err(UpdateManagerError::RecoveryRequired);
                }
                context
                    .check_readiness(controller, SlotServer::Candidate, deadline, probe)
                    .and_then(|ready| {
                        if &ready != started {
                            Err(CompositionError::SelectionMismatch)
                        } else if std::time::Instant::now() >= deadline {
                            Err(CompositionError::Deadline)
                        } else {
                            Ok(())
                        }
                    })
                    .map_err(|error| {
                        // The journal records only the rollback reason.
                        eprintln!("native candidate readiness failed: {error:?}");
                        UpdateManagerError::RecoveryRequired
                    })
            },
            || {
                self.kernel
                    .snapshot()
                    .map_err(|_| UpdateManagerError::InvalidClock)
            },
        )?;
        if outcome == bli_kvm_update_manager::NativeConfirmationOutcome::Confirmed {
            let generation = self.manager.native_status()?.generation;
            self.cleanup_native_confirmation(generation, operation_id, &started.slot_id)?;
        }
        Ok(outcome)
    }

    /// Cleanup-only replay after successful native confirmation, including partial
    /// credential deletion. Historical startup identity does not authorize cleanup.
    /// Record an explicit native rejection of the exact pending candidate, so
    /// that the next native boot restores the confirmed composition at once
    /// instead of waiting for the health deadline. Services are not touched.
    pub fn reject_native_candidate(
        &self,
        operation_id: [u8; 16],
    ) -> Result<(), UpdateRuntimeError> {
        let generation = self.manager.inspect_recovery()?.generation;
        self.manager
            .begin_native_rejection(generation, operation_id)?;
        Ok(())
    }

    pub fn cleanup_native_confirmation(
        &self,
        expected_generation: u64,
        operation_id: [u8; 16],
        target_slot_id: &str,
    ) -> Result<(), UpdateRuntimeError> {
        let mut cleanup_error = None;
        let result = self.manager.cleanup_native_confirmation(
            expected_generation,
            operation_id,
            target_slot_id,
            |binding| {
                let result = (|| {
                    if self.challenges.confirmed_apply_request()?.is_some() {
                        return Err(UpdateRuntimeError::InvalidApplyRequest);
                    }
                    self.challenges.consume_exact(
                        operation_id,
                        crate::challenge::decode_array(binding)?,
                        target_slot_id,
                    )?;
                    Ok(())
                })();
                result.map_err(|error| {
                    cleanup_error = Some(error);
                    UpdateManagerError::RecoveryRequired
                })
            },
        );
        if let Some(error) = cleanup_error {
            return Err(error);
        }
        Ok(result?)
    }

    /// Continue an already durable native rollback intent. Every retry quiesces
    /// again before selecting the confirmed slot. This performs restored startup
    /// and identity-fenced probes, persists completion, then cleans the exact
    /// challenge. A cleanup failure retains completion for cleanup-only replay.
    /// The trusted controller and probes must enforce the supplied deadline.
    pub fn restore_native_composition(
        &self,
        expected_generation: u64,
        intent: &bli_kvm_update_manager::NativeRollbackIntent,
        controller: &mut impl crate::native_identity::NativeStartController,
        deadline: std::time::Instant,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<
        (
            bli_kvm_update_manager::RecoveryInspection,
            crate::native_identity::NativeIdentityObservation,
        ),
        UpdateRuntimeError,
    > {
        use crate::{
            composition::{self, CompositionError},
            native_identity::{NativeSlotIdentity, NativeStartUnit, SlotServer},
        };
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline.into());
        }
        let recovery = self.manager.inspect_recovery()?;
        if recovery.generation != expected_generation
            || recovery.rollback_intent.as_ref() != Some(intent)
        {
            return Err(UpdateManagerError::RecoveryRequired.into());
        }
        // Direct restoration callers also need this pre-disruption guard; the
        // completion cleanup check is too late to prevent competing apply work.
        if self.challenges.confirmed_apply_request()?.is_some() {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        // Validate the complete confirmed inventory without requiring an intact
        // candidate. The context keeps verified executable handles and locks.
        let context = NativeSlotIdentity::open(&self.manager, &recovery.confirmed_slot_id)?;
        for unit in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(SlotServer::Current),
        ] {
            if std::time::Instant::now() >= deadline {
                return Err(CompositionError::Deadline.into());
            }
            controller.validate_start(unit)?;
        }
        controller.prepare(context.slot_id())?;
        let mut stop_error = None;
        let restored = self
            .manager
            .restore_native_selection(expected_generation, intent, || {
                let result = if std::time::Instant::now() >= deadline {
                    Err(CompositionError::Deadline)
                } else {
                    composition::quiesce(controller)
                };
                result
                    .and_then(|()| {
                        if std::time::Instant::now() >= deadline {
                            Err(CompositionError::Deadline)
                        } else {
                            Ok(())
                        }
                    })
                    .map_err(|error| {
                        stop_error = Some(error);
                        UpdateManagerError::RecoveryRequired
                    })
            });
        if let Some(error) = stop_error {
            return Err(error.into());
        }
        let restored = restored?;
        let restoring = restored
            .rollback_intent
            .as_ref()
            .ok_or(UpdateManagerError::RecoveryRequired)?;
        let started = context.start_restored_and_observe(
            controller,
            restored.generation,
            restoring,
            deadline,
        )?;
        let mut readiness_error = None;
        let completion =
            self.manager
                .acknowledge_native_restoration(restored.generation, restoring, || {
                    context
                        .check_readiness(controller, SlotServer::Current, deadline, probe)
                        .and_then(|ready| {
                            if ready != started {
                                Err(CompositionError::SelectionMismatch)
                            } else if std::time::Instant::now() >= deadline {
                                Err(CompositionError::Deadline)
                            } else {
                                Ok(())
                            }
                        })
                        .map_err(|error| {
                            readiness_error = Some(error);
                            UpdateManagerError::RecoveryRequired
                        })
                });
        if let Some(error) = readiness_error {
            return Err(error.into());
        }
        let completion = completion?;
        self.cleanup_native_completion(completion.completion_generation, &completion)?;
        Ok((self.manager.inspect_recovery()?, started))
    }

    /// Replay cleanup without restarting services or treating historical readiness
    /// as current health. The manager retains the exclusive journal/slot locks.
    pub fn cleanup_native_completion(
        &self,
        expected_generation: u64,
        completion: &bli_kvm_update_manager::NativeRecoveryCompletion,
    ) -> Result<(), UpdateRuntimeError> {
        let before = self.manager.inspect_recovery()?;
        if before.generation != expected_generation
            || before.completion.as_ref() != Some(completion)
            || before.selection != bli_kvm_update_manager::RecoverySelection::Confirmed
        {
            return Err(UpdateManagerError::RecoveryRequired.into());
        }
        // No native operation publishes this legacy server-only restart request.
        // Its presence cannot be attributed to this completion by slot alone.
        if self.challenges.confirmed_apply_request()?.is_some() {
            return Err(UpdateRuntimeError::InvalidApplyRequest);
        }
        self.challenges.consume_exact(
            crate::challenge::decode_array(&completion.intent.operation_id)?,
            crate::challenge::decode_array(&completion.health_binding)?,
            &completion.failed_target_slot_id,
        )?;
        if self.manager.inspect_recovery()? != before {
            return Err(UpdateManagerError::RecoveryRequired.into());
        }
        Ok(())
    }

    /// Boot an idle schema-3 selection with fresh full-composition readiness.
    /// Pending recovery and leftover credentials require their exact recovery or
    /// cleanup path first. This neither migrates nor changes the journal/floor.
    pub fn start_idle_composition(
        &self,
        expected_generation: u64,
        controller: &mut impl crate::native_identity::NativeStartController,
        deadline: std::time::Instant,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<crate::native_identity::NativeIdentityObservation, UpdateRuntimeError> {
        use crate::{
            composition::{self, CompositionError},
            native_identity::{NativeSlotIdentity, NativeStartUnit, SlotServer},
        };
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline.into());
        }
        // native_status refuses legacy schemas and unacknowledged recovery.
        let status = self.manager.native_status()?;
        if status.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        if status.pending.is_some() || status.active_slot_id != status.confirmed_slot_id {
            return Err(UpdateManagerError::RecoveryRequired.into());
        }
        self.challenges.require_empty_native_runtime()?;
        let before = self.manager.inspect_recovery()?;
        let context = NativeSlotIdentity::open(&self.manager, &status.confirmed_slot_id)?;
        for unit in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(SlotServer::Current),
        ] {
            if std::time::Instant::now() >= deadline {
                return Err(CompositionError::Deadline.into());
            }
            controller.validate_start(unit)?;
        }
        controller.prepare(context.slot_id())?;
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        if self.manager.inspect_recovery()? != before {
            return Err(CompositionError::SelectionMismatch.into());
        }
        self.challenges.require_empty_native_runtime()?;
        composition::quiesce(controller)?;
        // Revalidate after a callback and before starting any selected service.
        if self.manager.inspect_recovery()? != before {
            return Err(CompositionError::SelectionMismatch.into());
        }
        self.challenges.require_empty_native_runtime()?;
        let started = context.start_and_observe(controller, SlotServer::Current, deadline)?;
        let ready = context.check_readiness(controller, SlotServer::Current, deadline, probe)?;
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        if started != ready || self.manager.inspect_recovery()? != before {
            return Err(CompositionError::SelectionMismatch.into());
        }
        self.challenges.require_empty_native_runtime()?;
        Ok(ready)
    }

    /// Boot/start after a completed rollback. Historical completion is an exact
    /// selection fence, never evidence that today's services are already healthy.
    pub fn start_completed_composition(
        &self,
        expected_generation: u64,
        completion: &bli_kvm_update_manager::NativeRecoveryCompletion,
        controller: &mut impl crate::native_identity::NativeStartController,
        deadline: std::time::Instant,
        probe: impl FnMut(
            crate::composition::ApplicationUnit,
            &crate::process_identity::ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), crate::composition::CompositionError>,
    ) -> Result<crate::native_identity::NativeIdentityObservation, UpdateRuntimeError> {
        use crate::{
            composition::{self, CompositionError},
            native_identity::{NativeSlotIdentity, NativeStartUnit, SlotServer},
        };
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline.into());
        }
        // Cleanup validates exact receipt, generation, anchor and selection and
        // refuses incompatible legacy apply requests before any service mutation.
        self.cleanup_native_completion(expected_generation, completion)?;
        let before = self.manager.inspect_recovery()?;
        let context = NativeSlotIdentity::open(&self.manager, &before.confirmed_slot_id)?;
        for unit in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(SlotServer::Current),
        ] {
            if std::time::Instant::now() >= deadline {
                return Err(CompositionError::Deadline.into());
            }
            controller.validate_start(unit)?;
        }
        controller.prepare(context.slot_id())?;
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        composition::quiesce(controller)?;
        let started = context.start_completed_and_observe(
            controller,
            expected_generation,
            completion,
            deadline,
        )?;
        let ready = context.check_readiness(controller, SlotServer::Current, deadline, probe)?;
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline.into());
        }
        if started != ready || self.manager.inspect_recovery()? != before {
            return Err(CompositionError::SelectionMismatch.into());
        }
        Ok(ready)
    }

    fn check_native_activation_precondition(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
    ) -> Result<(), UpdateRuntimeError> {
        let status = self.manager.native_status()?;
        if status.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        if status.pending.is_some() {
            return Err(UpdateManagerError::PendingActivation.into());
        }
        if status.active_slot_id == target_slot_id {
            return Err(UpdateRuntimeError::ActivationTargetAlreadyActive);
        }
        let _inventory =
            crate::native_identity::NativeSlotIdentity::open(&self.manager, target_slot_id)?;
        self.challenges.require_empty_native_runtime()
    }

    fn activate_native_with_admission(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
        deliver: impl FnOnce(&SupervisorAdmission) -> Result<(), UpdateManagerError>,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        self.check_native_activation_precondition(target_slot_id, expected_generation)?;
        let clock = self.kernel.snapshot()?;
        let deadline = clock
            .monotonic_ms
            .checked_add(self.manager.health_timeout_ms())
            .ok_or(UpdateRuntimeError::InvalidBootClock)?;
        // A published credential must already have a durable journal owner.
        // Publication failure retains Prepared for native recovery; no legacy
        // watchdog or server-only apply request is permitted here.
        let issued =
            self.challenges
                .issue_after_admission(target_slot_id, clock, deadline, |issued| {
                    let request = ActivationRequest {
                        operation_id: issued.operation_id,
                        target_slot_id: target_slot_id.to_owned(),
                        health_binding: issued.health_binding,
                        clock,
                    };
                    if self
                        .manager
                        .admit_native_activation(expected_generation, &request)?
                        != bli_kvm_update_manager::ActivationPreparation::Prepared
                    {
                        return Err(UpdateRuntimeError::SupervisorStateMismatch);
                    }
                    Ok(())
                })?;
        let mut request = ActivationRequest {
            operation_id: issued.operation_id,
            target_slot_id: target_slot_id.to_owned(),
            health_binding: issued.health_binding,
            clock,
        };
        let prepared = self.manager.native_status()?;
        deliver(&SupervisorAdmission {
            operation_id: issued.operation_id,
            target_slot_id: target_slot_id.to_owned(),
            prepared_generation: prepared.generation,
            deadline_monotonic_ms: deadline,
        })?;
        let before_stop = self.kernel.snapshot()?;
        validate_quiescence_clock(clock, before_stop, deadline)?;
        request.clock = before_stop;
        let outcome = self.manager.execute_native_prepared_activation(
            prepared.generation,
            request,
            quiesce,
            || {
                self.kernel
                    .snapshot()
                    .map_err(|_| UpdateManagerError::InvalidClock)
            },
        )?;
        if !matches!(&outcome, ActivationOutcome::AwaitingHealth {
            operation_id, slot_id, deadline_monotonic_ms,
        } if *operation_id == issued.operation_id && slot_id == target_slot_id
            && *deadline_monotonic_ms == deadline)
        {
            return Err(UpdateRuntimeError::SupervisorStateMismatch);
        }
        Ok(SupervisorActivation {
            outcome,
            credential_path: Some(issued.credential_path),
        })
    }

    fn check_activation_precondition(
        &self,
        target_slot_id: &str,
        expected_generation: u64,
        native: bool,
    ) -> Result<(), UpdateRuntimeError> {
        let status = self.manager.status()?;
        if status.generation != expected_generation {
            return Err(UpdateRuntimeError::StaleActivationGeneration);
        }
        if status.pending.is_some() {
            return Err(UpdateManagerError::PendingActivation.into());
        }
        if status.active_slot_id == target_slot_id {
            return Err(UpdateRuntimeError::ActivationTargetAlreadyActive);
        }
        // Invalid/untrusted targets must not provoke challenge issuance or
        // watchdog housekeeping through the network activation entrypoint.
        let staged = self.manager.inspect_slot(target_slot_id)?;
        if native {
            crate::composition::validate_native_payload(
                staged
                    .manifest
                    .files
                    .iter()
                    .map(|file| (file.path.as_str(), file.mode, file.size)),
            )
            .map_err(|_| UpdateRuntimeError::IncompleteNativePayload)?;
        }
        Ok(())
    }

    fn activate_inner(
        &self,
        target_slot_id: &str,
        expected_generation: Option<u64>,
        native: bool,
        deliver: impl FnOnce(&SupervisorAdmission) -> Result<(), UpdateManagerError>,
        quiesce: impl FnOnce() -> Result<(), UpdateManagerError>,
    ) -> Result<SupervisorActivation, UpdateRuntimeError> {
        if let Some(generation) = expected_generation {
            self.check_activation_precondition(target_slot_id, generation, native)?;
        }
        let _ = self.watchdog_once()?;
        if let Some(generation) = expected_generation {
            // Recovery may itself advance the journal. That does not refresh
            // the user's confirmation or authorize a new target transition.
            self.check_activation_precondition(target_slot_id, generation, native)?;
        }
        if self.manager.status()?.pending.is_some() {
            return Err(UpdateManagerError::PendingActivation.into());
        }
        let clock = self.kernel.snapshot()?;
        let deadline = clock
            .monotonic_ms
            .checked_add(self.manager.health_timeout_ms())
            .ok_or(UpdateRuntimeError::InvalidBootClock)?;
        let issued = self.challenges.issue(target_slot_id, clock, deadline)?;
        let request = ActivationRequest {
            operation_id: issued.operation_id,
            target_slot_id: target_slot_id.to_string(),
            health_binding: issued.health_binding,
            clock,
        };
        let outcome = match self.manager.activate_with_quiescence(request, || {
            let prepared = self.manager.status()?;
            deliver(&SupervisorAdmission {
                operation_id: issued.operation_id,
                target_slot_id: target_slot_id.to_owned(),
                prepared_generation: prepared.generation,
                deadline_monotonic_ms: deadline,
            })?;
            let before_stop = self
                .kernel
                .snapshot()
                .map_err(|_| UpdateManagerError::InvalidClock)?;
            validate_quiescence_clock(clock, before_stop, deadline)?;
            quiesce()?;
            let after = self
                .kernel
                .snapshot()
                .map_err(|_| UpdateManagerError::InvalidClock)?;
            validate_quiescence_clock(clock, after, deadline)
        }) {
            Ok(outcome) => outcome,
            Err(error) => {
                // A partially quiesced composition still needs confirmed-apply
                // recovery. Keep the challenge as a crash-recovery anchor until
                // the watchdog publishes that request, even if journal recovery
                // finishes before the request can be stored.
                if self
                    .manager
                    .status()
                    .is_ok_and(|status| status.pending.is_none())
                {
                    self.challenges.discard(issued.operation_id)?;
                }
                return Err(error.into());
            }
        };
        match &outcome {
            ActivationOutcome::AwaitingHealth {
                operation_id,
                slot_id,
                deadline_monotonic_ms,
            } if *operation_id == issued.operation_id
                && slot_id == target_slot_id
                && *deadline_monotonic_ms == deadline =>
            {
                Ok(SupervisorActivation {
                    outcome,
                    credential_path: Some(issued.credential_path),
                })
            }
            ActivationOutcome::AlreadyActive
            | ActivationOutcome::ReplayConfirmed
            | ActivationOutcome::ReplayRolledBack { .. } => {
                self.challenges.discard(issued.operation_id)?;
                Ok(SupervisorActivation {
                    outcome,
                    credential_path: None,
                })
            }
            ActivationOutcome::AwaitingHealth { .. } => {
                let _ = self.manager.abort_missing_health_challenge();
                let _ = self.challenges.discard(issued.operation_id);
                Err(UpdateRuntimeError::SupervisorStateMismatch)
            }
        }
    }

    pub fn confirm_from_reader(
        &self,
        reader: &mut impl Read,
    ) -> Result<ConfirmationOutcome, UpdateRuntimeError> {
        let status = self
            .challenges
            .status()?
            .ok_or(UpdateRuntimeError::ChallengeMissing)?;
        let mut response = [0_u8; 32];
        if reader.read_exact(&mut response).is_err() {
            response.zeroize();
            return Err(UpdateRuntimeError::InvalidHealthResponseLength);
        }
        let mut extra = [0_u8; 1];
        match reader.read(&mut extra) {
            Ok(0) => {}
            Ok(_) | Err(_) => {
                response.zeroize();
                return Err(UpdateRuntimeError::InvalidHealthResponseLength);
            }
        }
        let clock = match self.kernel.snapshot() {
            Ok(clock) => clock,
            Err(error) => {
                response.zeroize();
                return Err(error);
            }
        };
        let verified =
            self.challenges
                .verify_response(status.operation_id, &mut response, clock)?;
        let outcome =
            self.manager
                .confirm_health(verified.operation_id, verified.health_binding, clock)?;
        self.request_confirmed_apply()?;
        self.challenges.consume(verified.operation_id)?;
        Ok(outcome)
    }

    pub fn watchdog_once(&self) -> Result<SupervisorRecovery, UpdateRuntimeError> {
        let storage = self.manager.recover_update_storage()?;
        let abandoned_staging_removed = storage.abandoned_staging_removed;
        let slot_deletions_completed = storage.slot_deletions_completed;
        let clock = self.kernel.snapshot()?;
        let initial = self.manager.recover(clock)?;
        let status = self.manager.status()?;
        if status.pending.is_none()
            && matches!(
                initial,
                RecoveryOutcome::RolledBack { .. } | RecoveryOutcome::RepairedPointer { .. }
            )
        {
            // Publish before clearing the challenge, so a crash leaves either
            // the request or stale challenge as evidence for the next pass.
            self.request_confirmed_apply()?;
        }
        let challenge = match (status.pending.as_ref(), self.challenges.status()) {
            (Some(pending), Ok(Some(challenge))) if challenge_matches(pending, &challenge) => {
                ChallengeReconciliation::Ready
            }
            (Some(_), Ok(None)) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                self.request_confirmed_apply()?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMissing,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Ok(Some(challenge))) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                self.request_confirmed_apply()?;
                self.challenges.discard(challenge.operation_id)?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMismatch,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Err(UpdateRuntimeError::InvalidChallengeState)) => {
                let manager = self.manager.abort_missing_health_challenge()?;
                self.request_confirmed_apply()?;
                let _ = self.challenges.clear_stale_after_rollback()?;
                return Ok(SupervisorRecovery {
                    manager,
                    challenge: ChallengeReconciliation::RolledBackMismatch,
                    abandoned_staging_removed,
                    slot_deletions_completed,
                });
            }
            (Some(_), Err(error)) => return Err(error),
            (None, Ok(Some(challenge))) => {
                self.request_confirmed_apply()?;
                self.challenges.discard(challenge.operation_id)?;
                ChallengeReconciliation::DiscardedStale
            }
            (None, Err(UpdateRuntimeError::InvalidChallengeState)) => {
                self.request_confirmed_apply()?;
                self.challenges.clear_stale_after_rollback()?;
                ChallengeReconciliation::DiscardedStale
            }
            (None, Ok(None)) => ChallengeReconciliation::None,
            (None, Err(error)) => return Err(error),
        };
        if self.manager.status()?.pending.is_none()
            && self.challenges.confirmed_apply_request()?.is_some()
        {
            self.request_confirmed_apply()?;
        }
        Ok(SupervisorRecovery {
            manager: initial,
            challenge,
            abandoned_staging_removed,
            slot_deletions_completed,
        })
    }
}

fn challenge_matches(
    pending: &bli_kvm_update_manager::PendingStatus,
    challenge: &HealthChallengeStatus,
) -> bool {
    pending.operation_id == encode_hex(&challenge.operation_id)
        && pending.target_slot_id == challenge.target_slot_id
        && pending.boot_id == encode_hex(&challenge.boot_id)
        && pending.deadline_monotonic_ms == challenge.deadline_monotonic_ms
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut value = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        value.push(char::from(HEX[usize::from(byte >> 4)]));
        value.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    value
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{BTreeMap, BTreeSet},
        fs::{self, Permissions},
        io::Cursor,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
    };

    use bli_kvm_update_manager::{
        ApplicationUpdateManager, ConfirmationOutcome, DEFAULT_HEALTH_TIMEOUT_MS, RollbackReason,
    };
    use bli_kvm_update_stager::{ApplicationSlotStager, StagedApplication};
    use bli_kvm_update_verifier::{
        APPLICATION_ARCHITECTURE, APPLICATION_FORMAT_V1, APPLICATION_MANIFEST_PATH,
        APPLICATION_PRODUCT, APPLICATION_SIGNATURE_PATH, APPLICATION_TARGET, ApplicationFile,
        ApplicationManifest, ApplicationTrustRoot,
    };
    use ed25519_dalek::{Signer, SigningKey};
    use sha2::{Digest, Sha256};
    use tar::{Builder, EntryType, Header};
    use tempfile::TempDir;

    use super::*;

    const KEY_ID: &str = "app-supervisor-test-key-1";

    #[test]
    fn quiescence_cannot_switch_after_expiry_boot_change_or_clock_reversal() {
        use bli_kvm_update_manager::ClockSnapshot;
        let before = ClockSnapshot {
            boot_id: [1; 16],
            monotonic_ms: 100,
        };
        for after in [
            ClockSnapshot {
                monotonic_ms: 201,
                ..before
            },
            ClockSnapshot {
                monotonic_ms: 99,
                ..before
            },
            ClockSnapshot {
                boot_id: [2; 16],
                monotonic_ms: 150,
            },
        ] {
            assert!(matches!(
                validate_quiescence_clock(before, after, 200),
                Err(UpdateManagerError::RecoveryRequired)
            ));
        }
        for time in [100, 150, 200] {
            validate_quiescence_clock(
                before,
                ClockSnapshot {
                    monotonic_ms: time,
                    ..before
                },
                200,
            )
            .unwrap();
        }
    }

    struct Fixture {
        slots: TempDir,
        state: TempDir,
        runtime: TempDir,
        sources: TempDir,
        signing: SigningKey,
        trust: ApplicationTrustRoot,
    }

    impl Fixture {
        fn new() -> Self {
            let signing = SigningKey::from_bytes(&[37; 32]);
            let trust =
                ApplicationTrustRoot::new(KEY_ID, signing.verifying_key().to_bytes()).unwrap();
            Self {
                slots: tempfile::tempdir().unwrap(),
                state: tempfile::tempdir().unwrap(),
                runtime: tempfile::tempdir().unwrap(),
                sources: tempfile::tempdir().unwrap(),
                signing,
                trust,
            }
        }

        fn stage(&self, sequence: u64) -> StagedApplication {
            let payload = payload(sequence);
            self.stage_payload(sequence, payload)
        }

        fn stage_payload(
            &self,
            sequence: u64,
            payload: BTreeMap<&str, (u32, Vec<u8>)>,
        ) -> StagedApplication {
            let manifest = manifest(sequence, &payload);
            let archive = bundle(&self.signing, &manifest, &payload);
            let source = self
                .sources
                .path()
                .join(format!("application-{sequence}.tar"));
            fs::write(&source, archive).unwrap();
            fs::set_permissions(&source, Permissions::from_mode(0o600)).unwrap();
            ApplicationSlotStager::open(self.slots.path())
                .unwrap()
                .stage(source, &self.trust)
                .unwrap()
        }

        fn supervisor(&self) -> UpdateSupervisor {
            let manager = ApplicationUpdateManager::open(
                self.slots.path(),
                self.state.path(),
                self.trust.clone(),
                1,
                DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .unwrap();
            let challenges = HealthChallengeStore::open(self.runtime.path()).unwrap();
            UpdateSupervisor::new(manager, challenges)
        }
    }

    #[test]
    fn failed_quiescence_keeps_recovery_anchor_across_interrupted_rollback() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let generation = supervisor.status().unwrap().generation;
        assert!(
            supervisor
                .activate_if_generation_with_quiescence(&candidate.slot_id, generation, || {
                    assert_eq!(
                        supervisor.status().unwrap().pending.unwrap().phase,
                        bli_kvm_update_manager::PendingPhaseStatus::Prepared
                    );
                    Err(UpdateManagerError::RecoveryRequired)
                })
                .is_err()
        );
        assert!(supervisor.challenges.status().unwrap().is_some());
        assert_eq!(
            supervisor.status().unwrap().active_slot_id,
            previous.slot_id
        );
        // Simulate the watchdog stopping after durable journal rollback but
        // before publishing the confirmed-apply request.
        supervisor
            .manager
            .recover(supervisor.kernel.snapshot().unwrap())
            .unwrap();
        assert!(
            supervisor
                .challenges
                .confirmed_apply_request()
                .unwrap()
                .is_none()
        );
        drop(supervisor);
        let reopened = fixture.supervisor();
        let recovery = reopened.watchdog_once().unwrap();
        assert_eq!(recovery.challenge, ChallengeReconciliation::DiscardedStale);
        let apply = reopened
            .challenges
            .confirmed_apply_request()
            .unwrap()
            .unwrap();
        assert_eq!(apply.slot_id, previous.slot_id);
        assert_eq!(reopened.status().unwrap().floor_sequence, 1);
        assert!(reopened.challenges.status().unwrap().is_none());
    }

    #[test]
    fn rejecting_a_native_candidate_records_intent_for_immediate_restoration() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let idle = supervisor
            .manager
            .migrate_native_idle(1, |_| Ok(()))
            .unwrap();
        let clock = supervisor.kernel.snapshot().unwrap();
        let issued = supervisor
            .challenges
            .issue(
                &candidate.slot_id,
                clock,
                clock.monotonic_ms + DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .unwrap();
        let request = ActivationRequest {
            operation_id: issued.operation_id,
            health_binding: issued.health_binding,
            target_slot_id: candidate.slot_id.clone(),
            clock,
        };
        supervisor
            .manager
            .admit_native_activation(idle, &request)
            .unwrap();
        let prepared = supervisor.manager.inspect_recovery().unwrap();
        supervisor
            .manager
            .execute_native_prepared_activation(
                prepared.generation,
                request,
                || Ok(()),
                || Ok(clock),
            )
            .unwrap();
        // A foreign operation is refused without changing the journal.
        let before = supervisor.manager.inspect_recovery().unwrap();
        assert!(supervisor.reject_native_candidate([7; 16]).is_err());
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), before);
        supervisor
            .reject_native_candidate(issued.operation_id)
            .unwrap();
        let rejected = supervisor.manager.inspect_recovery().unwrap();
        let intent = rejected.rollback_intent.unwrap();
        assert_eq!(intent.reason, RollbackReason::ExplicitHealthFailure);
        assert_eq!(rejected.confirmed_slot_id, previous.slot_id);
        // An existing intent is kept, never relabeled.
        assert!(
            supervisor
                .reject_native_candidate(issued.operation_id)
                .is_err()
        );
    }

    #[test]
    fn native_credential_loss_records_intent_without_cleanup_or_legacy_apply() {
        for case in ["complete", "state-only", "token-only", "absent", "mismatch"] {
            let fixture = Fixture::new();
            let previous = fixture.stage(1);
            let candidate = fixture.stage(2);
            let supervisor = fixture.supervisor();
            supervisor.initialize(&previous.slot_id).unwrap();
            let idle = supervisor
                .manager
                .migrate_native_idle(1, |_| Ok(()))
                .unwrap();
            let clock = supervisor.kernel.snapshot().unwrap();
            let issued = supervisor
                .challenges
                .issue(
                    &candidate.slot_id,
                    clock,
                    clock.monotonic_ms + DEFAULT_HEALTH_TIMEOUT_MS,
                )
                .unwrap();
            let request = ActivationRequest {
                operation_id: issued.operation_id,
                health_binding: issued.health_binding,
                target_slot_id: candidate.slot_id.clone(),
                clock,
            };
            supervisor
                .manager
                .admit_native_activation(idle, &request)
                .unwrap();
            let prepared = supervisor.manager.inspect_recovery().unwrap();
            supervisor
                .manager
                .execute_native_prepared_activation(
                    prepared.generation,
                    request,
                    || Ok(()),
                    || Ok(clock),
                )
                .unwrap();
            let state = fixture.runtime.path().join("health-challenge.json");
            let token = fixture.runtime.path().join("health-token");
            if matches!(case, "token-only" | "absent") {
                fs::remove_file(&state).unwrap();
            }
            if matches!(case, "state-only" | "absent") {
                fs::remove_file(&token).unwrap();
            }
            if case == "mismatch" {
                fs::set_permissions(&token, Permissions::from_mode(0o600)).unwrap();
                fs::write(&token, [99u8; 32]).unwrap();
                fs::set_permissions(&token, Permissions::from_mode(0o400)).unwrap();
            }
            let state_before = fs::read(&state).ok();
            let token_before = fs::read(&token).ok();
            let before = supervisor.manager.native_status().unwrap();
            let result = supervisor.begin_native_credential_recovery(before.generation);
            if case == "mismatch" {
                assert!(result.is_err());
                assert_eq!(supervisor.manager.native_status().unwrap(), before);
            } else if case == "complete" {
                assert!(result.unwrap().is_none());
                assert_eq!(supervisor.manager.native_status().unwrap(), before);
            } else {
                assert_eq!(
                    result.unwrap().unwrap().reason,
                    bli_kvm_update_manager::RollbackReason::HealthChallengeLost
                );
                let after = supervisor.manager.inspect_recovery().unwrap();
                assert_eq!(after.pending, before.pending);
                assert_eq!(after.confirmed_slot_id, previous.slot_id);
                assert_eq!(
                    fs::read_link(fixture.slots.path().join("current"))
                        .unwrap()
                        .file_name()
                        .unwrap(),
                    candidate.slot_id.as_str()
                );
            }
            assert_eq!(fs::read(&state).ok(), state_before);
            assert_eq!(fs::read(&token).ok(), token_before);
            assert!(
                supervisor
                    .challenges
                    .confirmed_apply_request()
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn native_idle_migration_refuses_orphan_credentials_and_legacy_apply() {
        for case in 0..4 {
            let fixture = Fixture::new();
            let confirmed = fixture.stage(1);
            let supervisor = fixture.supervisor();
            supervisor.initialize(&confirmed.slot_id).unwrap();
            let before = supervisor.status().unwrap();
            if case == 3 {
                supervisor
                    .challenges
                    .request_confirmed_apply(before.generation, &confirmed.slot_id)
                    .unwrap();
            } else {
                let now = supervisor.kernel.snapshot().unwrap();
                supervisor
                    .challenges
                    .issue(&confirmed.slot_id, now, now.monotonic_ms + 60_000)
                    .unwrap();
                if case == 1 {
                    fs::remove_file(fixture.runtime.path().join("health-token")).unwrap();
                }
                if case == 2 {
                    fs::remove_file(fixture.runtime.path().join("health-challenge.json")).unwrap();
                }
            }
            let nodes_before = fs::read_dir(fixture.runtime.path()).unwrap().count();
            assert!(supervisor.migrate_native_idle(before.generation).is_err());
            assert_eq!(supervisor.status().unwrap(), before);
            assert_eq!(
                fs::read_dir(fixture.runtime.path()).unwrap().count(),
                nodes_before
            );
        }
    }

    #[test]
    fn native_idle_migration_checks_inventory_and_replay_runtime() {
        let fixture = Fixture::new();
        let mut legacy = payload(1);
        legacy.remove("libexec/bli-kvm-boardd");
        let incomplete = fixture.stage_payload(1, legacy);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&incomplete.slot_id).unwrap();
        let before = supervisor.status().unwrap();
        assert!(supervisor.migrate_native_idle(before.generation).is_err());
        assert_eq!(supervisor.status().unwrap(), before);
        drop(supervisor);

        let fixture = Fixture::new();
        let confirmed = fixture.stage(1);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&confirmed.slot_id).unwrap();
        let before = supervisor.status().unwrap();
        assert_eq!(supervisor.manager.observed_status().unwrap(), before);
        let generation = supervisor.migrate_native_idle(before.generation).unwrap();
        assert_eq!(generation, before.generation + 1);
        // The update status socket reads a native journal through the same
        // observed status that serves the legacy one.
        assert!(supervisor.status().is_err());
        assert_eq!(
            supervisor.manager.observed_status().unwrap(),
            supervisor.native_status().unwrap()
        );
        drop(supervisor);
        let supervisor = fixture.supervisor();
        assert_eq!(
            supervisor.migrate_native_idle(generation).unwrap(),
            generation
        );
        let now = supervisor.kernel.snapshot().unwrap();
        supervisor
            .challenges
            .issue(&confirmed.slot_id, now, now.monotonic_ms + 60_000)
            .unwrap();
        assert!(supervisor.migrate_native_idle(generation).is_err());
        assert!(supervisor.challenges.status().unwrap().is_some());
        assert_eq!(
            supervisor.manager.inspect_recovery().unwrap().generation,
            generation
        );
    }

    #[test]
    fn rejected_native_admission_does_not_publish_orphan_credentials() {
        let fixture = Fixture::new();
        let older = fixture.stage(1);
        let confirmed = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&confirmed.slot_id).unwrap();
        let idle = supervisor.migrate_native_idle(1).unwrap();
        let before = supervisor.native_status().unwrap();
        let result = supervisor.activate_native_with_admission(
            &older.slot_id,
            idle,
            |_| panic!("rejected admission delivery"),
            || panic!("rejected quiescence"),
        );
        assert!(matches!(
            result,
            Err(UpdateRuntimeError::Manager(
                UpdateManagerError::AntiRollback
            ))
        ));
        assert_eq!(supervisor.native_status().unwrap(), before);
        supervisor
            .challenges
            .require_empty_native_runtime()
            .unwrap();
    }

    #[test]
    fn interrupted_native_issuance_retains_prepared_authority_after_reopen() {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1);
        let target = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&confirmed.slot_id).unwrap();
        let idle = supervisor.migrate_native_idle(1).unwrap();
        let clock = supervisor.kernel.snapshot().unwrap();
        let result = supervisor.challenges.issue_after_admission(
            &target.slot_id,
            clock,
            clock.monotonic_ms + DEFAULT_HEALTH_TIMEOUT_MS,
            |issued| {
                supervisor.manager.admit_native_activation(
                    idle,
                    &ActivationRequest {
                        operation_id: issued.operation_id,
                        health_binding: issued.health_binding,
                        target_slot_id: target.slot_id.clone(),
                        clock,
                    },
                )?;
                // Simulate loss of the caller after journal persistence but
                // before issuance is permitted to publish either file.
                Err(UpdateRuntimeError::SupervisorStateMismatch)
            },
        );
        assert!(result.is_err());
        supervisor
            .challenges
            .require_empty_native_runtime()
            .unwrap();
        drop(supervisor);
        let reopened = fixture.supervisor();
        let before = reopened.native_status().unwrap();
        assert_eq!(before.active_slot_id, confirmed.slot_id);
        assert_eq!(
            before.pending.as_ref().unwrap().phase,
            bli_kvm_update_manager::PendingPhaseStatus::Prepared
        );
        let intent = reopened
            .begin_native_credential_recovery(before.generation)
            .unwrap()
            .unwrap();
        assert_eq!(
            intent.reason,
            bli_kvm_update_manager::RollbackReason::InterruptedPrepare
        );
        assert_eq!(
            reopened.manager.inspect_recovery().unwrap().pending,
            before.pending
        );
        reopened.challenges.require_empty_native_runtime().unwrap();
    }

    #[test]
    fn native_idle_migration_consumes_only_current_completion_credentials() {
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&confirmed.slot_id).unwrap();
        supervisor.activate(&candidate.slot_id).unwrap();
        let before = supervisor.status().unwrap();
        let challenge = supervisor.challenges.status().unwrap().unwrap();
        let intent = supervisor
            .manager
            .begin_native_rejection(before.generation, challenge.operation_id)
            .unwrap();
        let rejected = supervisor.manager.inspect_recovery().unwrap();
        let restored = supervisor
            .manager
            .restore_native_selection(rejected.generation, &intent, || Ok(()))
            .unwrap();
        let completion = supervisor
            .manager
            .acknowledge_native_restoration(
                restored.generation,
                restored.rollback_intent.as_ref().unwrap(),
                || Ok(()),
            )
            .unwrap();
        assert!(supervisor.challenges.status().unwrap().is_some());
        let generation = supervisor
            .migrate_native_idle(completion.completion_generation)
            .unwrap();
        assert!(supervisor.challenges.status().unwrap().is_none());
        let after = supervisor.manager.inspect_recovery().unwrap();
        assert_eq!(after.generation, generation);
        assert!(after.completion.is_none());
        assert_eq!(after.confirmed_slot_id, confirmed.slot_id);
        assert!(
            supervisor
                .cleanup_native_completion(completion.completion_generation, &completion)
                .is_err()
        );
    }

    #[test]
    fn prepared_receipt_precedes_quiescence_and_stale_admission_is_silent() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let generation = supervisor.status().unwrap().generation;
        assert!(
            supervisor
                .activate_if_generation_with_admission(
                    &candidate.slot_id,
                    generation + 1,
                    |_| panic!("stale request must not receive admission"),
                    || panic!("stale request must not quiesce"),
                )
                .is_err()
        );
        let receipt = std::cell::RefCell::new(None);
        let activation = supervisor
            .activate_if_generation_with_admission(
                &candidate.slot_id,
                generation,
                |admission| {
                    let status = supervisor.manager.status()?;
                    assert_eq!(status.active_slot_id, previous.slot_id);
                    assert_eq!(
                        status.pending.unwrap().phase,
                        bli_kvm_update_manager::PendingPhaseStatus::Prepared
                    );
                    assert_eq!(admission.prepared_generation, status.generation);
                    assert_eq!(admission.target_slot_id, candidate.slot_id);
                    *receipt.borrow_mut() = Some(admission.clone());
                    Ok(())
                },
                || {
                    assert!(receipt.borrow().is_some());
                    assert_eq!(
                        supervisor.manager.status()?.active_slot_id,
                        previous.slot_id
                    );
                    Ok(())
                },
            )
            .unwrap();
        let receipt = receipt.into_inner().unwrap();
        assert!(
            matches!(activation.outcome, ActivationOutcome::AwaitingHealth {
            operation_id, deadline_monotonic_ms, ..
        } if operation_id == receipt.operation_id && deadline_monotonic_ms == receipt.deadline_monotonic_ms)
        );
    }

    #[test]
    fn signed_legacy_payload_without_boardd_cannot_receive_native_admission() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let mut legacy = payload(2);
        legacy.remove("libexec/bli-kvm-boardd");
        let candidate = fixture.stage_payload(2, legacy);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let before = supervisor.status().unwrap();
        assert!(matches!(
            supervisor.activate_if_generation_with_admission(
                &candidate.slot_id,
                before.generation,
                |_| panic!("incomplete payload must not receive admission"),
                || panic!("incomplete payload must not quiesce"),
            ),
            Err(UpdateRuntimeError::IncompleteNativePayload)
        ));
        assert_eq!(supervisor.status().unwrap(), before);
        assert!(supervisor.challenges.status().unwrap().is_none());
    }

    #[test]
    fn native_candidate_pipeline_orders_phases_and_never_confirms_early() {
        use crate::{
            composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
            native_identity::{NativeIdentityController, NativeStartController, NativeStartUnit},
            process_identity::ProcessIdentity,
        };
        use std::{cell::RefCell, os::unix::fs::MetadataExt, rc::Rc};
        // Every controller call is one helper request in production.
        thread_local! {
            static REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }
        struct Controller {
            fault: &'static str,
            events: Rc<RefCell<Vec<&'static str>>>,
            identity_observations: std::cell::Cell<usize>,
            preflight: Rc<RefCell<Vec<NativeStartUnit>>>,
        }
        impl ServiceController for Controller {
            fn stop(&mut self, _: ApplicationUnit) -> Result<(), CompositionError> {
                REQUESTS.with(|count| count.set(count.get() + 1));
                assert!(matches!(
                    self.events.borrow().first(),
                    Some(&"admission" | &"rollback-intent" | &"completed")
                ));
                self.events.borrow_mut().push("stop");
                if self.fault == "stop" {
                    Err(CompositionError::Controller)
                } else {
                    Ok(())
                }
            }
            fn observe_stop(
                &mut self,
                _: ApplicationUnit,
            ) -> Result<StopObservation, CompositionError> {
                REQUESTS.with(|count| count.set(count.get() + 1));
                Ok(StopObservation::Clean)
            }
        }
        impl NativeStartController for Controller {
            fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError> {
                REQUESTS.with(|count| count.set(count.get() + 1));
                self.preflight.borrow_mut().push(unit);
                if self.fault == "preflight" && matches!(unit, NativeStartUnit::Server(_)) {
                    Err(CompositionError::IncompatibleStartup(unit.unit()))
                } else {
                    Ok(())
                }
            }
            fn start(&mut self, _: NativeStartUnit) -> Result<(), CompositionError> {
                REQUESTS.with(|count| count.set(count.get() + 1));
                assert_eq!(
                    self.events
                        .borrow()
                        .iter()
                        .filter(|event| **event == "stop")
                        .count(),
                    7
                );
                self.events.borrow_mut().push("start");
                if self.fault == "start" {
                    Err(CompositionError::Controller)
                } else {
                    Ok(())
                }
            }
        }
        impl NativeIdentityController for Controller {
            fn observe_identity(
                &self,
                _: ApplicationUnit,
                file: &std::fs::File,
            ) -> Result<ProcessIdentity, CompositionError> {
                REQUESTS.with(|count| count.set(count.get() + 1));
                let meta = file.metadata().unwrap();
                let count = self.identity_observations.get() + 1;
                self.identity_observations.set(count);
                Ok(ProcessIdentity {
                    pid: 42,
                    start_ticks: if self.fault == "replacement" && count > 8 {
                        2
                    } else {
                        1
                    },
                    executable_device: meta.dev(),
                    executable_inode: meta.ino(),
                })
            }
        }
        for fault in [
            "none",
            "confirm-refusal",
            "confirm-replacement",
            "deliver",
            "stop",
            "start",
            "probe",
            "stale",
            "budget",
            "replacement",
            "preflight",
        ] {
            let fixture = Fixture::new();
            REQUESTS.with(|count| count.set(0));
            let previous = fixture.stage(1);
            let candidate = fixture.stage(2);
            let supervisor = fixture.supervisor();
            supervisor.initialize(&previous.slot_id).unwrap();
            let generation = supervisor.status().unwrap().generation;
            let generation = supervisor.migrate_native_idle(generation).unwrap();
            let events = Rc::new(RefCell::new(Vec::new()));
            let preflight = Rc::new(RefCell::new(Vec::new()));
            let mut controller = Controller {
                fault,
                events: events.clone(),
                identity_observations: std::cell::Cell::new(0),
                preflight: preflight.clone(),
            };
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(if fault == "budget" { 3600 } else { 5 });
            let result = supervisor.activate_native_candidate(
                &candidate.slot_id,
                generation + u64::from(fault == "stale"),
                &mut controller,
                deadline,
                |_| {
                    assert_eq!(preflight.borrow().len(), 4);
                    assert_eq!(
                        supervisor.native_status().unwrap().active_slot_id,
                        previous.slot_id
                    );
                    assert_eq!(
                        supervisor.native_status().unwrap().pending.unwrap().phase,
                        bli_kvm_update_manager::PendingPhaseStatus::Prepared
                    );
                    if fault == "deliver" {
                        return Err(UpdateManagerError::RecoveryRequired);
                    }
                    events.borrow_mut().push("admission");
                    Ok(())
                },
                |_, _, supplied_deadline| {
                    assert_eq!(supplied_deadline, deadline);
                    assert_eq!(
                        events
                            .borrow()
                            .iter()
                            .filter(|event| **event == "start")
                            .count(),
                        4
                    );
                    assert_eq!(
                        supervisor.native_status().unwrap().active_slot_id,
                        candidate.slot_id
                    );
                    events.borrow_mut().push("probe");
                    REQUESTS.with(|count| count.set(count.get() + 1));
                    if fault == "probe" {
                        Err(CompositionError::Controller)
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(
                result.is_ok(),
                matches!(fault, "none" | "confirm-refusal" | "confirm-replacement"),
                "{fault}: {result:?}"
            );
            let status = supervisor.native_status().unwrap();
            assert!(
                supervisor
                    .challenges
                    .confirmed_apply_request()
                    .unwrap()
                    .is_none()
            );
            if matches!(fault, "stale" | "preflight") {
                assert_eq!(status.generation, generation);
                assert!(status.pending.is_none());
                assert!(supervisor.challenges.status().unwrap().is_none());
            }
            if fault == "stale" {
                assert!(preflight.borrow().is_empty());
            } else {
                assert_eq!(preflight.borrow().len(), 4);
            }
            assert_eq!(status.floor_sequence, 1);
            assert_eq!(status.confirmed_slot_id, previous.slot_id);
            if matches!(
                fault,
                "none"
                    | "confirm-refusal"
                    | "confirm-replacement"
                    | "start"
                    | "probe"
                    | "replacement"
            ) {
                assert_eq!(status.active_slot_id, candidate.slot_id);
                assert_eq!(
                    status.pending.unwrap().phase,
                    bli_kvm_update_manager::PendingPhaseStatus::AwaitingHealth
                );
                assert!(supervisor.challenges.status().unwrap().is_some());
            } else {
                assert_eq!(status.active_slot_id, previous.slot_id);
            }
            if matches!(fault, "stale" | "deliver" | "budget" | "preflight") {
                assert!(events.borrow().is_empty());
            }
            if matches!(fault, "none" | "confirm-refusal" | "confirm-replacement") {
                let (activation, identity) = result.unwrap();
                assert!(matches!(
                    activation.outcome,
                    ActivationOutcome::AwaitingHealth { .. }
                ));
                assert_eq!(identity.slot_id, candidate.slot_id);
                assert_eq!(
                    events
                        .borrow()
                        .iter()
                        .filter(|event| **event == "probe")
                        .count(),
                    4
                );
                let ActivationOutcome::AwaitingHealth { operation_id, .. } = activation.outcome
                else {
                    panic!("native activation must await confirmation");
                };
                if fault == "confirm-replacement" {
                    controller.fault = "replacement";
                    controller.identity_observations.set(8);
                }
                let outcome = supervisor
                    .confirm_native_candidate(
                        supervisor.native_status().unwrap().generation,
                        operation_id,
                        &identity,
                        &mut controller,
                        deadline,
                        |_, _, _| {
                            REQUESTS.with(|count| count.set(count.get() + 1));
                            if fault == "confirm-refusal" {
                                Err(CompositionError::Controller)
                            } else {
                                Ok(())
                            }
                        },
                    )
                    .unwrap();
                assert!(
                    supervisor
                        .challenges
                        .confirmed_apply_request()
                        .unwrap()
                        .is_none()
                );
                if fault == "none" {
                    assert_eq!(
                        outcome,
                        bli_kvm_update_manager::NativeConfirmationOutcome::Confirmed
                    );
                    // Prepare and select are two further helper requests. The
                    // whole attempt must fit one helper session (64 did not).
                    let requests = REQUESTS.with(std::cell::Cell::get) + 2;
                    assert!(requests > 64, "{requests}");
                    assert!(
                        requests
                            <= usize::from(
                                bli_kvm_update_protocol::execution::MAX_EXECUTION_SESSION_REQUESTS
                            ),
                        "{requests}"
                    );
                    let confirmed = supervisor.native_status().unwrap();
                    assert_eq!(confirmed.floor_sequence, 2);
                    assert!(supervisor.challenges.status().unwrap().is_none());
                    drop(supervisor);
                    let reopened = fixture.supervisor();
                    reopened
                        .cleanup_native_confirmation(
                            confirmed.generation,
                            operation_id,
                            &identity.slot_id,
                        )
                        .unwrap();
                    assert!(
                        reopened
                            .cleanup_native_confirmation(
                                confirmed.generation - 1,
                                operation_id,
                                &identity.slot_id
                            )
                            .is_err()
                    );
                    assert_eq!(reopened.native_status().unwrap(), confirmed);
                } else {
                    assert!(matches!(
                        outcome,
                        bli_kvm_update_manager::NativeConfirmationOutcome::RecoveryPending { .. }
                    ));
                    let recovery = supervisor.manager.inspect_recovery().unwrap();
                    assert!(recovery.rollback_intent.is_some());
                    assert!(recovery.pending.is_some());
                    assert_eq!(recovery.confirmed_slot_id, previous.slot_id);
                    assert!(supervisor.challenges.status().unwrap().is_some());
                }
            }
        }
        for fault in [
            "none",
            "stale",
            "preflight",
            "stop",
            "start",
            "probe",
            "replacement",
            "budget",
            "cleanup",
            "legacy-apply",
        ] {
            let fixture = Fixture::new();
            let previous = fixture.stage(1);
            let candidate = fixture.stage(2);
            let supervisor = fixture.supervisor();
            supervisor.initialize(&previous.slot_id).unwrap();
            let clock = LinuxKernelEvidence::new().snapshot().unwrap();
            let issued = supervisor
                .challenges
                .issue(&candidate.slot_id, clock, clock.monotonic_ms + 30_000)
                .unwrap();
            supervisor
                .manager
                .activate(ActivationRequest {
                    operation_id: issued.operation_id,
                    target_slot_id: candidate.slot_id,
                    health_binding: issued.health_binding,
                    clock,
                })
                .unwrap();
            let intent = supervisor
                .manager
                .begin_native_rejection(
                    supervisor.status().unwrap().generation,
                    issued.operation_id,
                )
                .unwrap();
            let before = supervisor.manager.inspect_recovery().unwrap();
            if fault == "legacy-apply" {
                supervisor
                    .challenges
                    .request_confirmed_apply(before.generation, &previous.slot_id)
                    .unwrap();
            }
            let events = Rc::new(RefCell::new(vec!["rollback-intent"]));
            let preflight = Rc::new(RefCell::new(Vec::new()));
            let mut controller = Controller {
                fault,
                events: events.clone(),
                preflight: preflight.clone(),
                identity_observations: std::cell::Cell::new(0),
            };
            let deadline = std::time::Instant::now()
                + std::time::Duration::from_secs(if fault == "budget" { 60 } else { 5 });
            let result = supervisor.restore_native_composition(
                before.generation + u64::from(fault == "stale"),
                &intent,
                &mut controller,
                deadline,
                |unit, _, supplied_deadline| {
                    assert_eq!(supplied_deadline, deadline);
                    assert_ne!(unit, ApplicationUnit::CandidateServer);
                    assert_eq!(
                        events
                            .borrow()
                            .iter()
                            .filter(|event| **event == "start")
                            .count(),
                        4
                    );
                    let state = supervisor.manager.inspect_recovery().unwrap();
                    assert_eq!(
                        state.selection,
                        bli_kvm_update_manager::RecoverySelection::Confirmed
                    );
                    assert!(state.pending.is_some());
                    events.borrow_mut().push("probe");
                    if fault == "cleanup" && unit == ApplicationUnit::CurrentServer {
                        // A late, noncooperating legacy writer must still be
                        // rejected during cleanup after readiness acknowledgement.
                        supervisor
                            .challenges
                            .request_confirmed_apply(state.generation, &previous.slot_id)
                            .unwrap();
                    }
                    if fault == "probe" {
                        Err(CompositionError::Controller)
                    } else {
                        Ok(())
                    }
                },
            );
            assert_eq!(
                result.is_ok(),
                fault == "none",
                "restore {fault}: {result:?}"
            );
            let after = supervisor.manager.inspect_recovery().unwrap();
            assert_eq!(after.pending.is_none(), matches!(fault, "none" | "cleanup"));
            if matches!(
                fault,
                "stale" | "budget" | "preflight" | "stop" | "legacy-apply"
            ) {
                assert_eq!(after, before);
            } else if !matches!(fault, "none" | "cleanup") {
                assert_eq!(
                    after.rollback_intent.as_ref().unwrap().phase,
                    bli_kvm_update_manager::NativeRollbackPhase::Restoring
                );
                assert_eq!(after.confirmed_slot_id, previous.slot_id);
            }
            if matches!(fault, "stale" | "budget" | "legacy-apply") {
                assert!(preflight.borrow().is_empty());
            }
            if matches!(fault, "stale" | "budget" | "preflight" | "legacy-apply") {
                assert_eq!(*events.borrow(), ["rollback-intent"]);
            }
            if let Ok((restored, ready)) = result {
                assert_eq!(restored, after);
                assert_eq!(ready.slot_id, previous.slot_id);
                assert_eq!(
                    events
                        .borrow()
                        .iter()
                        .filter(|event| **event == "probe")
                        .count(),
                    4
                );
            }
            if matches!(fault, "none" | "cleanup") {
                let receipt = after.completion.clone().unwrap();
                assert_eq!(receipt.health_binding, encode_hex(&issued.health_binding));
                assert!(
                    supervisor
                        .cleanup_native_completion(after.generation - 1, &receipt)
                        .is_err()
                );
                let mut substituted = receipt.clone();
                substituted.health_binding = "f".repeat(64);
                assert!(
                    supervisor
                        .cleanup_native_completion(after.generation, &substituted)
                        .is_err()
                );
                if fault == "cleanup" {
                    assert!(supervisor.challenges.status().unwrap().is_some());
                    let request = supervisor
                        .challenges
                        .confirmed_apply_request()
                        .unwrap()
                        .unwrap();
                    assert!(
                        supervisor
                            .cleanup_native_completion(after.generation, &receipt)
                            .is_err()
                    );
                    assert_eq!(
                        supervisor.challenges.confirmed_apply_request().unwrap(),
                        Some(request.clone())
                    );
                    supervisor
                        .challenges
                        .acknowledge_confirmed_apply(&request)
                        .unwrap();
                    // Simulate interruption after token unlink but before state
                    // unlink; reopened cleanup must retain the same receipt fence.
                    fs::remove_file(&issued.credential_path).unwrap();
                }
                drop(supervisor);
                let reopened = fixture.supervisor();
                let event_count = events.borrow().len();
                reopened
                    .cleanup_native_completion(after.generation, &receipt)
                    .unwrap();
                reopened
                    .cleanup_native_completion(after.generation, &receipt)
                    .unwrap();
                assert!(reopened.challenges.status().unwrap().is_none());
                assert_eq!(events.borrow().len(), event_count);
                assert_eq!(reopened.manager.inspect_recovery().unwrap(), after);
                if fault == "none" {
                    for boot_fault in [
                        "none",
                        "stale",
                        "budget",
                        "preflight",
                        "stop",
                        "start",
                        "probe",
                        "replacement",
                    ] {
                        let boot_events = Rc::new(RefCell::new(vec!["completed"]));
                        let boot_preflight = Rc::new(RefCell::new(Vec::new()));
                        let mut boot_controller = Controller {
                            fault: boot_fault,
                            events: boot_events.clone(),
                            preflight: boot_preflight.clone(),
                            identity_observations: std::cell::Cell::new(0),
                        };
                        let boot_deadline = std::time::Instant::now()
                            + std::time::Duration::from_secs(if boot_fault == "budget" {
                                60
                            } else {
                                5
                            });
                        let result = reopened.start_completed_composition(
                            after.generation + u64::from(boot_fault == "stale"),
                            &receipt,
                            &mut boot_controller,
                            boot_deadline,
                            |unit, _, supplied_deadline| {
                                assert_ne!(unit, ApplicationUnit::CandidateServer);
                                assert_eq!(supplied_deadline, boot_deadline);
                                assert_eq!(
                                    boot_events
                                        .borrow()
                                        .iter()
                                        .filter(|event| **event == "start")
                                        .count(),
                                    4
                                );
                                boot_events.borrow_mut().push("probe");
                                if boot_fault == "probe" {
                                    Err(CompositionError::Controller)
                                } else {
                                    Ok(())
                                }
                            },
                        );
                        assert_eq!(
                            result.is_ok(),
                            boot_fault == "none",
                            "boot {boot_fault}: {result:?}"
                        );
                        assert_eq!(reopened.manager.inspect_recovery().unwrap(), after);
                        if matches!(boot_fault, "stale" | "budget" | "preflight") {
                            assert_eq!(*boot_events.borrow(), ["completed"]);
                        }
                        if boot_fault == "none" {
                            assert_eq!(result.unwrap().slot_id, previous.slot_id);
                            assert_eq!(
                                boot_events
                                    .borrow()
                                    .iter()
                                    .filter(|event| **event == "probe")
                                    .count(),
                                4
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn idle_boot_requires_native_idle_and_fresh_readiness_without_journal_writes() {
        use crate::{
            composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
            native_identity::{NativeIdentityController, NativeStartController, NativeStartUnit},
            process_identity::ProcessIdentity,
        };
        use std::os::unix::fs::MetadataExt;
        #[derive(Default)]
        struct Controller {
            stopped: usize,
            started: Vec<ApplicationUnit>,
        }
        impl ServiceController for Controller {
            fn stop(&mut self, _: ApplicationUnit) -> Result<(), CompositionError> {
                self.stopped += 1;
                Ok(())
            }
            fn observe_stop(
                &mut self,
                _: ApplicationUnit,
            ) -> Result<StopObservation, CompositionError> {
                Ok(StopObservation::Clean)
            }
        }
        impl NativeIdentityController for Controller {
            fn observe_identity(
                &self,
                unit: ApplicationUnit,
                file: &std::fs::File,
            ) -> Result<ProcessIdentity, CompositionError> {
                assert!(self.started.contains(&unit));
                let metadata = file.metadata().unwrap();
                Ok(ProcessIdentity {
                    pid: 42,
                    start_ticks: 1,
                    executable_device: metadata.dev(),
                    executable_inode: metadata.ino(),
                })
            }
        }
        impl NativeStartController for Controller {
            fn validate_start(&self, _: NativeStartUnit) -> Result<(), CompositionError> {
                Ok(())
            }
            fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
                assert_eq!(self.stopped, 7);
                self.started.push(unit.unit());
                Ok(())
            }
        }
        let fixture = Fixture::new();
        let slot = fixture.stage(1);
        let supervisor = fixture.supervisor();
        let initial = supervisor.initialize(&slot.slot_id).unwrap();
        let deadline = || std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut controller = Controller::default();
        assert!(
            supervisor
                .start_idle_composition(
                    initial.generation,
                    &mut controller,
                    deadline(),
                    |_, _, _| Ok(())
                )
                .is_err()
        );
        assert_eq!(controller.stopped, 0);
        supervisor.migrate_native_idle(initial.generation).unwrap();
        let before = supervisor.manager.inspect_recovery().unwrap();
        let journal_path = fixture.state.path().join("application-state.json");
        let journal = std::fs::read(&journal_path).unwrap();
        assert!(
            supervisor
                .start_idle_composition(
                    before.generation - 1,
                    &mut controller,
                    deadline(),
                    |_, _, _| Ok(())
                )
                .is_err()
        );
        assert_eq!(controller.stopped, 0);
        let mut probes = Vec::new();
        let ready = supervisor
            .start_idle_composition(
                before.generation,
                &mut controller,
                deadline(),
                |unit, _, _| {
                    probes.push(unit);
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(ready.slot_id, slot.slot_id);
        assert_eq!(
            controller.started,
            vec![
                ApplicationUnit::Owner,
                ApplicationUnit::VideoProxy,
                ApplicationUnit::BoardDaemon,
                ApplicationUnit::CurrentServer
            ]
        );
        assert_eq!(probes, controller.started);
        let mut failed = Controller::default();
        assert!(
            supervisor
                .start_idle_composition(before.generation, &mut failed, deadline(), |_, _, _| Err(
                    CompositionError::Controller
                ))
                .is_err()
        );
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), before);
        assert_eq!(std::fs::read(&journal_path).unwrap(), journal);
        std::fs::write(fixture.runtime.path().join("health-token"), b"orphan").unwrap();
        let mut refused = Controller::default();
        assert!(
            supervisor
                .start_idle_composition(before.generation, &mut refused, deadline(), |_, _, _| Ok(
                    ()
                ))
                .is_err()
        );
        assert_eq!(refused.stopped, 0);
        assert!(refused.started.is_empty());
        assert_eq!(
            std::fs::read(fixture.runtime.path().join("health-token")).unwrap(),
            b"orphan"
        );

        // Exercise actual boot dispatch, including an interrupted admission and
        // replay of its persisted completion, with the same instrumented backend.
        let fixture = Fixture::new();
        let confirmed = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let initial = supervisor.initialize(&confirmed.slot_id).unwrap();
        let mut legacy = Controller::default();
        assert!(
            supervisor
                .boot_native_composition(initial.generation, &mut legacy, deadline(), |_, _, _| Ok(
                    ()
                ))
                .is_err()
        );
        assert_eq!(legacy.stopped, 0);
        supervisor.migrate_native_idle(initial.generation).unwrap();
        let idle = supervisor.manager.inspect_recovery().unwrap();
        assert!(matches!(
            supervisor
                .boot_native_composition(
                    idle.generation,
                    &mut Controller::default(),
                    deadline(),
                    |_, _, _| Ok(())
                )
                .unwrap(),
            NativeBootOutcome::Ready(_)
        ));
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), idle);
        assert!(
            supervisor
                .activate_native_with_admission(
                    &candidate.slot_id,
                    idle.generation,
                    |_| Err(UpdateManagerError::RecoveryRequired),
                    || panic!("failed delivery must not quiesce")
                )
                .is_err()
        );
        let prepared = supervisor.manager.inspect_recovery().unwrap();
        let token_path = fixture.runtime.path().join("health-token");
        let challenge_path = fixture.runtime.path().join("health-challenge.json");
        let token = std::fs::read(&token_path).unwrap();
        let challenge = std::fs::read(&challenge_path).unwrap();
        let read_journal = || -> serde_json::Value {
            serde_json::from_slice(
                &std::fs::read(fixture.state.path().join("application-state.json")).unwrap(),
            )
            .unwrap()
        };
        let prepared_journal = read_journal();
        assert_eq!(prepared_journal["floorSequence"].as_u64(), Some(1));
        // Expired attempt budgets cannot even persist intent or stop services.
        let mut expired = Controller::default();
        assert!(
            supervisor
                .boot_native_composition(
                    prepared.generation,
                    &mut expired,
                    std::time::Instant::now(),
                    |_, _, _| Ok(())
                )
                .is_err()
        );
        assert_eq!(expired.stopped, 0);
        assert_eq!(read_journal(), prepared_journal);
        supervisor
            .challenges
            .request_confirmed_apply(prepared.generation, &confirmed.slot_id)
            .unwrap();
        let legacy_request = supervisor
            .challenges
            .confirmed_apply_request()
            .unwrap()
            .unwrap();
        let mut conflicting = Controller::default();
        assert!(matches!(
            supervisor.boot_native_composition(
                prepared.generation,
                &mut conflicting,
                deadline(),
                |_, _, _| Ok(())
            ),
            Err(UpdateRuntimeError::InvalidApplyRequest)
        ));
        assert_eq!(conflicting.stopped, 0);
        assert!(conflicting.started.is_empty());
        assert_eq!(read_journal(), prepared_journal);
        assert_eq!(
            supervisor
                .challenges
                .confirmed_apply_request()
                .unwrap()
                .as_ref(),
            Some(&legacy_request)
        );
        assert!(
            supervisor
                .challenges
                .acknowledge_confirmed_apply(&legacy_request)
                .unwrap()
        );
        assert!(
            supervisor
                .boot_native_composition(
                    prepared.generation,
                    &mut Controller::default(),
                    deadline(),
                    |_, _, _| Err(CompositionError::Controller)
                )
                .is_err()
        );
        let restoring = supervisor.manager.inspect_recovery().unwrap();
        let intent = restoring.rollback_intent.as_ref().unwrap();
        assert_eq!(
            intent.phase,
            bli_kvm_update_manager::NativeRollbackPhase::Restoring
        );
        assert_eq!(intent.reason, RollbackReason::InterruptedPrepare);
        assert!(restoring.pending.is_some());
        assert!(restoring.completion.is_none());
        assert_eq!(
            restoring.selection,
            bli_kvm_update_manager::RecoverySelection::Confirmed
        );
        assert_eq!(std::fs::read(&token_path).unwrap(), token);
        assert_eq!(std::fs::read(&challenge_path).unwrap(), challenge);
        assert_eq!(
            read_journal()["floorSequence"],
            prepared_journal["floorSequence"]
        );
        supervisor
            .challenges
            .request_confirmed_apply(restoring.generation, &confirmed.slot_id)
            .unwrap();
        let legacy_request = supervisor
            .challenges
            .confirmed_apply_request()
            .unwrap()
            .unwrap();
        let mut conflicting = Controller::default();
        assert!(matches!(
            supervisor.restore_native_composition(
                restoring.generation,
                intent,
                &mut conflicting,
                deadline(),
                |_, _, _| Ok(())
            ),
            Err(UpdateRuntimeError::InvalidApplyRequest)
        ));
        assert_eq!(conflicting.stopped, 0);
        assert!(conflicting.started.is_empty());
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), restoring);
        assert_eq!(std::fs::read(&token_path).unwrap(), token);
        assert_eq!(std::fs::read(&challenge_path).unwrap(), challenge);
        assert_eq!(
            supervisor
                .challenges
                .confirmed_apply_request()
                .unwrap()
                .as_ref(),
            Some(&legacy_request)
        );
        assert!(
            supervisor
                .challenges
                .acknowledge_confirmed_apply(&legacy_request)
                .unwrap()
        );
        assert!(matches!(
            supervisor
                .boot_native_composition(
                    restoring.generation,
                    &mut Controller::default(),
                    deadline(),
                    |_, _, _| Ok(())
                )
                .unwrap(),
            NativeBootOutcome::Ready(_)
        ));
        let recovered = supervisor.manager.inspect_recovery().unwrap();
        assert!(recovered.pending.is_none());
        assert_eq!(&recovered.completion.as_ref().unwrap().intent, intent);
        assert_eq!(
            read_journal()["floorSequence"],
            prepared_journal["floorSequence"]
        );
        assert_eq!(
            recovered.completion.as_ref().unwrap().intent.reason,
            RollbackReason::InterruptedPrepare
        );
        assert!(!fixture.runtime.path().join("health-token").exists());
        let mut restarted = Controller::default();
        assert!(matches!(
            supervisor
                .boot_native_composition(
                    recovered.generation,
                    &mut restarted,
                    deadline(),
                    |_, _, _| Ok(())
                )
                .unwrap(),
            NativeBootOutcome::Ready(_)
        ));
        assert_eq!(restarted.started.len(), 4);
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), recovered);

        // A same-boot, intact pending activation keeps its deadline and makes no
        // service calls. AwaitingHealth is deliberately distinct from Ready.
        supervisor
            .migrate_native_idle(recovered.generation)
            .unwrap();
        let idle = supervisor.native_status().unwrap();
        supervisor
            .activate_native_with_admission(
                &candidate.slot_id,
                idle.generation,
                |_| Ok(()),
                || Ok(()),
            )
            .unwrap();
        let pending = supervisor.manager.inspect_recovery().unwrap();
        let mut waiting = Controller::default();
        assert_eq!(
            supervisor
                .boot_native_composition(
                    pending.generation,
                    &mut waiting,
                    deadline(),
                    |_, _, _| panic!("live pending is not a readiness probe")
                )
                .unwrap(),
            NativeBootOutcome::AwaitingHealth(pending.pending.clone().unwrap())
        );
        assert_eq!(waiting.stopped, 0);
        assert!(waiting.started.is_empty());
        assert_eq!(supervisor.manager.inspect_recovery().unwrap(), pending);
    }

    #[test]
    fn native_start_requires_quiescence_and_checks_each_started_executable() {
        use crate::{
            composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
            native_identity::{
                NativeIdentityController, NativeSlotIdentity, NativeStartController,
                NativeStartUnit, SlotServer,
            },
            process_identity::ProcessIdentity,
        };
        use std::os::unix::fs::{FileExt, MetadataExt};
        #[derive(Default)]
        struct Controller {
            blocked: bool,
            stop_observations: usize,
            fail_start: Option<ApplicationUnit>,
            fail_identity: Option<ApplicationUnit>,
            started: Vec<ApplicationUnit>,
            observed: std::cell::RefCell<Vec<ApplicationUnit>>,
        }
        impl ServiceController for Controller {
            fn stop(&mut self, _: ApplicationUnit) -> Result<(), CompositionError> {
                panic!("startup must not stop units")
            }
            fn observe_stop(
                &mut self,
                _: ApplicationUnit,
            ) -> Result<StopObservation, CompositionError> {
                self.stop_observations += 1;
                Ok(if self.blocked {
                    StopObservation::RunningOrQueued
                } else {
                    StopObservation::Clean
                })
            }
        }
        impl NativeStartController for Controller {
            fn validate_start(&self, _: NativeStartUnit) -> Result<(), CompositionError> {
                Ok(())
            }
            fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
                assert_eq!(self.stop_observations, 7);
                assert_eq!(self.started, *self.observed.borrow());
                self.started.push(unit.unit());
                if self.fail_start == Some(unit.unit()) {
                    Err(CompositionError::Controller)
                } else {
                    Ok(())
                }
            }
        }
        impl NativeIdentityController for Controller {
            fn observe_identity(
                &self,
                unit: ApplicationUnit,
                file: &std::fs::File,
            ) -> Result<ProcessIdentity, CompositionError> {
                assert!(self.started.contains(&unit));
                if self.fail_identity == Some(unit) {
                    return Err(CompositionError::Controller);
                }
                let expected = match unit {
                    ApplicationUnit::Owner => "control-2",
                    ApplicationUnit::VideoProxy => "video-2",
                    ApplicationUnit::BoardDaemon => "board-2",
                    ApplicationUnit::CandidateServer | ApplicationUnit::CurrentServer => "server-2",
                    _ => panic!("unexpected executable"),
                };
                let mut bytes = vec![0; expected.len()];
                file.read_exact_at(&mut bytes, 0).unwrap();
                assert_eq!(bytes, expected.as_bytes());
                self.observed.borrow_mut().push(unit);
                let meta = file.metadata().unwrap();
                Ok(ProcessIdentity {
                    pid: 42,
                    start_ticks: 1,
                    executable_device: meta.dev(),
                    executable_inode: meta.ino(),
                })
            }
        }
        for server in [SlotServer::Candidate, SlotServer::Current] {
            let fixture = Fixture::new();
            let previous = fixture.stage(1);
            let candidate = fixture.stage(2);
            let next = fixture.stage(3);
            let supervisor = fixture.supervisor();
            let context =
                NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
            let reject_without_service_calls = |selected_server| {
                let mut controller = Controller::default();
                assert!(matches!(
                    context.start_and_observe(
                        &mut controller,
                        selected_server,
                        std::time::Instant::now() + std::time::Duration::from_secs(2),
                    ),
                    Err(CompositionError::SelectionMismatch)
                ));
                assert!(controller.started.is_empty());
                assert_eq!(controller.stop_observations, 0);
            };
            reject_without_service_calls(server);
            if server == SlotServer::Candidate {
                supervisor.initialize(&previous.slot_id).unwrap();
                let previous_context =
                    NativeSlotIdentity::open(&supervisor.manager, &previous.slot_id).unwrap();
                reject_without_service_calls(server);
                supervisor
                    .activate_if_generation_with_admission(
                        &candidate.slot_id,
                        supervisor.status().unwrap().generation,
                        |_| {
                            reject_without_service_calls(SlotServer::Candidate);
                            reject_without_service_calls(SlotServer::Current);
                            let mut controller = Controller::default();
                            assert!(matches!(
                                previous_context.start_and_observe(
                                    &mut controller,
                                    SlotServer::Current,
                                    std::time::Instant::now() + std::time::Duration::from_secs(2),
                                ),
                                Err(CompositionError::SelectionMismatch)
                            ));
                            assert_eq!(controller.stop_observations, 0);
                            Ok(())
                        },
                        || Ok(()),
                    )
                    .unwrap();
                reject_without_service_calls(SlotServer::Current);
                let mut renewed = Controller::default();
                assert!(matches!(
                    context.start_and_observe(
                        &mut renewed,
                        server,
                        std::time::Instant::now() + std::time::Duration::from_secs(3600),
                    ),
                    Err(CompositionError::Deadline)
                ));
                assert_eq!(renewed.stop_observations, 0);
            } else {
                supervisor.initialize(&candidate.slot_id).unwrap();
                reject_without_service_calls(SlotServer::Candidate);
            }
            let units = [
                ApplicationUnit::Owner,
                ApplicationUnit::VideoProxy,
                ApplicationUnit::BoardDaemon,
                server.unit(),
            ];
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut clean = Controller::default();
            context
                .start_and_observe(&mut clean, server, deadline)
                .unwrap();
            assert_eq!(clean.started, units);
            assert_eq!(*clean.observed.borrow(), [units, units].concat());
            let mut blocked = Controller {
                blocked: true,
                ..Default::default()
            };
            assert!(
                context
                    .start_and_observe(&mut blocked, server, deadline)
                    .is_err()
            );
            assert!(blocked.started.is_empty());
            for (index, unit) in units.into_iter().enumerate() {
                for fail_dispatch in [true, false] {
                    let mut failed = Controller {
                        fail_start: fail_dispatch.then_some(unit),
                        fail_identity: (!fail_dispatch).then_some(unit),
                        ..Default::default()
                    };
                    assert!(
                        context
                            .start_and_observe(&mut failed, server, deadline)
                            .is_err()
                    );
                    assert_eq!(failed.started, units[..=index]);
                    assert_eq!(*failed.observed.borrow(), units[..index]);
                }
            }
            let mut expired = Controller::default();
            assert!(matches!(
                context.start_and_observe(&mut expired, server, std::time::Instant::now()),
                Err(CompositionError::Deadline)
            ));
            assert!(expired.started.is_empty());
            if server == SlotServer::Current {
                supervisor
                    .manager
                    .activate(ActivationRequest {
                        operation_id: [44; 16],
                        target_slot_id: next.slot_id.clone(),
                        health_binding: [8; 32],
                        clock: LinuxKernelEvidence::new().snapshot().unwrap(),
                    })
                    .unwrap();
                let intent = supervisor
                    .manager
                    .begin_native_rejection(supervisor.status().unwrap().generation, [44; 16])
                    .unwrap();
                let generation = supervisor.manager.inspect_recovery().unwrap().generation;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut premature = Controller::default();
                assert!(
                    context
                        .start_restored_and_observe(&mut premature, generation, &intent, deadline,)
                        .is_err()
                );
                assert_eq!(premature.stop_observations, 0);
                let restored = supervisor
                    .manager
                    .restore_native_selection(generation, &intent, || Ok(()))
                    .unwrap();
                let restoring = restored.rollback_intent.as_ref().unwrap();
                for (fence, marker, limit) in [
                    (generation, restoring, deadline),
                    (restored.generation, &intent, deadline),
                    (restored.generation, restoring, std::time::Instant::now()),
                    (
                        restored.generation,
                        restoring,
                        std::time::Instant::now() + std::time::Duration::from_secs(60),
                    ),
                ] {
                    let mut rejected = Controller::default();
                    assert!(
                        context
                            .start_restored_and_observe(&mut rejected, fence, marker, limit,)
                            .is_err()
                    );
                    assert_eq!(rejected.stop_observations, 0);
                    assert!(rejected.started.is_empty());
                }
                let wrong_slot =
                    NativeSlotIdentity::open(&supervisor.manager, &next.slot_id).unwrap();
                let mut rejected = Controller::default();
                assert!(
                    wrong_slot
                        .start_restored_and_observe(
                            &mut rejected,
                            restored.generation,
                            restoring,
                            deadline,
                        )
                        .is_err()
                );
                assert_eq!(rejected.stop_observations, 0);
                let mut blocked = Controller {
                    blocked: true,
                    ..Default::default()
                };
                assert!(
                    context
                        .start_restored_and_observe(
                            &mut blocked,
                            restored.generation,
                            restoring,
                            deadline,
                        )
                        .is_err()
                );
                assert!(blocked.started.is_empty());
                for unit in units {
                    let mut failed = Controller {
                        fail_identity: Some(unit),
                        ..Default::default()
                    };
                    assert!(
                        context
                            .start_restored_and_observe(
                                &mut failed,
                                restored.generation,
                                restoring,
                                deadline,
                            )
                            .is_err()
                    );
                    assert_eq!(supervisor.manager.inspect_recovery().unwrap(), restored);
                }
                let mut clean = Controller::default();
                context
                    .start_restored_and_observe(
                        &mut clean,
                        restored.generation,
                        restoring,
                        deadline,
                    )
                    .unwrap();
                assert_eq!(clean.started, units);
                assert_eq!(supervisor.manager.inspect_recovery().unwrap(), restored);
                assert!(
                    context
                        .start_and_observe(
                            &mut Controller::default(),
                            SlotServer::Current,
                            deadline,
                        )
                        .is_err()
                );
            }
        }
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let mut clock = LinuxKernelEvidence::new().snapshot().unwrap();
        clock.boot_id[0] ^= 1;
        supervisor
            .manager
            .activate(ActivationRequest {
                operation_id: [9; 16],
                target_slot_id: candidate.slot_id.clone(),
                health_binding: [7; 32],
                clock,
            })
            .unwrap();
        let context = NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
        let mut stale_boot = Controller::default();
        assert!(matches!(
            context.start_and_observe(
                &mut stale_boot,
                SlotServer::Candidate,
                std::time::Instant::now() + std::time::Duration::from_secs(2),
            ),
            Err(CompositionError::SelectionMismatch)
        ));
        assert_eq!(stale_boot.stop_observations, 0);
    }

    #[test]
    fn native_context_maps_verified_handles_and_rejects_an_extra_server() {
        use crate::{
            composition::{ApplicationUnit, CompositionError, ServiceController, StopObservation},
            native_identity::{NativeIdentityController, NativeSlotIdentity, SlotServer},
            process_identity::ProcessIdentity,
        };
        use std::os::unix::fs::MetadataExt;
        struct Controller {
            extra: bool,
            stopped: Vec<ApplicationUnit>,
            seen: std::cell::RefCell<Vec<ApplicationUnit>>,
            changed: std::rc::Rc<std::cell::Cell<bool>>,
        }
        impl ServiceController for Controller {
            fn stop(&mut self, _: ApplicationUnit) -> Result<(), CompositionError> {
                panic!("observation must not mutate services")
            }
            fn observe_stop(
                &mut self,
                unit: ApplicationUnit,
            ) -> Result<StopObservation, CompositionError> {
                self.stopped.push(unit);
                Ok(if self.extra {
                    StopObservation::RunningOrQueued
                } else {
                    StopObservation::Clean
                })
            }
        }
        impl NativeIdentityController for Controller {
            fn observe_identity(
                &self,
                unit: ApplicationUnit,
                file: &std::fs::File,
            ) -> Result<ProcessIdentity, CompositionError> {
                use std::os::unix::fs::FileExt;
                let expected = match unit {
                    ApplicationUnit::Owner => "control-2",
                    ApplicationUnit::VideoProxy => "video-2",
                    ApplicationUnit::BoardDaemon => "board-2",
                    ApplicationUnit::CandidateServer | ApplicationUnit::CurrentServer => "server-2",
                    _ => panic!("unexpected native executable"),
                };
                let mut bytes = vec![0; expected.len()];
                file.read_exact_at(&mut bytes, 0).unwrap();
                assert_eq!(bytes, expected.as_bytes());
                let metadata = file.metadata().unwrap();
                self.seen.borrow_mut().push(unit);
                Ok(ProcessIdentity {
                    pid: 42,
                    start_ticks: if self.changed.get() { 2 } else { 1 },
                    executable_device: metadata.dev(),
                    executable_inode: metadata.ino(),
                })
            }
        }
        let fixture = Fixture::new();
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let context = NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
        for server in [SlotServer::Candidate, SlotServer::Current] {
            let mut controller = Controller {
                extra: false,
                stopped: Vec::new(),
                seen: Default::default(),
                changed: Default::default(),
            };
            let observation = context.observe(&mut controller, server).unwrap();
            assert_eq!(observation.slot_id, candidate.slot_id);
            assert_eq!(
                *controller.seen.borrow(),
                [
                    ApplicationUnit::Owner,
                    ApplicationUnit::VideoProxy,
                    ApplicationUnit::BoardDaemon,
                    server.unit()
                ]
            );
            assert_eq!(controller.stopped.len(), 3);
            assert!(!controller.stopped.contains(&server.unit()));
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut probes = Vec::new();
            assert_eq!(
                context
                    .check_readiness(
                        &mut controller,
                        server,
                        deadline,
                        |unit, identity, supplied| {
                            assert_eq!(supplied, deadline);
                            assert_eq!(identity.start_ticks, 1);
                            probes.push(unit);
                            Ok(())
                        }
                    )
                    .unwrap(),
                observation
            );
            assert_eq!(
                probes,
                observation
                    .processes
                    .iter()
                    .map(|(unit, _)| *unit)
                    .collect::<Vec<_>>()
            );
            let mut failed_probes = 0;
            assert!(
                context
                    .check_readiness(&mut controller, server, deadline, |_, _, _| {
                        failed_probes += 1;
                        Err(CompositionError::Controller)
                    })
                    .is_err()
            );
            assert_eq!(failed_probes, 1, "probe failure must not continue or retry");
            let changed = controller.changed.clone();
            assert!(
                context
                    .check_readiness(&mut controller, server, deadline, |_, _, _| {
                        changed.set(true);
                        Ok(())
                    })
                    .is_err(),
                "process replacement during probes must invalidate readiness"
            );
            controller.changed.set(false);
            assert!(matches!(
                context.check_readiness(
                    &mut controller,
                    server,
                    std::time::Instant::now(),
                    |_, _, _| { panic!("expired readiness must not probe") }
                ),
                Err(CompositionError::Deadline)
            ));
            controller.extra = true;
            controller.seen.borrow_mut().clear();
            assert!(context.observe(&mut controller, server).is_err());
            assert!(controller.seen.borrow().is_empty());
        }
    }

    #[test]
    fn served_web_body_must_match_the_signed_slot_path_size_and_digest() {
        use crate::native_identity::NativeSlotIdentity;
        let fixture = Fixture::new();
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let context = NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
        context
            .verify_web_asset("share/web/index.html", b"<html>2</html>")
            .unwrap();
        for body in [
            b"<html>1</html>".as_slice(),
            b"<html>2</html>extra",
            b"",
            b"<html>",
        ] {
            assert!(
                context
                    .verify_web_asset("share/web/index.html", body)
                    .is_err()
            );
        }
        for path in [
            "bin/bli-kvm-server",
            "/share/web/index.html",
            "share/web/../web/index.html",
            "share/web/missing.js",
        ] {
            assert!(
                context.verify_web_asset(path, b"<html>2</html>").is_err(),
                "{path}"
            );
        }
    }

    #[tokio::test]
    async fn web_tree_requires_root_and_every_signed_asset() {
        use crate::native_identity::NativeSlotIdentity;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fixture = Fixture::new();
        let mut files = payload(2);
        files.insert("share/web/assets/app.css", (0o644, b"body{}".to_vec()));
        files.insert(
            "share/web/assets/app.js",
            (0o644, b"console.log(2)".to_vec()),
        );
        let candidate = fixture.stage_payload(2, files);
        let supervisor = fixture.supervisor();
        let context = NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
        for failure in [
            None,
            Some("/"),
            Some("/assets/app.css"),
            Some("/assets/app.js"),
            Some("/index.html"),
        ] {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                tokio::time::timeout(std::time::Duration::from_secs(2), async move {
                    let mut routes = Vec::new();
                    for _ in 0..4 {
                        let (mut socket, _) = listener.accept().await.unwrap();
                        let mut request = Vec::new();
                        while !request.ends_with(b"\r\n\r\n") {
                            let mut byte = [0];
                            socket.read_exact(&mut byte).await.unwrap();
                            request.push(byte[0]);
                            assert!(request.len() <= 4096);
                        }
                        let request = String::from_utf8(request).unwrap();
                        let route = request.split_whitespace().nth(1).unwrap();
                        let mut body = match route {
                            "/" | "/index.html" => b"<html>2</html>".to_vec(),
                            "/assets/app.css" => b"body{}".to_vec(),
                            "/assets/app.js" => b"console.log(2)".to_vec(),
                            _ => panic!("unexpected route: {route}"),
                        };
                        let corrupt = failure == Some(route);
                        if corrupt { body[0] ^= 1; }
                        let mime = match route {
                            "/assets/app.css" => "text/css",
                            "/assets/app.js" => "text/javascript",
                            _ => "text/html",
                        };
                        socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes()).await.unwrap();
                        socket.write_all(&body).await.unwrap();
                        routes.push(route.to_owned());
                        if corrupt { break; }
                    }
                    routes
                }).await.unwrap()
            });
            let result = crate::web_probe::verify_tree(
                context.manifest(),
                address,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await;
            let routes = server.await.unwrap();
            if let Some(path) = failure {
                assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::InvalidData);
                assert_eq!(routes.last().unwrap(), path);
            } else {
                let observation = result.unwrap();
                assert_eq!(observation.manifest_assets, 3);
                assert_eq!(observation.manifest_bytes, 14 + 6 + 14);
                assert_eq!(
                    routes,
                    ["/", "/assets/app.css", "/assets/app.js", "/index.html"]
                );
            }
        }
    }

    #[tokio::test]
    async fn fetched_web_asset_is_bound_to_the_signed_candidate() {
        use crate::native_identity::NativeSlotIdentity;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let fixture = Fixture::new();
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let context = NativeSlotIdentity::open(&supervisor.manager, &candidate.slot_id).unwrap();
        for version in [1, 2] {
            let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).await.unwrap();
                    request.push(byte[0]);
                    assert!(request.len() <= 4096);
                }
                assert!(request.starts_with(b"GET /index.html HTTP/1.1\r\n"));
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 14\r\nConnection: close\r\n\r\n<html>{version}</html>").as_bytes()).await.unwrap();
            });
            let result = crate::web_probe::verify_asset(
                context.manifest(),
                address,
                "share/web/index.html",
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .await;
            assert_eq!(result.is_ok(), version == 2, "{result:?}");
            server.await.unwrap();
        }
    }

    #[test]
    fn failed_admission_delivery_preserves_recovery_without_quiescence() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let generation = supervisor.status().unwrap().generation;
        assert!(
            supervisor
                .activate_if_generation_with_admission(
                    &candidate.slot_id,
                    generation,
                    |_| Err(UpdateManagerError::Io(
                        std::io::ErrorKind::BrokenPipe.into()
                    )),
                    || panic!("failed delivery must not stop the server"),
                )
                .is_err()
        );
        assert_eq!(
            supervisor.status().unwrap().active_slot_id,
            previous.slot_id
        );
        assert!(supervisor.challenges.status().unwrap().is_some());
        drop(supervisor);
        let reopened = fixture.supervisor();
        reopened.watchdog_once().unwrap();
        assert!(reopened.status().unwrap().pending.is_none());
        assert_eq!(reopened.status().unwrap().floor_sequence, 1);
        assert_eq!(
            reopened
                .challenges
                .confirmed_apply_request()
                .unwrap()
                .unwrap()
                .slot_id,
            previous.slot_id
        );
    }

    #[test]
    fn generation_fence_precedes_quiescence_and_success_retains_health_gate() {
        let fixture = Fixture::new();
        let previous = fixture.stage(1);
        let candidate = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&previous.slot_id).unwrap();
        let generation = supervisor.status().unwrap().generation;
        assert!(matches!(
            supervisor.activate_if_generation_with_quiescence(
                &candidate.slot_id,
                generation + 1,
                || panic!("stale request quiesced services")
            ),
            Err(UpdateRuntimeError::StaleActivationGeneration)
        ));
        assert!(supervisor.challenges.status().unwrap().is_none());
        let called = std::cell::Cell::new(false);
        let activation = supervisor
            .activate_if_generation_with_quiescence(&candidate.slot_id, generation, || {
                assert_eq!(
                    supervisor.status().unwrap().active_slot_id,
                    previous.slot_id
                );
                called.set(true);
                Ok(())
            })
            .unwrap();
        assert!(called.get());
        assert!(activation.credential_path.unwrap().exists());
        let status = supervisor.status().unwrap();
        assert_eq!(status.active_slot_id, candidate.slot_id);
        assert_eq!(status.confirmed_slot_id, previous.slot_id);
        assert_eq!(status.floor_sequence, 1);
        assert!(status.pending.is_some());
    }

    #[test]
    fn activation_credential_confirms_exactly_once_without_argv_or_environment() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();

        let activation = supervisor.activate(&slot2.slot_id).unwrap();
        assert!(matches!(
            activation.outcome,
            ActivationOutcome::AwaitingHealth { .. }
        ));
        let credential = activation.credential_path.unwrap();
        let token = fs::read(&credential).unwrap();
        assert_eq!(token.len(), 32);

        let mut oversized = token.clone();
        oversized.push(0);
        assert!(matches!(
            supervisor.confirm_from_reader(&mut Cursor::new(oversized)),
            Err(UpdateRuntimeError::InvalidHealthResponseLength)
        ));
        assert!(credential.exists());

        assert_eq!(
            supervisor
                .confirm_from_reader(&mut Cursor::new(token))
                .unwrap(),
            ConfirmationOutcome::Confirmed
        );
        assert!(!credential.exists());
        let status = supervisor.status().unwrap();
        assert_eq!(status.active_slot_id, slot2.slot_id);
        assert_eq!(status.floor_sequence, 2);
    }

    #[test]
    fn missing_credential_forces_manager_rollback_and_exact_cleanup() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let activation = supervisor.activate(&slot2.slot_id).unwrap();
        fs::remove_file(activation.credential_path.unwrap()).unwrap();

        let recovery = supervisor.watchdog_once().unwrap();
        assert!(matches!(
            recovery.manager,
            RecoveryOutcome::RolledBack {
                reason: RollbackReason::HealthChallengeLost,
                ..
            }
        ));
        assert_eq!(
            recovery.challenge,
            ChallengeReconciliation::RolledBackMismatch
        );
        assert_eq!(supervisor.status().unwrap().active_slot_id, slot1.slot_id);
        let request = fs::read(
            fixture
                .runtime
                .path()
                .join(crate::CONFIRMED_APPLY_FILE_NAME),
        )
        .unwrap();
        assert!(
            supervisor
                .acknowledge_confirmed_apply(&mut Cursor::new(request))
                .unwrap()
        );
        assert_eq!(fs::read_dir(fixture.runtime.path()).unwrap().count(), 0);
    }

    #[test]
    fn watchdog_discards_a_complete_challenge_without_pending_manager_state() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let slot2 = fixture.stage(2);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let clock = LinuxKernelEvidence::new().snapshot().unwrap();
        let issued = supervisor
            .challenges
            .issue(
                &slot2.slot_id,
                clock,
                clock.monotonic_ms + DEFAULT_HEALTH_TIMEOUT_MS,
            )
            .unwrap();

        let recovery = supervisor.watchdog_once().unwrap();
        assert_eq!(recovery.manager, RecoveryOutcome::Clean);
        assert_eq!(recovery.challenge, ChallengeReconciliation::DiscardedStale);
        assert!(!issued.credential_path.exists());
        assert_eq!(supervisor.status().unwrap().active_slot_id, slot1.slot_id);
    }

    #[test]
    fn activating_the_current_slot_is_a_credential_free_noop() {
        let fixture = Fixture::new();
        let slot1 = fixture.stage(1);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot1.slot_id).unwrap();
        let activation = supervisor.activate(&slot1.slot_id).unwrap();
        assert_eq!(activation.outcome, ActivationOutcome::AlreadyActive);
        assert_eq!(activation.credential_path, None);
        assert_eq!(fs::read_dir(fixture.runtime.path()).unwrap().count(), 0);
    }

    #[test]
    fn guarded_activation_rejects_stale_current_and_invalid_targets_without_mutation() {
        let fixture = Fixture::new();
        let first = fixture.stage(1);
        let second = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let initial = supervisor.initialize(&first.slot_id).unwrap();
        assert!(matches!(
            supervisor.activate_if_generation(&second.slot_id, initial.generation + 1),
            Err(UpdateRuntimeError::StaleActivationGeneration)
        ));
        assert!(matches!(
            supervisor.activate_if_generation(&first.slot_id, initial.generation),
            Err(UpdateRuntimeError::ActivationTargetAlreadyActive)
        ));
        assert!(
            supervisor
                .activate_if_generation("../untrusted", initial.generation)
                .is_err()
        );
        assert_eq!(supervisor.status().unwrap(), initial);
        assert_eq!(fs::read_dir(fixture.runtime.path()).unwrap().count(), 0);
    }

    #[test]
    fn guarded_repeats_do_not_replace_pending_challenge_or_reactivate_after_confirmation() {
        let fixture = Fixture::new();
        let first = fixture.stage(1);
        let second = fixture.stage(2);
        let supervisor = fixture.supervisor();
        let initial = supervisor.initialize(&first.slot_id).unwrap();
        let activation = supervisor
            .activate_if_generation(&second.slot_id, initial.generation)
            .unwrap();
        assert!(matches!(
            activation.outcome,
            ActivationOutcome::AwaitingHealth { .. }
        ));
        let pending = supervisor.status().unwrap();
        let credential = activation.credential_path.unwrap();
        let token = fs::read(&credential).unwrap();
        assert!(matches!(
            supervisor.activate_if_generation(&second.slot_id, initial.generation),
            Err(UpdateRuntimeError::StaleActivationGeneration)
        ));
        assert!(matches!(
            supervisor.activate_if_generation(&second.slot_id, pending.generation),
            Err(UpdateRuntimeError::Manager(
                UpdateManagerError::PendingActivation
            ))
        ));
        assert_eq!(supervisor.status().unwrap(), pending);
        assert_eq!(fs::read(&credential).unwrap(), token);
        assert_eq!(
            supervisor
                .confirm_from_reader(&mut Cursor::new(token))
                .unwrap(),
            ConfirmationOutcome::Confirmed
        );
        let confirmed = supervisor.status().unwrap();
        assert!(matches!(
            supervisor.activate_if_generation(&second.slot_id, initial.generation),
            Err(UpdateRuntimeError::StaleActivationGeneration)
        ));
        assert!(matches!(
            supervisor.activate_if_generation(&second.slot_id, confirmed.generation),
            Err(UpdateRuntimeError::ActivationTargetAlreadyActive)
        ));
        assert_eq!(supervisor.status().unwrap(), confirmed);
        assert!(!credential.exists());
    }

    #[test]
    fn clean_watchdog_ticks_do_not_request_service_restarts() {
        let fixture = Fixture::new();
        let slot = fixture.stage(1);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&slot.slot_id).unwrap();
        for _ in 0..12 {
            assert_eq!(
                supervisor.watchdog_once().unwrap().manager,
                RecoveryOutcome::Clean
            );
            assert!(
                supervisor
                    .challenges
                    .confirmed_apply_request()
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn stale_ack_cannot_remove_a_newer_confirmed_service_request() {
        let fixture = Fixture::new();
        let first = fixture.stage(1);
        let second = fixture.stage(2);
        let third = fixture.stage(3);
        let supervisor = fixture.supervisor();
        supervisor.initialize(&first.slot_id).unwrap();
        let token = fs::read(
            supervisor
                .activate(&second.slot_id)
                .unwrap()
                .credential_path
                .unwrap(),
        )
        .unwrap();
        supervisor
            .confirm_from_reader(&mut Cursor::new(token))
            .unwrap();
        let path = fixture
            .runtime
            .path()
            .join(crate::CONFIRMED_APPLY_FILE_NAME);
        let previous = fs::read(&path).unwrap();
        supervisor.watchdog_once().unwrap();
        assert_eq!(fs::read(&path).unwrap(), previous);
        let token = fs::read(
            supervisor
                .activate(&third.slot_id)
                .unwrap()
                .credential_path
                .unwrap(),
        )
        .unwrap();
        supervisor
            .confirm_from_reader(&mut Cursor::new(token))
            .unwrap();
        let current = fs::read(&path).unwrap();
        assert_ne!(previous, current);
        assert!(
            !supervisor
                .acknowledge_confirmed_apply(&mut Cursor::new(previous))
                .unwrap()
        );
        assert_eq!(fs::read(&path).unwrap(), current);
        assert!(
            supervisor
                .acknowledge_confirmed_apply(&mut Cursor::new(current))
                .unwrap()
        );
        for _ in 0..12 {
            supervisor.watchdog_once().unwrap();
            assert!(!path.exists());
        }
    }

    fn payload(sequence: u64) -> BTreeMap<&'static str, (u32, Vec<u8>)> {
        BTreeMap::from([
            (
                "bin/bli-kvm-server",
                (0o755, format!("server-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-control-broker",
                (0o755, format!("control-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-platform-broker",
                (0o755, format!("broker-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-video-proxy",
                (0o755, format!("video-{sequence}").into_bytes()),
            ),
            (
                "libexec/bli-kvm-boardd",
                (0o755, format!("board-{sequence}").into_bytes()),
            ),
            (
                "share/web/index.html",
                (0o644, format!("<html>{sequence}</html>").into_bytes()),
            ),
        ])
    }

    fn manifest(sequence: u64, payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> ApplicationManifest {
        ApplicationManifest {
            format: APPLICATION_FORMAT_V1.into(),
            product: APPLICATION_PRODUCT.into(),
            target: APPLICATION_TARGET.into(),
            architecture: APPLICATION_ARCHITECTURE.into(),
            key_id: KEY_ID.into(),
            version: format!("0.{sequence}.0"),
            sequence,
            minimum_system_sequence: 1,
            source_commit: format!("{sequence:040x}"),
            files: payload
                .iter()
                .map(|(path, (mode, bytes))| ApplicationFile {
                    path: (*path).to_string(),
                    mode: *mode,
                    size: bytes.len() as u64,
                    sha256: sha256_hex(bytes),
                })
                .collect(),
        }
    }

    fn header(path: &str, mode: u32, size: u64, entry_type: EntryType) -> Header {
        let mut header = Header::new_ustar();
        header.set_path(path).unwrap();
        header.set_mode(mode);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_size(size);
        header.set_entry_type(entry_type);
        header.set_cksum();
        header
    }

    fn bundle(
        signing_key: &SigningKey,
        manifest: &ApplicationManifest,
        payload: &BTreeMap<&str, (u32, Vec<u8>)>,
    ) -> Vec<u8> {
        let manifest_bytes = serde_json::to_vec(manifest).unwrap();
        let signature = signing_key.sign(&manifest_bytes).to_bytes();
        let mut builder = Builder::new(Vec::new());
        builder.mode(tar::HeaderMode::Deterministic);
        builder
            .append(
                &header(
                    APPLICATION_MANIFEST_PATH,
                    0o644,
                    manifest_bytes.len() as u64,
                    EntryType::Regular,
                ),
                manifest_bytes.as_slice(),
            )
            .unwrap();
        builder
            .append(
                &header(APPLICATION_SIGNATURE_PATH, 0o644, 64, EntryType::Regular),
                signature.as_slice(),
            )
            .unwrap();
        let directories = payload_directories(payload);
        let mut entries = directories.clone();
        entries.extend(payload.keys().map(|path| format!("payload/{path}")));
        for path in entries {
            if directories.contains(&path) {
                builder
                    .append(&header(&path, 0o755, 0, EntryType::Directory), &[][..])
                    .unwrap();
            } else {
                let relative = path.strip_prefix("payload/").unwrap();
                let (mode, bytes) = payload.get(relative).unwrap();
                builder
                    .append(
                        &header(&path, *mode, bytes.len() as u64, EntryType::Regular),
                        bytes.as_slice(),
                    )
                    .unwrap();
            }
        }
        builder.into_inner().unwrap()
    }

    fn payload_directories(payload: &BTreeMap<&str, (u32, Vec<u8>)>) -> BTreeSet<String> {
        let mut directories = BTreeSet::from(["payload".to_string()]);
        for path in payload.keys() {
            let mut current = PathBuf::from("payload");
            let components = Path::new(path).components().collect::<Vec<_>>();
            for component in components.iter().take(components.len() - 1) {
                current.push(component.as_os_str());
                directories.insert(current.to_str().unwrap().to_string());
            }
        }
        directories
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        let mut value = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut value, "{byte:02x}").unwrap();
        }
        value
    }
}
