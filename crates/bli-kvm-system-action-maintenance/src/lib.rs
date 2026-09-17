use std::{ffi::OsString, fmt, fs};

use bli_kvm_system_action_policy::{
    GuardState, PostBootObservation, SystemActionPhase, SystemActionPolicyEngine,
    SystemActionPolicyState, SystemActionPostBootDecision,
};
use bli_kvm_system_action_store::{SystemActionStore, SystemActionStoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceCommand {
    InitializeEmpty,
    InspectEmpty,
    AcceptPostBoot,
}

impl MaintenanceCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitializeEmpty => "initialize-empty",
            Self::InspectEmpty => "inspect-empty",
            Self::AcceptPostBoot => "accept-post-boot",
        }
    }
}

/// How the caller attests the post-boot guards. `Ready` is a reviewed claim
/// that the update, storage, recovery and health checks passed; any other value
/// lets the policy answer `Wait` without changing the journal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostBootGuards {
    Ready,
    Unknown,
}

/// The reviewed inputs of one acceptance attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PostBootRequest {
    pub guards: PostBootGuards,
    pub expect_operation: Option<[u8; 16]>,
    pub require_accepted: bool,
}

impl PostBootRequest {
    pub const fn new(
        guards: PostBootGuards,
        expect_operation: Option<[u8; 16]>,
        require_accepted: bool,
    ) -> Self {
        Self {
            guards,
            expect_operation,
            require_accepted,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptanceOutcome {
    Accepted {
        operation_id: [u8; 16],
        generation: u64,
    },
    Waiting {
        operation_id: [u8; 16],
    },
    Rejected {
        operation_id: [u8; 16],
    },
    Absent,
    Ambiguous,
}

/// The parsed command line: a command plus the reviewed acceptance inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceInvocation {
    pub command: MaintenanceCommand,
    pub request: PostBootRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceSnapshot {
    pub generation: u64,
    pub degraded: bool,
    pub state: SystemActionPolicyState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceBackendError {
    OperationRejected,
}

pub trait MaintenanceBackend {
    fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError>;
    fn inspect(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError>;
    fn accept_post_boot(
        &self,
        request: &PostBootRequest,
    ) -> Result<AcceptanceOutcome, MaintenanceBackendError>;
}

#[derive(Debug)]
pub struct ExactStoreBackend {
    store: SystemActionStore,
}

impl ExactStoreBackend {
    pub fn open() -> Result<Self, MaintenanceError> {
        Ok(Self {
            store: SystemActionStore::open().map_err(MaintenanceError::Store)?,
        })
    }
}

/// The current boot identity from the kernel: `/proc/sys/kernel/random/boot_id`
/// is a UUID-shaped value that changes on every boot.
fn current_boot_binding() -> Result<[u8; 16], MaintenanceBackendError> {
    let raw = fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map_err(|_| MaintenanceBackendError::OperationRejected)?;
    parse_boot_binding(&raw).ok_or(MaintenanceBackendError::OperationRejected)
}

/// Parse a UUID-shaped boot identity into the 16-byte binding the policy uses.
fn parse_boot_binding(raw: &str) -> Option<[u8; 16]> {
    let mut bytes = [0_u8; 16];
    let mut count = 0;
    let mut high: Option<u8> = None;
    for character in raw.trim().chars() {
        if character == '-' {
            continue;
        }
        let digit = character.to_digit(16)? as u8;
        match high.take() {
            None => high = Some(digit),
            Some(previous) => {
                if count == bytes.len() {
                    return None;
                }
                bytes[count] = (previous << 4) | digit;
                count += 1;
            }
        }
    }
    if count == bytes.len() && high.is_none() && bytes.iter().any(|byte| *byte != 0) {
        Some(bytes)
    } else {
        None
    }
}

/// Milliseconds since boot from `/proc/uptime`.
fn current_monotonic_ms() -> Result<u64, MaintenanceBackendError> {
    let raw = fs::read_to_string("/proc/uptime")
        .map_err(|_| MaintenanceBackendError::OperationRejected)?;
    let seconds: f64 = raw
        .split_whitespace()
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or(MaintenanceBackendError::OperationRejected)?;
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(MaintenanceBackendError::OperationRejected);
    }
    Ok((seconds * 1000.0) as u64)
}

fn post_boot_observation(
    guards: PostBootGuards,
) -> Result<PostBootObservation, MaintenanceBackendError> {
    let guard_state = match guards {
        PostBootGuards::Ready => GuardState::Ready,
        PostBootGuards::Unknown => GuardState::Unknown,
    };
    PostBootObservation::new(
        current_boot_binding()?,
        current_monotonic_ms()?,
        guard_state,
        guard_state,
        guard_state,
        guard_state,
    )
    .map_err(|_| MaintenanceBackendError::OperationRejected)
}

impl MaintenanceBackend for ExactStoreBackend {
    fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError> {
        self.store
            .initialize(SystemActionPolicyState::empty())
            .map(|persisted| persisted.generation)
            .map_err(|_| MaintenanceBackendError::OperationRejected)
    }

    fn accept_post_boot(
        &self,
        request: &PostBootRequest,
    ) -> Result<AcceptanceOutcome, MaintenanceBackendError> {
        let selected = self
            .store
            .load(1)
            .map_err(|_| MaintenanceBackendError::OperationRejected)?;
        if selected.degraded() {
            // A degraded journal is not evidence; acceptance stays closed.
            return Err(MaintenanceBackendError::OperationRejected);
        }
        let state = selected.state();
        let mut engine = SystemActionPolicyEngine::restore(state)
            .map_err(|_| MaintenanceBackendError::OperationRejected)?;
        let mut pending = None;
        let mut pending_count = 0_usize;
        for record in state.record_snapshots().into_iter().flatten() {
            if !matches!(
                record.phase,
                SystemActionPhase::Prepared | SystemActionPhase::Dispatching
            ) {
                continue;
            }
            if let Some(expected) = request.expect_operation {
                if expected != record.operation_id {
                    continue;
                }
            }
            pending = Some(record.operation_id);
            pending_count += 1;
        }
        match pending_count {
            0 => Ok(AcceptanceOutcome::Absent),
            1 => {
                let operation_id = pending.expect("counted record");
                let observation = post_boot_observation(request.guards)?;
                let decision = engine
                    .accept_post_boot(operation_id, observation)
                    .map_err(|_| MaintenanceBackendError::OperationRejected)?;
                match decision {
                    SystemActionPostBootDecision::PersistAccepted => {
                        let persisted = self
                            .store
                            .update(selected.generation(), 1, engine.snapshot())
                            .map_err(|_| MaintenanceBackendError::OperationRejected)?;
                        Ok(AcceptanceOutcome::Accepted {
                            operation_id,
                            generation: persisted.generation,
                        })
                    }
                    SystemActionPostBootDecision::ReplayAccepted => {
                        Ok(AcceptanceOutcome::Accepted {
                            operation_id,
                            generation: selected.generation(),
                        })
                    }
                    SystemActionPostBootDecision::ReplayRejected => {
                        Ok(AcceptanceOutcome::Rejected { operation_id })
                    }
                    SystemActionPostBootDecision::Wait(_reason) => {
                        Ok(AcceptanceOutcome::Waiting { operation_id })
                    }
                }
            }
            _ => Ok(AcceptanceOutcome::Ambiguous),
        }
    }

    fn inspect(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError> {
        self.store
            .load(1)
            .map(|selected| MaintenanceSnapshot {
                generation: selected.generation(),
                degraded: selected.degraded(),
                state: selected.state(),
            })
            .map_err(|_| MaintenanceBackendError::OperationRejected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaintenanceReceipt {
    command: MaintenanceCommand,
    outcome: Option<AcceptanceOutcome>,
}

impl MaintenanceReceipt {
    pub const fn command(self) -> MaintenanceCommand {
        self.command
    }

    pub const fn outcome(self) -> Option<AcceptanceOutcome> {
        self.outcome
    }

    /// The one-shot acceptance step is required to close the record it was
    /// started for; every other outcome keeps the journal unchanged.
    pub const fn accepted(self) -> bool {
        matches!(self.outcome, Some(AcceptanceOutcome::Accepted { .. }))
    }
}

fn hex_operation(operation_id: [u8; 16]) -> String {
    let mut value = String::with_capacity(32);
    for byte in operation_id {
        value.push_str(&format!("{byte:02x}"));
    }
    value
}

impl fmt::Display for MaintenanceReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.outcome {
            None => write!(
                formatter,
                "command={} result=empty generation=1 copies=healthy",
                self.command.as_str()
            ),
            Some(AcceptanceOutcome::Accepted {
                operation_id,
                generation,
            }) => write!(
                formatter,
                "command={} result=accepted operation={} generation={} copies=healthy",
                self.command.as_str(),
                hex_operation(operation_id),
                generation
            ),
            Some(AcceptanceOutcome::Waiting { operation_id }) => write!(
                formatter,
                "command={} result=waiting operation={} copies=healthy",
                self.command.as_str(),
                hex_operation(operation_id)
            ),
            Some(AcceptanceOutcome::Rejected { operation_id }) => write!(
                formatter,
                "command={} result=rejected operation={} copies=healthy",
                self.command.as_str(),
                hex_operation(operation_id)
            ),
            Some(AcceptanceOutcome::Absent) => write!(
                formatter,
                "command={} result=absent copies=healthy",
                self.command.as_str()
            ),
            Some(AcceptanceOutcome::Ambiguous) => write!(
                formatter,
                "command={} result=ambiguous copies=healthy",
                self.command.as_str()
            ),
        }
    }
}

#[derive(Debug)]
pub enum MaintenanceError {
    InvalidArguments,
    Store(SystemActionStoreError),
    Backend,
    UnexpectedState,
    NotAccepted,
}

impl MaintenanceError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::Store(_) => 10,
            Self::Backend => 11,
            Self::UnexpectedState => 12,
            Self::NotAccepted => 13,
        }
    }
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArguments => formatter.write_str("arguments are invalid"),
            Self::Store(_) => formatter.write_str("exact system-action store is unavailable"),
            Self::Backend => formatter.write_str("system-action store operation failed"),
            Self::UnexpectedState => {
                formatter.write_str("system-action store is not empty/healthy")
            }
            Self::NotAccepted => {
                formatter.write_str("system-action post-boot acceptance did not close the record")
            }
        }
    }
}

impl std::error::Error for MaintenanceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

pub fn parse_arguments(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<MaintenanceCommand, MaintenanceError> {
    let invocation = parse_invocation(arguments)?;
    if invocation.command == MaintenanceCommand::AcceptPostBoot {
        return Err(MaintenanceError::InvalidArguments);
    }
    if invocation.request != PostBootRequest::new(PostBootGuards::Unknown, None, false) {
        return Err(MaintenanceError::InvalidArguments);
    }
    Ok(invocation.command)
}

fn parse_hex_operation(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut bytes = [0_u8; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        let pair = value.get(index * 2..index * 2 + 2)?;
        *byte = u8::from_str_radix(pair, 16).ok()?;
    }
    if bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    Some(bytes)
}

pub fn parse_invocation(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<MaintenanceInvocation, MaintenanceError> {
    let mut arguments = arguments.into_iter();
    let _program = arguments.next().ok_or(MaintenanceError::InvalidArguments)?;
    let command = match arguments.next().as_deref() {
        Some(value) if value == "initialize-empty" => MaintenanceCommand::InitializeEmpty,
        Some(value) if value == "inspect-empty" => MaintenanceCommand::InspectEmpty,
        Some(value) if value == "accept-post-boot" => MaintenanceCommand::AcceptPostBoot,
        _ => return Err(MaintenanceError::InvalidArguments),
    };
    let mut request = PostBootRequest::new(PostBootGuards::Unknown, None, false);
    let mut accepted_command = false;
    while let Some(argument) = arguments.next() {
        if command != MaintenanceCommand::AcceptPostBoot {
            return Err(MaintenanceError::InvalidArguments);
        }
        match argument.to_str() {
            Some("--expect-operation") => {
                let operation = arguments
                    .next()
                    .and_then(|value| value.to_str().and_then(parse_hex_operation))
                    .ok_or(MaintenanceError::InvalidArguments)?;
                request.expect_operation = Some(operation);
                accepted_command = true;
            }
            Some("--pending") => accepted_command = true,
            Some("--require-accepted") => request.require_accepted = true,
            Some("--guards-ready") => request.guards = PostBootGuards::Ready,
            _ => return Err(MaintenanceError::InvalidArguments),
        }
    }
    if command == MaintenanceCommand::AcceptPostBoot && !accepted_command {
        // Acceptance is only meaningful for a named command.
        return Err(MaintenanceError::InvalidArguments);
    }
    Ok(MaintenanceInvocation { command, request })
}

pub fn run_with_backend(
    command: MaintenanceCommand,
    backend: &impl MaintenanceBackend,
) -> Result<MaintenanceReceipt, MaintenanceError> {
    if command == MaintenanceCommand::InitializeEmpty
        && backend
            .initialize_empty()
            .map_err(|_| MaintenanceError::Backend)?
            != 1
    {
        return Err(MaintenanceError::UnexpectedState);
    }
    let snapshot = backend.inspect().map_err(|_| MaintenanceError::Backend)?;
    if snapshot.generation != 1
        || snapshot.degraded
        || snapshot.state != SystemActionPolicyState::empty()
    {
        return Err(MaintenanceError::UnexpectedState);
    }
    Ok(MaintenanceReceipt {
        command,
        outcome: None,
    })
}

pub fn run_exact(command: MaintenanceCommand) -> Result<MaintenanceReceipt, MaintenanceError> {
    let backend = ExactStoreBackend::open()?;
    run_with_backend(command, &backend)
}

pub fn run_invocation_with_backend(
    invocation: MaintenanceInvocation,
    backend: &impl MaintenanceBackend,
) -> Result<MaintenanceReceipt, MaintenanceError> {
    if invocation.command != MaintenanceCommand::AcceptPostBoot {
        return run_with_backend(invocation.command, backend);
    }
    let outcome = backend
        .accept_post_boot(&invocation.request)
        .map_err(|_| MaintenanceError::Backend)?;
    let receipt = MaintenanceReceipt {
        command: invocation.command,
        outcome: Some(outcome),
    };
    if invocation.request.require_accepted && !receipt.accepted() {
        return Err(MaintenanceError::NotAccepted);
    }
    Ok(receipt)
}

pub fn run_exact_invocation(
    invocation: MaintenanceInvocation,
) -> Result<MaintenanceReceipt, MaintenanceError> {
    let backend = ExactStoreBackend::open()?;
    run_invocation_with_backend(invocation, &backend)
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    #[derive(Debug)]
    struct FakeBackend {
        initialized: Cell<usize>,
        initialize_generation: Result<u64, MaintenanceBackendError>,
        snapshot: Result<MaintenanceSnapshot, MaintenanceBackendError>,
        acceptance: Result<AcceptanceOutcome, MaintenanceBackendError>,
    }

    impl MaintenanceBackend for FakeBackend {
        fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError> {
            self.initialized.set(self.initialized.get() + 1);
            self.initialize_generation
        }

        fn inspect(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError> {
            self.snapshot
        }

        fn accept_post_boot(
            &self,
            _request: &PostBootRequest,
        ) -> Result<AcceptanceOutcome, MaintenanceBackendError> {
            self.acceptance
        }
    }

    fn healthy() -> MaintenanceSnapshot {
        MaintenanceSnapshot {
            generation: 1,
            degraded: false,
            state: SystemActionPolicyState::empty(),
        }
    }

    fn backend(snapshot: MaintenanceSnapshot) -> FakeBackend {
        FakeBackend {
            initialized: Cell::new(0),
            initialize_generation: Ok(1),
            snapshot: Ok(snapshot),
            acceptance: Ok(AcceptanceOutcome::Absent),
        }
    }

    #[test]
    fn parser_accepts_only_two_argumentless_commands() {
        assert_eq!(
            parse_arguments(["tool", "initialize-empty"].map(OsString::from)).unwrap(),
            MaintenanceCommand::InitializeEmpty
        );
        assert_eq!(
            parse_arguments(["tool", "inspect-empty"].map(OsString::from)).unwrap(),
            MaintenanceCommand::InspectEmpty
        );
        for invalid in [
            vec!["tool"],
            vec!["tool", "repair"],
            vec!["tool", "inspect-empty", "extra"],
            vec!["tool", "reboot"],
            vec!["tool", "shutdown"],
        ] {
            assert!(matches!(
                parse_arguments(invalid.into_iter().map(OsString::from)),
                Err(MaintenanceError::InvalidArguments)
            ));
        }
    }

    #[test]
    fn initialization_creates_only_generation_one_empty_state() {
        let backend = backend(healthy());
        let receipt = run_with_backend(MaintenanceCommand::InitializeEmpty, &backend).unwrap();
        assert_eq!(backend.initialized.get(), 1);
        assert_eq!(receipt.command(), MaintenanceCommand::InitializeEmpty);
        assert_eq!(
            receipt.to_string(),
            "command=initialize-empty result=empty generation=1 copies=healthy"
        );
    }

    #[test]
    fn inspect_never_initializes_or_repairs() {
        let backend = backend(healthy());
        let receipt = run_with_backend(MaintenanceCommand::InspectEmpty, &backend).unwrap();
        assert_eq!(backend.initialized.get(), 0);
        assert_eq!(receipt.command(), MaintenanceCommand::InspectEmpty);
    }

    #[test]
    fn degraded_nonempty_or_wrong_generation_is_rejected() {
        for snapshot in [
            MaintenanceSnapshot {
                degraded: true,
                ..healthy()
            },
            MaintenanceSnapshot {
                generation: 2,
                ..healthy()
            },
            MaintenanceSnapshot {
                state: SystemActionPolicyState::from_record_snapshots(
                    1,
                    SystemActionPolicyState::empty().record_snapshots(),
                )
                .unwrap(),
                ..healthy()
            },
        ] {
            let backend = backend(snapshot);
            assert!(matches!(
                run_with_backend(MaintenanceCommand::InspectEmpty, &backend),
                Err(MaintenanceError::UnexpectedState)
            ));
        }
    }

    #[test]
    fn backend_failures_and_wrong_initial_generation_are_stable() {
        let failing = FakeBackend {
            initialized: Cell::new(0),
            initialize_generation: Err(MaintenanceBackendError::OperationRejected),
            acceptance: Ok(AcceptanceOutcome::Absent),
            snapshot: Err(MaintenanceBackendError::OperationRejected),
        };
        assert!(matches!(
            run_with_backend(MaintenanceCommand::InitializeEmpty, &failing),
            Err(MaintenanceError::Backend)
        ));
        let wrong = FakeBackend {
            initialized: Cell::new(0),
            initialize_generation: Ok(2),
            snapshot: Ok(healthy()),
            acceptance: Ok(AcceptanceOutcome::Absent),
        };
        assert!(matches!(
            run_with_backend(MaintenanceCommand::InitializeEmpty, &wrong),
            Err(MaintenanceError::UnexpectedState)
        ));
    }

    #[test]
    fn accept_post_boot_requires_a_named_operation_and_flags() {
        let invocation = parse_invocation(
            [
                "tool",
                "accept-post-boot",
                "--expect-operation",
                "ab12ab12ab12ab12ab12ab12ab12ab12",
                "--guards-ready",
                "--require-accepted",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert_eq!(invocation.command, MaintenanceCommand::AcceptPostBoot);
        assert_eq!(invocation.request.guards, PostBootGuards::Ready);
        assert!(invocation.request.require_accepted);
        assert_eq!(
            invocation.request.expect_operation,
            Some([
                0xab, 0x12, 0xab, 0x12, 0xab, 0x12, 0xab, 0x12, 0xab, 0x12, 0xab, 0x12, 0xab, 0x12,
                0xab, 0x12
            ])
        );
        for invalid in [
            vec!["tool", "accept-post-boot"],
            vec!["tool", "accept-post-boot", "--guards-ready"],
            vec![
                "tool",
                "accept-post-boot",
                "--expect-operation",
                "00000000000000000000000000000000",
            ],
            vec!["tool", "accept-post-boot", "--expect-operation", "zz"],
            vec!["tool", "inspect-empty", "--guards-ready"],
        ] {
            assert!(parse_invocation(invalid.into_iter().map(OsString::from)).is_err());
        }
    }

    #[test]
    fn boot_binding_parses_the_kernel_uuid() {
        assert_eq!(
            parse_boot_binding("12ab34cd-56ef-7890-abcd-ef1234567890\n"),
            Some([
                0x12, 0xab, 0x34, 0xcd, 0x56, 0xef, 0x78, 0x90, 0xab, 0xcd, 0xef, 0x12, 0x34, 0x56,
                0x78, 0x90
            ])
        );
        for invalid in [
            "",
            "12ab34cd",
            "not-a-uuid",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert_eq!(parse_boot_binding(invalid), None);
        }
    }

    #[test]
    fn acceptance_requires_the_record_to_close() {
        let operation_id = [0x5a; 16];
        let mut waiting = backend(healthy());
        waiting.acceptance = Ok(AcceptanceOutcome::Waiting { operation_id });
        let invocation = parse_invocation(
            [
                "tool",
                "accept-post-boot",
                "--expect-operation",
                "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
                "--require-accepted",
            ]
            .map(OsString::from),
        )
        .unwrap();
        assert!(matches!(
            run_invocation_with_backend(invocation, &waiting),
            Err(MaintenanceError::NotAccepted)
        ));

        let mut accepted = backend(healthy());
        accepted.acceptance = Ok(AcceptanceOutcome::Accepted {
            operation_id,
            generation: 4,
        });
        let receipt = run_invocation_with_backend(invocation, &accepted).unwrap();
        assert!(receipt.accepted());
        assert_eq!(
            receipt.to_string(),
            "command=accept-post-boot result=accepted operation=5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a generation=4 copies=healthy"
        );

        let mut ambiguous = backend(healthy());
        ambiguous.acceptance = Ok(AcceptanceOutcome::Ambiguous);
        let optional = parse_invocation(
            [
                "tool",
                "accept-post-boot",
                "--expect-operation",
                "5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a",
            ]
            .map(OsString::from),
        )
        .unwrap();
        let receipt = run_invocation_with_backend(optional, &ambiguous).unwrap();
        assert!(!receipt.accepted());
        assert_eq!(
            receipt.to_string(),
            "command=accept-post-boot result=ambiguous copies=healthy"
        );
    }
}
