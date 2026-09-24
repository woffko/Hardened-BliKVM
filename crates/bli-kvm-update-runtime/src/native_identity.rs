//! One held verified slot inventory for native composition identity checks.

use bli_kvm_update_manager::{
    ApplicationUpdateManager, NativeRollbackIntent, NativeRollbackPhase, RecoverySelection,
    VerifiedSlotFiles,
};

use crate::{
    UpdateRuntimeError,
    composition::{
        ApplicationUnit, CompositionError, ServiceController, StopObservation,
        validate_native_payload,
    },
    composition_systemd::SystemdController,
    process_identity::ProcessIdentity,
};

pub(crate) const FILES: [&str; 5] = [
    "libexec/bli-kvm-control-broker",
    "libexec/bli-kvm-video-proxy",
    "libexec/bli-kvm-boardd",
    "bin/bli-kvm-server",
    "share/web/index.html",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotServer {
    Candidate,
    Current,
}

/// Only native slot components may be started by the update coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStartUnit {
    Owner,
    VideoProxy,
    BoardDaemon,
    Server(SlotServer),
}

impl NativeStartUnit {
    pub const fn unit(self) -> ApplicationUnit {
        match self {
            Self::Owner => ApplicationUnit::Owner,
            Self::VideoProxy => ApplicationUnit::VideoProxy,
            Self::BoardDaemon => ApplicationUnit::BoardDaemon,
            Self::Server(server) => server.unit(),
        }
    }
}

pub trait NativeStartController: NativeIdentityController {
    /// Read-only compatibility check before admission; start must check again.
    fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError>;
    /// Bound dispatch by the original transaction deadline. Success is not
    /// readiness; failed dispatch may still have queued a systemd job.
    fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError>;
}

impl NativeStartController for SystemdController {
    fn validate_start(&self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.validate_native_start(unit)
    }
    fn start(&mut self, unit: NativeStartUnit) -> Result<(), CompositionError> {
        self.start_native_unit(unit)
    }
}

impl SlotServer {
    pub const fn unit(self) -> ApplicationUnit {
        match self {
            Self::Candidate => ApplicationUnit::CandidateServer,
            Self::Current => ApplicationUnit::CurrentServer,
        }
    }
}

#[derive(Debug)]
pub struct NativeSlotIdentity<'manager> {
    manager: &'manager ApplicationUpdateManager,
    verified: VerifiedSlotFiles,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeIdentityObservation {
    pub slot_id: String,
    pub processes: [(ApplicationUnit, ProcessIdentity); 4],
}

/// Read-only identity observations share the controller's finite deadline.
/// Implementations must bind the closed unit to the supplied executable handle.
pub trait NativeIdentityController: ServiceController {
    fn observe_identity(
        &self,
        unit: ApplicationUnit,
        expected: &std::fs::File,
    ) -> Result<ProcessIdentity, CompositionError>;
}

impl NativeIdentityController for SystemdController {
    fn observe_identity(
        &self,
        unit: ApplicationUnit,
        expected: &std::fs::File,
    ) -> Result<ProcessIdentity, CompositionError> {
        self.observe_unit_identity(unit, expected)
    }
}

