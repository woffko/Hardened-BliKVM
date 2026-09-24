//! Root execution helper for native application compositions.
//!
//! Started per connection by a socket unit with `Accept=yes`; the connected
//! stream is standard input. The helper authenticates the coordinator UID,
//! then serves one bounded session on the protected execution store. It never
//! opens the update manager, journal or health credentials.

mod config;

use std::{
    ffi::OsString,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::net::UnixStream,
    },
    path::PathBuf,
    process::ExitCode,
};

use bli_kvm_update_runtime::{
    native_exec_server::{ExecutionSession, SystemdExecutionBackend, peer_uid, serve_connection},
    native_execution::NativeExecutionStore,
    native_selection::SelectionLedgerStore,
};
use bli_kvm_update_stager::ApplicationStageError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum HelperError {
    #[error("invalid arguments")]
    InvalidArguments,
    #[error("configuration path is invalid")]
    InvalidConfigPath,
    #[error("configuration file is unsafe")]
    UnsafeConfigFile,
    #[error("configuration is invalid")]
    InvalidConfig,
    #[error("connected peer is not the configured coordinator")]
    PeerDenied,
    #[error("protected execution store is unavailable")]
    Store(#[from] ApplicationStageError),
    #[error("session I/O failed")]
    Io(#[from] std::io::Error),
}

impl HelperError {
    fn exit_code(&self) -> u8 {
        match self {
            Self::InvalidArguments => 2,
            Self::InvalidConfigPath => 10,
            Self::UnsafeConfigFile => 11,
            Self::InvalidConfig => 12,
            Self::PeerDenied => 77,
            Self::Store(ApplicationStageError::StagerBusy) => 66,
            Self::Store(_) => 20,
            Self::Io(_) => 30,
        }
    }
}

fn parse_arguments(arguments: &[OsString]) -> Result<PathBuf, HelperError> {
    match arguments {
        [flag, path, command] if flag == "--config" && command == "serve-stdin" => {
            Ok(PathBuf::from(path))
        }
        _ => Err(HelperError::InvalidArguments),
    }
}

fn run(arguments: &[OsString]) -> Result<(), HelperError> {
    let config = config::load(&parse_arguments(arguments)?, true)?;
    // SAFETY: dup of the inherited standard input; the new descriptor is owned
    // exclusively by the stream below and closed when it drops.
    let descriptor = unsafe { libc::fcntl(0, libc::F_DUPFD_CLOEXEC, 3) };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    // SAFETY: fcntl returned a fresh descriptor owned by nobody else.
    let stream = UnixStream::from(unsafe { OwnedFd::from_raw_fd(descriptor) });
    // Refuse other peers before touching the protected store.
    if peer_uid(&stream)? != config.coordinator_uid {
        return Err(HelperError::PeerDenied);
    }
    let mut store =
        NativeExecutionStore::open(&config.execution_root, config.trust, config.policy)?;
    let ledger = SelectionLedgerStore::open(&config.ledger_root)?;
    let probes = config.probes;
    let updater = config.updater_slot_root;
    serve_connection(stream, config.coordinator_uid, |deadline| {
        ExecutionSession::begin(
            &mut store,
            &ledger,
            &updater,
            SystemdExecutionBackend::new(probes, deadline),
            deadline,
        )
    })?;
    Ok(())
}

fn main() -> ExitCode {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    match run(&arguments) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("bli-kvm-update-exec-helper: {error}");
            ExitCode::from(error.exit_code())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_closed() {
        let args = |values: &[&str]| values.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            parse_arguments(&args(&["--config", "/etc/x.json", "serve-stdin"])).unwrap(),
            PathBuf::from("/etc/x.json")
        );
        for invalid in [
            &["--config", "/etc/x.json"][..],
            &["--config", "/etc/x.json", "serve"],
            &["--cfg", "/etc/x.json", "serve-stdin"],
            &["--config", "/etc/x.json", "serve-stdin", "extra"],
        ] {
            assert!(matches!(
                parse_arguments(&args(invalid)),
                Err(HelperError::InvalidArguments)
            ));
        }
    }
}
