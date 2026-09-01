//! Empty-only maintenance boundary for the encrypted diagnostic store.
//!
//! The public command parser accepts exactly `initialize-empty` or
//! `inspect-empty`. Production opens only the fixed systemd credential and
//! fixed store root through their dedicated crates. There is no path, key,
//! floor, repair, update, acknowledgement, collector, network, HTTP, UI,
//! shell, process-control, or generic maintenance surface.

use std::{ffi::OsString, fmt};

use bli_kvm_diagnostic_key_loader::{
    LoadedDiagnosticJournalKeys, load_systemd_diagnostic_journal_keys,
};
use bli_kvm_diagnostic_store::DiagnosticJournalStore;

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
    pub entry_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceBackendError {
    OperationRejected,
}

pub trait MaintenanceBackend {
    fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError>;
    fn inspect_empty(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError>;
}

/// Exact production backend. It intentionally implements neither `Clone` nor
/// `Debug` because it owns the loaded key material.
pub struct ExactStoreBackend {
    keys: LoadedDiagnosticJournalKeys,
    store: DiagnosticJournalStore,
}

impl ExactStoreBackend {
    pub fn open() -> Result<Self, MaintenanceError> {
        let keys = load_systemd_diagnostic_journal_keys()
            .map_err(|_| MaintenanceError::CredentialUnavailable)?;
        let store =
            DiagnosticJournalStore::open().map_err(|_| MaintenanceError::StoreUnavailable)?;
        Ok(Self { keys, store })
    }
}

impl MaintenanceBackend for ExactStoreBackend {
    fn initialize_empty(&self) -> Result<u64, MaintenanceBackendError> {
        self.store
            .initialize(&self.keys.key_ring())
            .map(|persisted| persisted.generation)
            .map_err(|_| MaintenanceBackendError::OperationRejected)
    }

    fn inspect_empty(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError> {
        self.store
            .load_read_only(&self.keys.key_ring(), 1)
            .map(|selected| MaintenanceSnapshot {
                generation: selected.generation(),
                degraded: selected.degraded(),
                entry_count: selected.journal().entry_count(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceError {
    InvalidArguments,
    CredentialUnavailable,
    StoreUnavailable,
    Backend,
    UnexpectedState,
}

impl MaintenanceError {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::CredentialUnavailable => 10,
            Self::StoreUnavailable => 11,
            Self::Backend => 12,
            Self::UnexpectedState => 13,
        }
    }
}

impl fmt::Display for MaintenanceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidArguments => "arguments are invalid",
            Self::CredentialUnavailable => "diagnostic credential is unavailable",
            Self::StoreUnavailable => "exact diagnostic store is unavailable",
            Self::Backend => "diagnostic maintenance operation failed",
            Self::UnexpectedState => "diagnostic store is not empty and healthy",
        })
    }
}

impl std::error::Error for MaintenanceError {}

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
    let snapshot = backend
        .inspect_empty()
        .map_err(|_| MaintenanceError::Backend)?;
    if snapshot.generation != 1 || snapshot.degraded || snapshot.entry_count != 0 {
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

        fn inspect_empty(&self) -> Result<MaintenanceSnapshot, MaintenanceBackendError> {
            self.snapshot
        }
    }

    fn healthy() -> MaintenanceSnapshot {
        MaintenanceSnapshot {
            generation: 1,
            degraded: false,
            entry_count: 0,
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
            vec!["tool", "rotate-key"],
            vec!["tool", "acknowledge"],
            vec!["tool", "inspect-empty", "extra"],
            vec!["tool", "initialize-empty", "/tmp/store"],
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
                entry_count: 1,
                ..healthy()
            },
        ] {
            let backend = backend(snapshot);
            assert_eq!(
                run_with_backend(MaintenanceCommand::InspectEmpty, &backend).err(),
                Some(MaintenanceError::UnexpectedState)
            );
        }
    }

    #[test]
    fn backend_failures_and_wrong_initial_generation_are_stable() {
        let failing = FakeBackend {
            initialized: Cell::new(0),
            initialize_generation: Err(MaintenanceBackendError::OperationRejected),
            snapshot: Err(MaintenanceBackendError::OperationRejected),
        };
        assert_eq!(
            run_with_backend(MaintenanceCommand::InitializeEmpty, &failing).err(),
            Some(MaintenanceError::Backend)
        );
        let wrong = FakeBackend {
            initialized: Cell::new(0),
            initialize_generation: Ok(2),
            snapshot: Ok(healthy()),
        };
        assert_eq!(
            run_with_backend(MaintenanceCommand::InitializeEmpty, &wrong).err(),
            Some(MaintenanceError::UnexpectedState)
        );
    }

    #[test]
    fn errors_and_receipts_are_fixed_and_redacted() {
        for error in [
            MaintenanceError::InvalidArguments,
            MaintenanceError::CredentialUnavailable,
            MaintenanceError::StoreUnavailable,
            MaintenanceError::Backend,
            MaintenanceError::UnexpectedState,
        ] {
            let display = error.to_string();
            let debug = format!("{error:?}");
            for forbidden in [
                "/var/",
                "/run/",
                "diagnostic-journal-keyring",
                "key_id",
                "uid",
                "gid",
                "nonce",
            ] {
                assert!(!display.contains(forbidden));
                assert!(!debug.contains(forbidden));
            }
        }
    }
}