impl<'manager> NativeSlotIdentity<'manager> {
    /// Start only after durable selection and complete quiescence. Check each
    /// executable before dispatching its dependents. Failure retains the caller's
    /// recovery obligation: it must not confirm or blindly restart the old owner.
    /// Effective unit dependencies must belong to the trusted system recipe.
    /// Controller operations must enforce this deadline or an earlier one.
    pub fn start_and_observe(
        &self,
        controller: &mut impl NativeStartController,
        server: SlotServer,
        deadline: std::time::Instant,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        if std::time::Instant::now() >= deadline {
            return Err(CompositionError::Deadline);
        }
        // status() also validates the selected pointer and referenced signed
        // slots under the held manager locks. Prepared is never start authority.
        let status = self
            .manager
            .status()
            .or_else(|error| match error {
                bli_kvm_update_manager::UpdateManagerError::RecoveryRequired => {
                    self.manager.native_status()
                }
                error => Err(error),
            })
            .map_err(|_| CompositionError::SelectionMismatch)?;
        let selected = status.active_slot_id == self.slot_id()
            && match server {
                SlotServer::Candidate => status.pending.as_ref().is_some_and(|pending| {
                    pending.phase == bli_kvm_update_manager::PendingPhaseStatus::AwaitingHealth
                        && pending.target_slot_id == self.slot_id()
                }),
                SlotServer::Current => {
                    status.pending.is_none() && status.confirmed_slot_id == self.slot_id()
                }
            };
        if !selected {
            return Err(CompositionError::SelectionMismatch);
        }
        if let Some(pending) = status.pending.as_ref() {
            let observed_at = std::time::Instant::now();
            let clock = crate::LinuxKernelEvidence::new()
                .snapshot()
                .map_err(|_| CompositionError::SelectionMismatch)?;
            let boot_id: String = clock
                .boot_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if boot_id != pending.boot_id {
                return Err(CompositionError::SelectionMismatch);
            }
            let remaining = pending
                .deadline_monotonic_ms
                .checked_sub(clock.monotonic_ms)
                .filter(|remaining| *remaining > 0)
                .ok_or(CompositionError::Deadline)?;
            let latest = observed_at
                .checked_add(std::time::Duration::from_millis(remaining))
                .ok_or(CompositionError::Deadline)?;
            if deadline > latest {
                return Err(CompositionError::Deadline);
            }
        }
        self.start_selected(controller, server, deadline)
    }

