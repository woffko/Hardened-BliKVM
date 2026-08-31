use std::{ffi::OsString, fmt};

use bli_kvm_system_action_store::{
    SystemActionPolicyState, SystemActionStore, SystemActionStoreError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceCommand {
    InitializeEmpty,
    InspectEmpty,
}

impl MaintenanceCommand {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InitializeEmpty => "initialize-empty",
            Self::InspectEmpty => "inspect-empty",
        }
    }
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

impl MaintenanceBackend for ExactStoreBackend {
    fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError> {
        self.store
            .initialize(SystemActionPolicyState::empty())
            .map(|persisted| persisted.generation)
            .map_err(|_| MaintenanceBackendError::OperationRejected)
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
}

impl MaintenanceReceipt {
    pub const fn command(self) -> MaintenanceCommand {
        self.command
    }
}

impl fmt::Display for MaintenanceReceipt {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "command={} result=empty generation=1 copies=healthy",
            self.command.as_str()
        )
    }
}

#[derive(Debug)]
pub enum MaintenanceError {
    InvalidArguments,
    Store(SystemActionStoreError),
    Backend,
    UnexpectedState,
}

impl MaintenanceError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::Store(_) => 10,
            Self::Backend => 11,
            Self::UnexpectedState => 12,
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
    let mut arguments = arguments.into_iter();
    let _program = arguments.next().ok_or(MaintenanceError::InvalidArguments)?;
    let command = match arguments.next().as_deref() {
        Some(value) if value == "initialize-empty" => MaintenanceCommand::InitializeEmpty,
        Some(value) if value == "inspect-empty" => MaintenanceCommand::InspectEmpty,
        _ => return Err(MaintenanceError::InvalidArguments),
    };
    if arguments.next().is_some() {
        return Err(MaintenanceError::InvalidArguments);
    }
    Ok(command)
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
    Ok(MaintenanceReceipt { command })
}

pub fn run_exact(command: MaintenanceCommand) -> Result<MaintenanceReceipt, MaintenanceError> {
    let backend = ExactStoreBackend::open()?;
    run_with_backend(command, &backend)
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
    }

    impl MaintenanceBackend for FakeBackend {
        fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError> {
            self.initialized.set(self.initialized.get() + 1);
            self.initialize_generation
        }

        fn inspect(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError> {
            self.snapshot
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
        };
        assert!(matches!(
            run_with_backend(MaintenanceCommand::InitializeEmpty, &wrong),
            Err(MaintenanceError::UnexpectedState)
        ));
    }
}
