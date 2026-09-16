#![no_std]
#![forbid(unsafe_code)]

//! Pure fail-closed policy for operating-system reboot and shutdown actions.
//!
//! The crate models authorization, operational guards, idempotency, dispatch
//! intent, crash ambiguity, post-boot acceptance, and cooldown. It contains no
//! command strings, process spawning, filesystem, clock, network, or device
//! access. Callers supply monotonic time and opaque boot/session bindings.
//!
//! A returned state snapshot is only data to be persisted by a future adapter;
//! it is not evidence that persistence happened. The only public eligibility
//! constructor is disabled, so no production caller can currently obtain a
//! dispatch intent from this crate.

pub const STATE_SCHEMA_VERSION: u16 = 1;
pub const MAX_OPERATION_RECORDS: usize = 16;
pub const MAX_CONFIRMATION_AGE_MS: u64 = 10_000;
pub const MIN_ACTION_BOOT_UPTIME_MS: u64 = 60_000;
pub const MIN_POST_BOOT_ACCEPTANCE_UPTIME_MS: u64 = 30_000;
pub const ACTION_COOLDOWN_MS: u64 = 60_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemAction {
    Reboot,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionRequest {
    operation_id: [u8; 16],
    session_binding: u64,
    confirmation_binding: [u8; 32],
    confirmation_issued_at_ms: u64,
    origin_boot_binding: [u8; 16],
    action: SystemAction,
}

impl SystemActionRequest {
    pub fn new(
        operation_id: [u8; 16],
        session_binding: u64,
        confirmation_binding: [u8; 32],
        confirmation_issued_at_ms: u64,
        origin_boot_binding: [u8; 16],
        action: SystemAction,
    ) -> Result<Self, SystemActionPolicyError> {
        if is_zero(&operation_id)
            || session_binding == 0
            || is_zero(&confirmation_binding)
            || confirmation_issued_at_ms == 0
            || is_zero(&origin_boot_binding)
        {
            return Err(SystemActionPolicyError::InvalidRequest);
        }
        Ok(Self {
            operation_id,
            session_binding,
            confirmation_binding,
            confirmation_issued_at_ms,
            origin_boot_binding,
            action,
        })
    }

    pub const fn operation_id(self) -> [u8; 16] {
        self.operation_id
    }

    pub const fn action(self) -> SystemAction {
        self.action
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GuardState {
    Ready,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionEnvironment {
    current_boot_binding: [u8; 16],
    monotonic_ms: u64,
    update: GuardState,
    storage: GuardState,
    recovery: GuardState,
    post_boot_observer: GuardState,
}

impl SystemActionEnvironment {
    pub fn new(
        current_boot_binding: [u8; 16],
        monotonic_ms: u64,
        update: GuardState,
        storage: GuardState,
        recovery: GuardState,
        post_boot_observer: GuardState,
    ) -> Result<Self, SystemActionPolicyError> {
        if is_zero(&current_boot_binding) {
            return Err(SystemActionPolicyError::InvalidEnvironment);
        }
        Ok(Self {
            current_boot_binding,
            monotonic_ms,
            update,
            storage,
            recovery,
            post_boot_observer,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostBootObservation {
    current_boot_binding: [u8; 16],
    monotonic_ms: u64,
    update: GuardState,
    storage: GuardState,
    recovery: GuardState,
    health: GuardState,
}

impl PostBootObservation {
    pub fn new(
        current_boot_binding: [u8; 16],
        monotonic_ms: u64,
        update: GuardState,
        storage: GuardState,
        recovery: GuardState,
        health: GuardState,
    ) -> Result<Self, SystemActionPolicyError> {
        if is_zero(&current_boot_binding) {
            return Err(SystemActionPolicyError::InvalidEnvironment);
        }
        Ok(Self {
            current_boot_binding,
            monotonic_ms,
            update,
            storage,
            recovery,
            health,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionEligibility {
    capability_enabled: bool,
    backend_ready: bool,
    local_admin_verified: bool,
    reauthentication_verified: bool,
    confirmation_verified: bool,
}

impl SystemActionEligibility {
    pub const fn disabled() -> Self {
        Self {
            capability_enabled: false,
            backend_ready: false,
            local_admin_verified: false,
            reauthentication_verified: false,
            confirmation_verified: false,
        }
    }

    /// Production constructor: every flag comes from a reviewed check, never
    /// from a default. The profile that arms the capability supplies them.
    pub const fn reviewed(
        capability_enabled: bool,
        backend_ready: bool,
        local_admin_verified: bool,
        reauthentication_verified: bool,
        confirmation_verified: bool,
    ) -> Self {
        Self {
            capability_enabled,
            backend_ready,
            local_admin_verified,
            reauthentication_verified,
            confirmation_verified,
        }
    }

    #[cfg(test)]
    const fn verified_for_test() -> Self {
        Self {
            capability_enabled: true,
            backend_ready: true,
            local_admin_verified: true,
            reauthentication_verified: true,
            confirmation_verified: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemActionPhase {
    Prepared,
    Dispatching,
    PostBootAccepted,
    Rejected,
}

impl SystemActionPhase {
    const fn is_pending(self) -> bool {
        matches!(self, Self::Prepared | Self::Dispatching)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OperationFingerprint {
    session_binding: u64,
    confirmation_binding: [u8; 32],
    confirmation_issued_at_ms: u64,
    origin_boot_binding: [u8; 16],
    action: SystemAction,
}

impl From<SystemActionRequest> for OperationFingerprint {
    fn from(request: SystemActionRequest) -> Self {
        Self {
            session_binding: request.session_binding,
            confirmation_binding: request.confirmation_binding,
            confirmation_issued_at_ms: request.confirmation_issued_at_ms,
            origin_boot_binding: request.origin_boot_binding,
            action: request.action,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionRecord {
    operation_id: [u8; 16],
    fingerprint: OperationFingerprint,
    phase: SystemActionPhase,
    prepared_at_ms: u64,
    dispatch_started_at_ms: Option<u64>,
    terminal_at_ms: Option<u64>,
    terminal_boot_binding: Option<[u8; 16]>,
}

impl SystemActionRecord {
    pub const fn operation_id(self) -> [u8; 16] {
        self.operation_id
    }

    pub const fn action(self) -> SystemAction {
        self.fingerprint.action
    }

    pub const fn origin_boot_binding(self) -> [u8; 16] {
        self.fingerprint.origin_boot_binding
    }

    pub const fn phase(self) -> SystemActionPhase {
        self.phase
    }

    pub const fn prepared_at_ms(self) -> u64 {
        self.prepared_at_ms
    }

    pub const fn dispatch_started_at_ms(self) -> Option<u64> {
        self.dispatch_started_at_ms
    }

    pub const fn terminal_at_ms(self) -> Option<u64> {
        self.terminal_at_ms
    }

    pub const fn terminal_boot_binding(self) -> Option<[u8; 16]> {
        self.terminal_boot_binding
    }

    pub const fn snapshot(self) -> SystemActionRecordSnapshot {
        SystemActionRecordSnapshot {
            operation_id: self.operation_id,
            session_binding: self.fingerprint.session_binding,
            confirmation_binding: self.fingerprint.confirmation_binding,
            confirmation_issued_at_ms: self.fingerprint.confirmation_issued_at_ms,
            origin_boot_binding: self.fingerprint.origin_boot_binding,
            action: self.fingerprint.action,
            phase: self.phase,
            prepared_at_ms: self.prepared_at_ms,
            dispatch_started_at_ms: self.dispatch_started_at_ms,
            terminal_at_ms: self.terminal_at_ms,
            terminal_boot_binding: self.terminal_boot_binding,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionRecordSnapshot {
    pub operation_id: [u8; 16],
    pub session_binding: u64,
    pub confirmation_binding: [u8; 32],
    pub confirmation_issued_at_ms: u64,
    pub origin_boot_binding: [u8; 16],
    pub action: SystemAction,
    pub phase: SystemActionPhase,
    pub prepared_at_ms: u64,
    pub dispatch_started_at_ms: Option<u64>,
    pub terminal_at_ms: Option<u64>,
    pub terminal_boot_binding: Option<[u8; 16]>,
}

impl TryFrom<SystemActionRecordSnapshot> for SystemActionRecord {
    type Error = SystemActionPolicyError;

    fn try_from(snapshot: SystemActionRecordSnapshot) -> Result<Self, Self::Error> {
        let record = Self {
            operation_id: snapshot.operation_id,
            fingerprint: OperationFingerprint {
                session_binding: snapshot.session_binding,
                confirmation_binding: snapshot.confirmation_binding,
                confirmation_issued_at_ms: snapshot.confirmation_issued_at_ms,
                origin_boot_binding: snapshot.origin_boot_binding,
                action: snapshot.action,
            },
            phase: snapshot.phase,
            prepared_at_ms: snapshot.prepared_at_ms,
            dispatch_started_at_ms: snapshot.dispatch_started_at_ms,
            terminal_at_ms: snapshot.terminal_at_ms,
            terminal_boot_binding: snapshot.terminal_boot_binding,
        };
        validate_record(&record)?;
        Ok(record)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionPolicyState {
    schema_version: u16,
    generation: u64,
    records: [Option<SystemActionRecord>; MAX_OPERATION_RECORDS],
}

impl SystemActionPolicyState {
    pub const fn empty() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            generation: 0,
            records: [None; MAX_OPERATION_RECORDS],
        }
    }

    pub const fn schema_version(&self) -> u16 {
        self.schema_version
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    pub const fn records(&self) -> &[Option<SystemActionRecord>; MAX_OPERATION_RECORDS] {
        &self.records
    }

    pub fn record_snapshots(&self) -> [Option<SystemActionRecordSnapshot>; MAX_OPERATION_RECORDS] {
        let mut snapshots = [None; MAX_OPERATION_RECORDS];
        for (index, record) in self.records.iter().enumerate() {
            snapshots[index] = record.map(SystemActionRecord::snapshot);
        }
        snapshots
    }

    pub fn from_record_snapshots(
        generation: u64,
        snapshots: [Option<SystemActionRecordSnapshot>; MAX_OPERATION_RECORDS],
    ) -> Result<Self, SystemActionPolicyError> {
        let mut records = [None; MAX_OPERATION_RECORDS];
        for (index, snapshot) in snapshots.into_iter().enumerate() {
            records[index] = match snapshot {
                Some(snapshot) => Some(SystemActionRecord::try_from(snapshot)?),
                None => None,
            };
        }
        let state = Self {
            schema_version: STATE_SCHEMA_VERSION,
            generation,
            records,
        };
        validate_state(&state)?;
        Ok(state)
    }
}

impl Default for SystemActionPolicyState {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionTicket {
    operation_id: [u8; 16],
    action: SystemAction,
    prepared_generation: u64,
}

impl SystemActionTicket {
    pub const fn operation_id(self) -> [u8; 16] {
        self.operation_id
    }

    pub const fn action(self) -> SystemAction {
        self.action
    }

    pub const fn prepared_generation(self) -> u64 {
        self.prepared_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemActionDispatchIntent {
    operation_id: [u8; 16],
    action: SystemAction,
    dispatch_generation: u64,
}

impl SystemActionDispatchIntent {
    pub const fn operation_id(self) -> [u8; 16] {
        self.operation_id
    }

    pub const fn action(self) -> SystemAction {
        self.action
    }

    pub const fn dispatch_generation(self) -> u64 {
        self.dispatch_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActionDecision {
    PersistPrepared(SystemActionTicket),
    ReplayPostBootAccepted,
    ReplayRejected,
    Reject(SystemActionRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActionDispatchDecision {
    PersistDispatching(SystemActionDispatchIntent),
    PersistRejected(SystemActionRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActionPostBootDecision {
    PersistAccepted,
    ReplayAccepted,
    ReplayRejected,
    Wait(SystemActionRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActionRejection {
    CapabilityDisabled,
    BackendUnavailable,
    LocalAdminRequired,
    ReauthenticationRequired,
    ConfirmationInvalid,
    ConfirmationExpired,
    BootBindingMismatch,
    StartupGuardActive,
    UpdateNotIdle,
    StorageNotIdle,
    RecoveryUnavailable,
    PostBootObserverUnavailable,
    PostBootHealthUnavailable,
    OperationIdConflict,
    IndeterminatePending,
    DispatchNotRecorded,
    PostBootNotObserved,
    CooldownActive,
    JournalFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemActionPolicyError {
    InvalidRequest,
    InvalidEnvironment,
    InvalidState,
    OperationNotFound,
    WrongPhase,
    StaleTicket,
    ClockMovedBackwards,
    GenerationExhausted,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemActionPolicyEngine {
    state: SystemActionPolicyState,
}

impl SystemActionPolicyEngine {
    pub const fn new() -> Self {
        Self {
            state: SystemActionPolicyState::empty(),
        }
    }

    pub fn restore(state: SystemActionPolicyState) -> Result<Self, SystemActionPolicyError> {
        validate_state(&state)?;
        Ok(Self { state })
    }

    pub const fn snapshot(&self) -> SystemActionPolicyState {
        self.state
    }

    pub fn record_count(&self) -> usize {
        self.state.records.iter().flatten().count()
    }

    pub fn pending_count(&self) -> usize {
        self.state
            .records
            .iter()
            .flatten()
            .filter(|record| record.phase.is_pending())
            .count()
    }

    pub fn phase_for(&self, operation_id: [u8; 16]) -> Option<SystemActionPhase> {
        self.find_record(operation_id).map(|record| record.phase)
    }

    pub fn prepare(
        &mut self,
        request: SystemActionRequest,
        eligibility: SystemActionEligibility,
        environment: SystemActionEnvironment,
    ) -> Result<SystemActionDecision, SystemActionPolicyError> {
        let fingerprint = OperationFingerprint::from(request);
        if let Some(record) = self.find_record(request.operation_id) {
            if record.fingerprint != fingerprint {
                return Ok(SystemActionDecision::Reject(
                    SystemActionRejection::OperationIdConflict,
                ));
            }
            return Ok(match record.phase {
                SystemActionPhase::Prepared | SystemActionPhase::Dispatching => {
                    SystemActionDecision::Reject(SystemActionRejection::IndeterminatePending)
                }
                SystemActionPhase::PostBootAccepted => SystemActionDecision::ReplayPostBootAccepted,
                SystemActionPhase::Rejected => SystemActionDecision::ReplayRejected,
            });
        }

        if let Some(rejection) = check_dispatch_guards(fingerprint, eligibility, environment) {
            return Ok(SystemActionDecision::Reject(rejection));
        }
        if self.pending_count() != 0 {
            return Ok(SystemActionDecision::Reject(
                SystemActionRejection::IndeterminatePending,
            ));
        }
        if self.cooldown_active(environment) {
            return Ok(SystemActionDecision::Reject(
                SystemActionRejection::CooldownActive,
            ));
        }
        let Some(slot) = self.state.records.iter().position(Option::is_none) else {
            return Ok(SystemActionDecision::Reject(
                SystemActionRejection::JournalFull,
            ));
        };
        let next_generation = self.next_generation()?;
        self.state.records[slot] = Some(SystemActionRecord {
            operation_id: request.operation_id,
            fingerprint,
            phase: SystemActionPhase::Prepared,
            prepared_at_ms: environment.monotonic_ms,
            dispatch_started_at_ms: None,
            terminal_at_ms: None,
            terminal_boot_binding: None,
        });
        self.state.generation = next_generation;
        Ok(SystemActionDecision::PersistPrepared(SystemActionTicket {
            operation_id: request.operation_id,
            action: request.action,
            prepared_generation: next_generation,
        }))
    }

    pub fn record_dispatch_intent(
        &mut self,
        ticket: SystemActionTicket,
        eligibility: SystemActionEligibility,
        environment: SystemActionEnvironment,
    ) -> Result<SystemActionDispatchDecision, SystemActionPolicyError> {
        if self.state.generation != ticket.prepared_generation {
            return Err(SystemActionPolicyError::StaleTicket);
        }
        let index = self
            .find_record_index(ticket.operation_id)
            .ok_or(SystemActionPolicyError::OperationNotFound)?;
        let record = self.state.records[index].expect("record index came from occupied slot");
        if record.phase != SystemActionPhase::Prepared || record.fingerprint.action != ticket.action
        {
            return Err(SystemActionPolicyError::WrongPhase);
        }
        if record.fingerprint.origin_boot_binding != environment.current_boot_binding {
            self.transition_to_rejected(
                index,
                environment.current_boot_binding,
                environment.monotonic_ms,
            )?;
            return Ok(SystemActionDispatchDecision::PersistRejected(
                SystemActionRejection::BootBindingMismatch,
            ));
        }
        if environment.monotonic_ms < record.prepared_at_ms {
            return Err(SystemActionPolicyError::ClockMovedBackwards);
        }
        if let Some(rejection) = check_dispatch_guards(record.fingerprint, eligibility, environment)
        {
            self.transition_to_rejected(
                index,
                environment.current_boot_binding,
                environment.monotonic_ms,
            )?;
            return Ok(SystemActionDispatchDecision::PersistRejected(rejection));
        }

        let next_generation = self.next_generation()?;
        let record = self.state.records[index]
            .as_mut()
            .expect("record index came from occupied slot");
        record.phase = SystemActionPhase::Dispatching;
        record.dispatch_started_at_ms = Some(environment.monotonic_ms);
        self.state.generation = next_generation;
        Ok(SystemActionDispatchDecision::PersistDispatching(
            SystemActionDispatchIntent {
                operation_id: ticket.operation_id,
                action: ticket.action,
                dispatch_generation: next_generation,
            },
        ))
    }

    pub fn record_dispatch_rejected(
        &mut self,
        intent: SystemActionDispatchIntent,
        environment: SystemActionEnvironment,
    ) -> Result<(), SystemActionPolicyError> {
        if self.state.generation != intent.dispatch_generation {
            return Err(SystemActionPolicyError::StaleTicket);
        }
        let index = self
            .find_record_index(intent.operation_id)
            .ok_or(SystemActionPolicyError::OperationNotFound)?;
        let record = self.state.records[index].expect("record index came from occupied slot");
        if record.phase != SystemActionPhase::Dispatching
            || record.fingerprint.action != intent.action
            || record.fingerprint.origin_boot_binding != environment.current_boot_binding
        {
            return Err(SystemActionPolicyError::WrongPhase);
        }
        if environment.monotonic_ms
            < record
                .dispatch_started_at_ms
                .ok_or(SystemActionPolicyError::InvalidState)?
        {
            return Err(SystemActionPolicyError::ClockMovedBackwards);
        }
        self.transition_to_rejected(
            index,
            environment.current_boot_binding,
            environment.monotonic_ms,
        )
    }

    pub fn accept_post_boot(
        &mut self,
        operation_id: [u8; 16],
        observation: PostBootObservation,
    ) -> Result<SystemActionPostBootDecision, SystemActionPolicyError> {
        let index = self
            .find_record_index(operation_id)
            .ok_or(SystemActionPolicyError::OperationNotFound)?;
        let record = self.state.records[index].expect("record index came from occupied slot");
        match record.phase {
            SystemActionPhase::Prepared => {
                return Ok(SystemActionPostBootDecision::Wait(
                    SystemActionRejection::DispatchNotRecorded,
                ));
            }
            SystemActionPhase::PostBootAccepted => {
                return Ok(SystemActionPostBootDecision::ReplayAccepted);
            }
            SystemActionPhase::Rejected => {
                return Ok(SystemActionPostBootDecision::ReplayRejected);
            }
            SystemActionPhase::Dispatching => {}
        }
        if observation.current_boot_binding == record.fingerprint.origin_boot_binding {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::PostBootNotObserved,
            ));
        }
        if observation.monotonic_ms < MIN_POST_BOOT_ACCEPTANCE_UPTIME_MS {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::StartupGuardActive,
            ));
        }
        if observation.update != GuardState::Ready {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::UpdateNotIdle,
            ));
        }
        if observation.storage != GuardState::Ready {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::StorageNotIdle,
            ));
        }
        if observation.recovery != GuardState::Ready {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::RecoveryUnavailable,
            ));
        }
        if observation.health != GuardState::Ready {
            return Ok(SystemActionPostBootDecision::Wait(
                SystemActionRejection::PostBootHealthUnavailable,
            ));
        }

        let next_generation = self.next_generation()?;
        let record = self.state.records[index]
            .as_mut()
            .expect("record index came from occupied slot");
        record.phase = SystemActionPhase::PostBootAccepted;
        record.terminal_at_ms = Some(observation.monotonic_ms);
        record.terminal_boot_binding = Some(observation.current_boot_binding);
        self.state.generation = next_generation;
        Ok(SystemActionPostBootDecision::PersistAccepted)
    }

    fn transition_to_rejected(
        &mut self,
        index: usize,
        terminal_boot_binding: [u8; 16],
        monotonic_ms: u64,
    ) -> Result<(), SystemActionPolicyError> {
        let record = self.state.records[index].ok_or(SystemActionPolicyError::InvalidState)?;
        if !record.phase.is_pending() || is_zero(&terminal_boot_binding) {
            return Err(SystemActionPolicyError::WrongPhase);
        }
        if terminal_boot_binding == record.fingerprint.origin_boot_binding {
            if monotonic_ms < record.prepared_at_ms {
                return Err(SystemActionPolicyError::ClockMovedBackwards);
            }
        } else if record.phase != SystemActionPhase::Prepared
            || record.dispatch_started_at_ms.is_some()
        {
            return Err(SystemActionPolicyError::WrongPhase);
        }
        let next_generation = self.next_generation()?;
        let record = self.state.records[index]
            .as_mut()
            .ok_or(SystemActionPolicyError::InvalidState)?;
        record.phase = SystemActionPhase::Rejected;
        record.terminal_at_ms = Some(monotonic_ms);
        record.terminal_boot_binding = Some(terminal_boot_binding);
        self.state.generation = next_generation;
        Ok(())
    }

    fn find_record(&self, operation_id: [u8; 16]) -> Option<SystemActionRecord> {
        self.state
            .records
            .iter()
            .flatten()
            .copied()
            .find(|record| record.operation_id == operation_id)
    }

    fn find_record_index(&self, operation_id: [u8; 16]) -> Option<usize> {
        self.state
            .records
            .iter()
            .position(|entry| entry.is_some_and(|record| record.operation_id == operation_id))
    }

    fn next_generation(&self) -> Result<u64, SystemActionPolicyError> {
        self.state
            .generation
            .checked_add(1)
            .ok_or(SystemActionPolicyError::GenerationExhausted)
    }

    fn cooldown_active(&self, environment: SystemActionEnvironment) -> bool {
        self.state.records.iter().flatten().any(|record| {
            if record.phase != SystemActionPhase::PostBootAccepted
                || record.terminal_boot_binding != Some(environment.current_boot_binding)
            {
                return false;
            }
            record.terminal_at_ms.is_none_or(|terminal_at_ms| {
                environment.monotonic_ms < terminal_at_ms.saturating_add(ACTION_COOLDOWN_MS)
            })
        })
    }
}

impl Default for SystemActionPolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn check_dispatch_guards(
    fingerprint: OperationFingerprint,
    eligibility: SystemActionEligibility,
    environment: SystemActionEnvironment,
) -> Option<SystemActionRejection> {
    if !eligibility.capability_enabled {
        return Some(SystemActionRejection::CapabilityDisabled);
    }
    if !eligibility.backend_ready {
        return Some(SystemActionRejection::BackendUnavailable);
    }
    if !eligibility.local_admin_verified {
        return Some(SystemActionRejection::LocalAdminRequired);
    }
    if !eligibility.reauthentication_verified {
        return Some(SystemActionRejection::ReauthenticationRequired);
    }
    if !eligibility.confirmation_verified {
        return Some(SystemActionRejection::ConfirmationInvalid);
    }
    if fingerprint.origin_boot_binding != environment.current_boot_binding {
        return Some(SystemActionRejection::BootBindingMismatch);
    }
    if environment.monotonic_ms < fingerprint.confirmation_issued_at_ms
        || environment
            .monotonic_ms
            .saturating_sub(fingerprint.confirmation_issued_at_ms)
            > MAX_CONFIRMATION_AGE_MS
    {
        return Some(SystemActionRejection::ConfirmationExpired);
    }
    if environment.monotonic_ms < MIN_ACTION_BOOT_UPTIME_MS {
        return Some(SystemActionRejection::StartupGuardActive);
    }
    if environment.update != GuardState::Ready {
        return Some(SystemActionRejection::UpdateNotIdle);
    }
    if environment.storage != GuardState::Ready {
        return Some(SystemActionRejection::StorageNotIdle);
    }
    if environment.recovery != GuardState::Ready {
        return Some(SystemActionRejection::RecoveryUnavailable);
    }
    if environment.post_boot_observer != GuardState::Ready {
        return Some(SystemActionRejection::PostBootObserverUnavailable);
    }
    None
}

fn validate_state(state: &SystemActionPolicyState) -> Result<(), SystemActionPolicyError> {
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(SystemActionPolicyError::InvalidState);
    }
    let record_count = state.records.iter().flatten().count();
    if state.generation < record_count as u64 {
        return Err(SystemActionPolicyError::InvalidState);
    }

    let mut pending_count = 0usize;
    for (index, record) in state.records.iter().enumerate() {
        let Some(record) = record else {
            continue;
        };
        validate_record(record)?;
        if record.phase.is_pending() {
            pending_count += 1;
        }
        if state.records[index + 1..]
            .iter()
            .flatten()
            .any(|other| other.operation_id == record.operation_id)
        {
            return Err(SystemActionPolicyError::InvalidState);
        }
    }
    if pending_count > 1 {
        return Err(SystemActionPolicyError::InvalidState);
    }
    Ok(())
}

fn validate_record(record: &SystemActionRecord) -> Result<(), SystemActionPolicyError> {
    if is_zero(&record.operation_id)
        || record.fingerprint.session_binding == 0
        || is_zero(&record.fingerprint.confirmation_binding)
        || record.fingerprint.confirmation_issued_at_ms == 0
        || is_zero(&record.fingerprint.origin_boot_binding)
        || record.prepared_at_ms < record.fingerprint.confirmation_issued_at_ms
        || record
            .prepared_at_ms
            .saturating_sub(record.fingerprint.confirmation_issued_at_ms)
            > MAX_CONFIRMATION_AGE_MS
    {
        return Err(SystemActionPolicyError::InvalidState);
    }

    let valid = match record.phase {
        SystemActionPhase::Prepared => {
            record.dispatch_started_at_ms.is_none()
                && record.terminal_at_ms.is_none()
                && record.terminal_boot_binding.is_none()
        }
        SystemActionPhase::Dispatching => {
            record
                .dispatch_started_at_ms
                .is_some_and(|value| value >= record.prepared_at_ms)
                && record.terminal_at_ms.is_none()
                && record.terminal_boot_binding.is_none()
        }
        SystemActionPhase::PostBootAccepted => {
            record
                .dispatch_started_at_ms
                .is_some_and(|value| value >= record.prepared_at_ms)
                && record.terminal_at_ms.is_some()
                && record.terminal_boot_binding.is_some_and(|binding| {
                    !is_zero(&binding) && binding != record.fingerprint.origin_boot_binding
                })
        }
        SystemActionPhase::Rejected => {
            match (record.terminal_at_ms, record.terminal_boot_binding) {
                (Some(terminal_at_ms), Some(terminal_boot_binding))
                    if !is_zero(&terminal_boot_binding) =>
                {
                    if terminal_boot_binding == record.fingerprint.origin_boot_binding {
                        record.dispatch_started_at_ms.is_none_or(|value| {
                            value >= record.prepared_at_ms && value <= terminal_at_ms
                        }) && terminal_at_ms >= record.prepared_at_ms
                    } else {
                        record.dispatch_started_at_ms.is_none()
                    }
                }
                _ => false,
            }
        }
    };
    if valid {
        Ok(())
    } else {
        Err(SystemActionPolicyError::InvalidState)
    }
}

fn is_zero<const N: usize>(value: &[u8; N]) -> bool {
    value.iter().all(|byte| *byte == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOT_A: [u8; 16] = [0xA1; 16];
    const BOOT_B: [u8; 16] = [0xB2; 16];
    const BOOT_C: [u8; 16] = [0xC3; 16];

    fn request(
        seed: u8,
        action: SystemAction,
        issued_at_ms: u64,
        boot: [u8; 16],
    ) -> SystemActionRequest {
        SystemActionRequest::new(
            [seed; 16],
            u64::from(seed),
            [seed; 32],
            issued_at_ms,
            boot,
            action,
        )
        .unwrap()
    }

    fn ready_environment(boot: [u8; 16], now_ms: u64) -> SystemActionEnvironment {
        SystemActionEnvironment::new(
            boot,
            now_ms,
            GuardState::Ready,
            GuardState::Ready,
            GuardState::Ready,
            GuardState::Ready,
        )
        .unwrap()
    }

    fn ready_observation(boot: [u8; 16], now_ms: u64) -> PostBootObservation {
        PostBootObservation::new(
            boot,
            now_ms,
            GuardState::Ready,
            GuardState::Ready,
            GuardState::Ready,
            GuardState::Ready,
        )
        .unwrap()
    }

    fn prepared_ticket(
        engine: &mut SystemActionPolicyEngine,
        request: SystemActionRequest,
        now_ms: u64,
    ) -> SystemActionTicket {
        match engine
            .prepare(
                request,
                SystemActionEligibility::verified_for_test(),
                ready_environment(request.origin_boot_binding, now_ms),
            )
            .unwrap()
        {
            SystemActionDecision::PersistPrepared(ticket) => ticket,
            other => panic!("unexpected prepare decision: {other:?}"),
        }
    }

    fn dispatch_intent(
        engine: &mut SystemActionPolicyEngine,
        ticket: SystemActionTicket,
        boot: [u8; 16],
        now_ms: u64,
    ) -> SystemActionDispatchIntent {
        match engine
            .record_dispatch_intent(
                ticket,
                SystemActionEligibility::verified_for_test(),
                ready_environment(boot, now_ms),
            )
            .unwrap()
        {
            SystemActionDispatchDecision::PersistDispatching(intent) => intent,
            other => panic!("unexpected dispatch decision: {other:?}"),
        }
    }

    #[test]
    fn public_eligibility_is_disabled_and_does_not_mutate_state() {
        let mut engine = SystemActionPolicyEngine::new();
        let before = engine.snapshot();
        assert_eq!(
            engine
                .prepare(
                    request(1, SystemAction::Reboot, 60_000, BOOT_A),
                    SystemActionEligibility::disabled(),
                    ready_environment(BOOT_A, 60_001),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::CapabilityDisabled)
        );
        assert_eq!(engine.snapshot(), before);
        assert_eq!(engine.pending_count(), 0);
    }

    #[test]
    fn authorization_and_operational_guards_fail_closed() {
        let operation = request(1, SystemAction::Shutdown, 60_000, BOOT_A);
        let verified = SystemActionEligibility::verified_for_test();
        let environment = ready_environment(BOOT_A, 60_001);

        let eligibility_cases = [
            (
                SystemActionEligibility {
                    backend_ready: false,
                    ..verified
                },
                SystemActionRejection::BackendUnavailable,
            ),
            (
                SystemActionEligibility {
                    local_admin_verified: false,
                    ..verified
                },
                SystemActionRejection::LocalAdminRequired,
            ),
            (
                SystemActionEligibility {
                    reauthentication_verified: false,
                    ..verified
                },
                SystemActionRejection::ReauthenticationRequired,
            ),
            (
                SystemActionEligibility {
                    confirmation_verified: false,
                    ..verified
                },
                SystemActionRejection::ConfirmationInvalid,
            ),
        ];
        for (eligibility, expected) in eligibility_cases {
            let mut engine = SystemActionPolicyEngine::new();
            assert_eq!(
                engine.prepare(operation, eligibility, environment).unwrap(),
                SystemActionDecision::Reject(expected)
            );
            assert_eq!(engine.record_count(), 0);
        }

        let environment_cases = [
            (
                SystemActionEnvironment {
                    update: GuardState::Blocked,
                    ..environment
                },
                SystemActionRejection::UpdateNotIdle,
            ),
            (
                SystemActionEnvironment {
                    storage: GuardState::Unknown,
                    ..environment
                },
                SystemActionRejection::StorageNotIdle,
            ),
            (
                SystemActionEnvironment {
                    recovery: GuardState::Blocked,
                    ..environment
                },
                SystemActionRejection::RecoveryUnavailable,
            ),
            (
                SystemActionEnvironment {
                    post_boot_observer: GuardState::Unknown,
                    ..environment
                },
                SystemActionRejection::PostBootObserverUnavailable,
            ),
        ];
        for (environment, expected) in environment_cases {
            let mut engine = SystemActionPolicyEngine::new();
            assert_eq!(
                engine.prepare(operation, verified, environment).unwrap(),
                SystemActionDecision::Reject(expected)
            );
            assert_eq!(engine.record_count(), 0);
        }
    }

    #[test]
    fn confirmation_boot_binding_and_startup_age_are_bounded() {
        let verified = SystemActionEligibility::verified_for_test();
        let operation = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let cases = [
            (
                ready_environment(BOOT_B, 60_001),
                SystemActionRejection::BootBindingMismatch,
            ),
            (
                ready_environment(BOOT_A, 59_999),
                SystemActionRejection::ConfirmationExpired,
            ),
            (
                ready_environment(BOOT_A, 70_001),
                SystemActionRejection::ConfirmationExpired,
            ),
            (
                ready_environment(BOOT_A, 60_000 - 1),
                SystemActionRejection::ConfirmationExpired,
            ),
        ];
        for (environment, expected) in cases {
            let mut engine = SystemActionPolicyEngine::new();
            assert_eq!(
                engine.prepare(operation, verified, environment).unwrap(),
                SystemActionDecision::Reject(expected)
            );
        }

        let early = request(2, SystemAction::Reboot, 59_000, BOOT_A);
        assert_eq!(
            SystemActionPolicyEngine::new()
                .prepare(
                    early,
                    verified,
                    ready_environment(BOOT_A, MIN_ACTION_BOOT_UPTIME_MS - 1),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::StartupGuardActive)
        );
        assert_eq!(
            SystemActionEnvironment::new(
                [0; 16],
                0,
                GuardState::Ready,
                GuardState::Ready,
                GuardState::Ready,
                GuardState::Ready,
            ),
            Err(SystemActionPolicyError::InvalidEnvironment)
        );
    }

    #[test]
    fn prepare_and_dispatch_require_two_persistable_generations() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        assert_eq!(ticket.action(), SystemAction::Reboot);
        assert_eq!(ticket.prepared_generation(), 1);
        assert_eq!(engine.phase_for([1; 16]), Some(SystemActionPhase::Prepared));

        let prepared_snapshot = engine.snapshot();
        let mut restored = SystemActionPolicyEngine::restore(prepared_snapshot).unwrap();
        let intent = dispatch_intent(&mut restored, ticket, BOOT_A, 60_002);
        assert_eq!(intent.action(), SystemAction::Reboot);
        assert_eq!(intent.dispatch_generation(), 2);
        assert_eq!(
            restored.phase_for([1; 16]),
            Some(SystemActionPhase::Dispatching)
        );
        assert_eq!(restored.pending_count(), 1);
    }

    #[test]
    fn pending_state_never_replays_or_dispatches_automatically() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Shutdown, 60_000, BOOT_A);
        let _ticket = prepared_ticket(&mut engine, operation, 60_001);
        let mut restored = SystemActionPolicyEngine::restore(engine.snapshot()).unwrap();
        assert_eq!(
            restored
                .prepare(
                    operation,
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_A, 60_002),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::IndeterminatePending)
        );
        assert_eq!(
            restored
                .prepare(
                    request(2, SystemAction::Reboot, 60_002, BOOT_A),
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_A, 60_003),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::IndeterminatePending)
        );
        assert_eq!(
            restored
                .accept_post_boot([1; 16], ready_observation(BOOT_B, 30_000))
                .unwrap(),
            SystemActionPostBootDecision::Wait(SystemActionRejection::DispatchNotRecorded)
        );
        assert_eq!(
            restored.phase_for([1; 16]),
            Some(SystemActionPhase::Prepared)
        );
    }

    #[test]
    fn prepared_state_from_a_prior_boot_is_rejected_without_dispatch() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Shutdown, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        assert_eq!(
            engine
                .record_dispatch_intent(
                    ticket,
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_B, 30_000),
                )
                .unwrap(),
            SystemActionDispatchDecision::PersistRejected(
                SystemActionRejection::BootBindingMismatch
            )
        );
        let restored = SystemActionPolicyEngine::restore(engine.snapshot()).unwrap();
        let record = restored.snapshot().records()[0].unwrap();
        assert_eq!(record.phase(), SystemActionPhase::Rejected);
        assert_eq!(record.dispatch_started_at_ms(), None);
        assert_eq!(record.terminal_boot_binding(), Some(BOOT_B));
    }

    #[test]
    fn post_boot_acceptance_requires_changed_boot_and_all_health_guards() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        let _intent = dispatch_intent(&mut engine, ticket, BOOT_A, 60_002);

        assert_eq!(
            engine
                .accept_post_boot([1; 16], ready_observation(BOOT_A, 90_000))
                .unwrap(),
            SystemActionPostBootDecision::Wait(SystemActionRejection::PostBootNotObserved)
        );
        assert_eq!(
            engine
                .accept_post_boot(
                    [1; 16],
                    ready_observation(BOOT_B, MIN_POST_BOOT_ACCEPTANCE_UPTIME_MS - 1),
                )
                .unwrap(),
            SystemActionPostBootDecision::Wait(SystemActionRejection::StartupGuardActive)
        );
        let unhealthy = PostBootObservation {
            health: GuardState::Unknown,
            ..ready_observation(BOOT_B, MIN_POST_BOOT_ACCEPTANCE_UPTIME_MS)
        };
        assert_eq!(
            engine.accept_post_boot([1; 16], unhealthy).unwrap(),
            SystemActionPostBootDecision::Wait(SystemActionRejection::PostBootHealthUnavailable)
        );
        assert_eq!(
            engine
                .accept_post_boot(
                    [1; 16],
                    ready_observation(BOOT_B, MIN_POST_BOOT_ACCEPTANCE_UPTIME_MS),
                )
                .unwrap(),
            SystemActionPostBootDecision::PersistAccepted
        );
        assert_eq!(
            engine.phase_for([1; 16]),
            Some(SystemActionPhase::PostBootAccepted)
        );
        assert_eq!(
            engine
                .accept_post_boot([1; 16], ready_observation(BOOT_B, 30_001))
                .unwrap(),
            SystemActionPostBootDecision::ReplayAccepted
        );
        assert_eq!(
            engine
                .prepare(
                    operation,
                    SystemActionEligibility::disabled(),
                    ready_environment(BOOT_B, 90_000),
                )
                .unwrap(),
            SystemActionDecision::ReplayPostBootAccepted
        );
    }

    #[test]
    fn guards_are_rechecked_before_dispatch_and_rejection_is_replayable() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Shutdown, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        let blocked = SystemActionEnvironment {
            update: GuardState::Blocked,
            ..ready_environment(BOOT_A, 60_002)
        };
        assert_eq!(
            engine
                .record_dispatch_intent(
                    ticket,
                    SystemActionEligibility::verified_for_test(),
                    blocked,
                )
                .unwrap(),
            SystemActionDispatchDecision::PersistRejected(SystemActionRejection::UpdateNotIdle)
        );
        assert_eq!(engine.phase_for([1; 16]), Some(SystemActionPhase::Rejected));
        assert_eq!(engine.pending_count(), 0);
        assert_eq!(
            engine
                .prepare(
                    operation,
                    SystemActionEligibility::disabled(),
                    ready_environment(BOOT_A, 70_000),
                )
                .unwrap(),
            SystemActionDecision::ReplayRejected
        );
    }

    #[test]
    fn synchronous_dispatch_failure_is_terminal_and_stale_intents_fail() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        let intent = dispatch_intent(&mut engine, ticket, BOOT_A, 60_002);
        engine
            .record_dispatch_rejected(intent, ready_environment(BOOT_A, 60_003))
            .unwrap();
        assert_eq!(engine.phase_for([1; 16]), Some(SystemActionPhase::Rejected));
        assert_eq!(
            engine.record_dispatch_rejected(intent, ready_environment(BOOT_A, 60_004)),
            Err(SystemActionPolicyError::StaleTicket)
        );
        assert_eq!(
            engine
                .accept_post_boot([1; 16], ready_observation(BOOT_B, 30_000))
                .unwrap(),
            SystemActionPostBootDecision::ReplayRejected
        );
    }

    #[test]
    fn operation_id_conflicts_and_post_boot_cooldown_fail_closed() {
        let mut engine = SystemActionPolicyEngine::new();
        let reboot = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, reboot, 60_001);
        assert_eq!(
            engine
                .prepare(
                    request(1, SystemAction::Shutdown, 60_000, BOOT_A),
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_A, 60_002),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::OperationIdConflict)
        );
        let _intent = dispatch_intent(&mut engine, ticket, BOOT_A, 60_002);
        assert_eq!(
            engine
                .accept_post_boot([1; 16], ready_observation(BOOT_B, 30_000))
                .unwrap(),
            SystemActionPostBootDecision::PersistAccepted
        );

        let next = request(2, SystemAction::Shutdown, 89_999, BOOT_B);
        assert_eq!(
            engine
                .prepare(
                    next,
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_B, 90_000 - 1),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::CooldownActive)
        );
        assert!(matches!(
            engine
                .prepare(
                    request(3, SystemAction::Shutdown, 89_999, BOOT_B),
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_B, 90_000),
                )
                .unwrap(),
            SystemActionDecision::PersistPrepared(_)
        ));

        let mut later_boot = SystemActionPolicyEngine::restore(engine.snapshot()).unwrap();
        assert_eq!(
            later_boot
                .prepare(
                    request(4, SystemAction::Reboot, 59_998, BOOT_C),
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_C, 59_999),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::StartupGuardActive)
        );
    }

    #[test]
    fn bounded_journal_never_prunes_idempotency_records_implicitly() {
        let mut engine = SystemActionPolicyEngine::new();
        for seed in 1..=MAX_OPERATION_RECORDS as u8 {
            let now_ms = 100_000 + u64::from(seed) * 10;
            let operation = request(seed, SystemAction::Reboot, now_ms - 1, BOOT_A);
            let ticket = prepared_ticket(&mut engine, operation, now_ms);
            let disabled_backend = SystemActionEligibility {
                backend_ready: false,
                ..SystemActionEligibility::verified_for_test()
            };
            assert_eq!(
                engine
                    .record_dispatch_intent(
                        ticket,
                        disabled_backend,
                        ready_environment(BOOT_A, now_ms + 1),
                    )
                    .unwrap(),
                SystemActionDispatchDecision::PersistRejected(
                    SystemActionRejection::BackendUnavailable
                )
            );
        }
        assert_eq!(engine.record_count(), MAX_OPERATION_RECORDS);
        assert_eq!(
            engine
                .prepare(
                    request(99, SystemAction::Shutdown, 200_000, BOOT_A),
                    SystemActionEligibility::verified_for_test(),
                    ready_environment(BOOT_A, 200_001),
                )
                .unwrap(),
            SystemActionDecision::Reject(SystemActionRejection::JournalFull)
        );
        assert_eq!(engine.record_count(), MAX_OPERATION_RECORDS);
    }

    #[test]
    fn restore_rejects_corruption_and_generation_exhaustion_is_non_mutating() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Reboot, 60_000, BOOT_A);
        let _ticket = prepared_ticket(&mut engine, operation, 60_001);

        let mut wrong_schema = engine.snapshot();
        wrong_schema.schema_version += 1;
        assert_eq!(
            SystemActionPolicyEngine::restore(wrong_schema),
            Err(SystemActionPolicyError::InvalidState)
        );

        let mut duplicate = engine.snapshot();
        duplicate.records[1] = duplicate.records[0];
        duplicate.generation = 2;
        assert_eq!(
            SystemActionPolicyEngine::restore(duplicate),
            Err(SystemActionPolicyError::InvalidState)
        );

        let exhausted_state = SystemActionPolicyState {
            schema_version: STATE_SCHEMA_VERSION,
            generation: u64::MAX,
            records: [None; MAX_OPERATION_RECORDS],
        };
        let mut exhausted = SystemActionPolicyEngine::restore(exhausted_state).unwrap();
        let before = exhausted.snapshot();
        assert_eq!(
            exhausted.prepare(
                request(2, SystemAction::Shutdown, 60_000, BOOT_A),
                SystemActionEligibility::verified_for_test(),
                ready_environment(BOOT_A, 60_001),
            ),
            Err(SystemActionPolicyError::GenerationExhausted)
        );
        assert_eq!(exhausted.snapshot(), before);
    }

    #[test]
    fn record_snapshots_round_trip_only_through_state_validation() {
        let mut engine = SystemActionPolicyEngine::new();
        let operation = request(1, SystemAction::Shutdown, 60_000, BOOT_A);
        let ticket = prepared_ticket(&mut engine, operation, 60_001);
        let _intent = dispatch_intent(&mut engine, ticket, BOOT_A, 60_002);
        let state = engine.snapshot();
        let snapshots = state.record_snapshots();
        assert_eq!(
            SystemActionPolicyState::from_record_snapshots(state.generation(), snapshots).unwrap(),
            state
        );

        let mut invalid = snapshots;
        invalid[0].as_mut().unwrap().operation_id = [0; 16];
        assert_eq!(
            SystemActionPolicyState::from_record_snapshots(state.generation(), invalid),
            Err(SystemActionPolicyError::InvalidState)
        );
    }
}