    /// Start the confirmed composition during an exactly fenced restoration.
    /// This uses a separate bounded recovery attempt, not candidate health time.
    /// The manager locks remain held; successful startup is not recovery completion.
    pub fn start_restored_and_observe(
        &self,
        controller: &mut impl NativeStartController,
        expected_generation: u64,
        intent: &NativeRollbackIntent,
        deadline: std::time::Instant,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline);
        }
        let recovery = self
            .manager
            .inspect_recovery()
            .map_err(|_| CompositionError::SelectionMismatch)?;
        if recovery.generation != expected_generation
            || recovery.rollback_intent.as_ref() != Some(intent)
            || intent.phase != NativeRollbackPhase::Restoring
            || recovery.selection != RecoverySelection::Confirmed
            || recovery.confirmed_slot_id != self.slot_id()
        {
            return Err(CompositionError::SelectionMismatch);
        }
        self.start_selected(controller, SlotServer::Current, deadline)
    }

    /// Start a historically completed recovery on a new bounded boot attempt.
    /// Completion does not prove present health; the caller must probe again.
    pub fn start_completed_and_observe(
        &self,
        controller: &mut impl NativeStartController,
        expected_generation: u64,
        completion: &bli_kvm_update_manager::NativeRecoveryCompletion,
        deadline: std::time::Instant,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        let now = std::time::Instant::now();
        if deadline <= now || deadline.duration_since(now) > std::time::Duration::from_secs(45) {
            return Err(CompositionError::Deadline);
        }
        let recovery = self
            .manager
            .inspect_recovery()
            .map_err(|_| CompositionError::SelectionMismatch)?;
        if recovery.generation != expected_generation
            || recovery.completion.as_ref() != Some(completion)
            || recovery.selection != RecoverySelection::Confirmed
            || recovery.confirmed_slot_id != self.slot_id()
        {
            return Err(CompositionError::SelectionMismatch);
        }
        self.start_selected(controller, SlotServer::Current, deadline)
    }

    fn start_selected(
        &self,
        controller: &mut impl NativeStartController,
        server: SlotServer,
        deadline: std::time::Instant,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        let check_deadline = || {
            if std::time::Instant::now() >= deadline {
                Err(CompositionError::Deadline)
            } else {
                Ok(())
            }
        };
        for unit in [
            ApplicationUnit::BoardServer,
            ApplicationUnit::BootstrapServer,
            ApplicationUnit::CandidateServer,
            ApplicationUnit::CurrentServer,
            ApplicationUnit::BoardDaemon,
            ApplicationUnit::VideoProxy,
            ApplicationUnit::Owner,
        ] {
            check_deadline()?;
            if controller.observe_stop(unit)? != StopObservation::Clean {
                return Err(CompositionError::NotStopped(unit));
            }
        }
        for (unit, path) in [
            NativeStartUnit::Owner,
            NativeStartUnit::VideoProxy,
            NativeStartUnit::BoardDaemon,
            NativeStartUnit::Server(server),
        ]
        .into_iter()
        .zip(FILES)
        {
            check_deadline()?;
            let file = self
                .verified
                .files()
                .get(path)
                .ok_or(CompositionError::IncompletePayload)?;
            controller.start(unit)?;
            check_deadline()?;
            controller.observe_identity(unit.unit(), file)?;
        }
        check_deadline()?;
        let observation = self.observe(controller, server)?;
        check_deadline()?;
        Ok(observation)
    }

    /// Check an HTTP-decoded web body against the signed immutable inventory.
    /// See `web_probe::verify_body` for the transport obligations.
    pub fn verify_web_asset(
        &self,
        manifest_path: &str,
        body: &[u8],
    ) -> Result<(), CompositionError> {
        crate::web_probe::verify_body(self.manifest(), manifest_path, body)
    }

    /// The signed manifest of the held slot inventory.
    pub fn manifest(&self) -> &bli_kvm_update_verifier::ApplicationManifest {
        &self.verified.slot().manifest
    }

    /// Fence read-only native readiness probes with the same held slot files and
    /// process identities on both sides. The caller supplies the remaining
    /// original transaction deadline, never a renewed health budget.
    /// The controller must use that deadline or an earlier one; checks here
    /// reject late results but cannot interrupt blocking controller/probe code.
    ///
    /// Each probe must bound all I/O by deadline and bind IPC peer credentials
    /// to the supplied process. Server probing must include authenticated API
    /// and selected web assets; generic health JSON alone is insufficient.
    /// This does not confirm an update or advance its rollback floor.
    pub fn check_readiness(
        &self,
        controller: &mut impl NativeIdentityController,
        server: SlotServer,
        deadline: std::time::Instant,
        mut probe: impl FnMut(
            ApplicationUnit,
            &ProcessIdentity,
            std::time::Instant,
        ) -> Result<(), CompositionError>,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        let check_deadline = || {
            if std::time::Instant::now() >= deadline {
                Err(CompositionError::Deadline)
            } else {
                Ok(())
            }
        };
        check_deadline()?;
        let before = self.observe(controller, server)?;
        for (unit, identity) in &before.processes {
            check_deadline()?;
            probe(*unit, identity, deadline)?;
        }
        check_deadline()?;
        let after = self.observe(controller, server)?;
        check_deadline()?;
        if before != after {
            return Err(CompositionError::Controller);
        }
        Ok(after)
    }

    /// Keep the manager and its locks alive throughout use of this context.
    pub fn open(
        manager: &'manager ApplicationUpdateManager,
        slot_id: &str,
    ) -> Result<Self, UpdateRuntimeError> {
        let verified = manager.open_verified_slot_files(slot_id, &FILES)?;
        validate_native_payload(
            verified
                .slot()
                .manifest
                .files
                .iter()
                .map(|file| (file.path.as_str(), file.mode, file.size)),
        )
        .map_err(|_| UpdateRuntimeError::IncompleteNativePayload)?;
        Ok(Self { manager, verified })
    }

    pub fn slot_id(&self) -> &str {
        &self.verified.slot().slot_id
    }

    /// Collect identity only. Call before and after bounded native IPC/readiness
    /// probes and require equal observations before any health confirmation.
    pub fn observe(
        &self,
        controller: &mut impl NativeIdentityController,
        server: SlotServer,
    ) -> Result<NativeIdentityObservation, CompositionError> {
        for unit in [
            ApplicationUnit::BoardServer,
            ApplicationUnit::BootstrapServer,
            ApplicationUnit::CandidateServer,
            ApplicationUnit::CurrentServer,
        ] {
            if unit != server.unit() && controller.observe_stop(unit)? != StopObservation::Clean {
                return Err(CompositionError::NotStopped(unit));
            }
        }
        let observe = |unit: ApplicationUnit,
                       path: &str|
         -> Result<(ApplicationUnit, ProcessIdentity), CompositionError> {
            let file = self
                .verified
                .files()
                .get(path)
                .ok_or(CompositionError::IncompletePayload)?;
            Ok((unit, controller.observe_identity(unit, file)?))
        };
        Ok(NativeIdentityObservation {
            slot_id: self.slot_id().to_owned(),
            processes: [
                observe(ApplicationUnit::Owner, FILES[0])?,
                observe(ApplicationUnit::VideoProxy, FILES[1])?,
                observe(ApplicationUnit::BoardDaemon, FILES[2])?,
                observe(server.unit(), FILES[3])?,
            ],
        })
    }
}
