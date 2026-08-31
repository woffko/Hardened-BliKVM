#![forbid(unsafe_code)]

//! Fixed-path process boundary for the unshipped diagnostic collector.
//!
//! The systemd service owns account and runtime-directory creation. This crate
//! validates that exact directory, binds one fixed Unix socket, and delegates
//! all peer authentication, time, frame I/O, and window state to the verified
//! diagnostic runtime library.

use std::{
    error::Error,
    fmt,
    fs::{self, Permissions},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    thread,
};

use bli_kvm_diagnostic_peer_adapter::{DiagnosticCollectorRuntime, DiagnosticListenerError};
use nix::unistd::{getegid, geteuid};
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use tokio::{net::UnixListener, sync::oneshot};

pub const DIAGNOSTIC_RUNTIME_DIRECTORY: &str = "/run/hardened-blikvm-diagnostics";
pub const DIAGNOSTIC_SOCKET_PATH: &str = "/run/hardened-blikvm-diagnostics/collector.sock";
pub const DIAGNOSTIC_RUNTIME_DIRECTORY_MODE: u32 = 0o750;
pub const DIAGNOSTIC_SOCKET_MODE: u32 = 0o660;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCollectorError {
    InvalidRuntimeDirectory,
    SocketAlreadyExists,
    SocketIo,
    SignalUnavailable,
    Runtime(DiagnosticListenerError),
}

impl fmt::Display for DiagnosticCollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRuntimeDirectory => "diagnostic runtime directory is invalid",
            Self::SocketAlreadyExists => "diagnostic collector socket already exists",
            Self::SocketIo => "diagnostic collector socket I/O failed",
            Self::SignalUnavailable => "diagnostic shutdown signal is unavailable",
            Self::Runtime(_) => "diagnostic listener runtime failed",
        };
        formatter.write_str(message)
    }
}

impl Error for DiagnosticCollectorError {}

impl From<DiagnosticListenerError> for DiagnosticCollectorError {
    fn from(error: DiagnosticListenerError) -> Self {
        Self::Runtime(error)
    }
}

pub async fn run() -> Result<(), DiagnosticCollectorError> {
    let runtime = DiagnosticCollectorRuntime::from_current_process()?;
    let (listener, cleanup) = bind_fixed_listener()?;
    let mut signals =
        Signals::new([SIGTERM, SIGINT]).map_err(|_| DiagnosticCollectorError::SignalUnavailable)?;
    let signal_handle = signals.handle();
    let (shutdown_sender, shutdown) = oneshot::channel();
    let signal_thread = thread::Builder::new()
        .name("diagnostic-signal".into())
        .spawn(move || {
            if signals.forever().next().is_some() {
                let _ = shutdown_sender.send(());
            }
        })
        .map_err(|_| DiagnosticCollectorError::SignalUnavailable)?;
    let result = runtime
        .serve_until(listener, async move {
            let _ = shutdown.await;
        })
        .await
        .map_err(Into::into);
    signal_handle.close();
    let signal_result = signal_thread.join();
    drop(cleanup);
    if signal_result.is_err() && result.is_ok() {
        return Err(DiagnosticCollectorError::SignalUnavailable);
    }
    result
}

fn bind_fixed_listener() -> Result<(UnixListener, SocketCleanup), DiagnosticCollectorError> {
    bind_listener_at(
        Path::new(DIAGNOSTIC_RUNTIME_DIRECTORY),
        Path::new(DIAGNOSTIC_SOCKET_PATH),
        geteuid().as_raw(),
        getegid().as_raw(),
    )
}

fn bind_listener_at(
    runtime_directory: &Path,
    socket_path: &Path,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<(UnixListener, SocketCleanup), DiagnosticCollectorError> {
    validate_runtime_directory(runtime_directory, socket_path, expected_uid, expected_gid)?;
    match fs::symlink_metadata(socket_path) {
        Ok(_) => return Err(DiagnosticCollectorError::SocketAlreadyExists),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(DiagnosticCollectorError::SocketIo),
    }

    let mut cleanup = SocketCleanup::new(socket_path, expected_uid, expected_gid);
    let listener =
        UnixListener::bind(socket_path).map_err(|_| DiagnosticCollectorError::SocketIo)?;
    cleanup.capture_identity()?;
    fs::set_permissions(socket_path, Permissions::from_mode(DIAGNOSTIC_SOCKET_MODE))
        .map_err(|_| DiagnosticCollectorError::SocketIo)?;
    validate_socket(&cleanup)?;
    Ok((listener, cleanup))
}

fn validate_runtime_directory(
    runtime_directory: &Path,
    socket_path: &Path,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<(), DiagnosticCollectorError> {
    if socket_path.parent() != Some(runtime_directory) {
        return Err(DiagnosticCollectorError::InvalidRuntimeDirectory);
    }
    let metadata = fs::symlink_metadata(runtime_directory)
        .map_err(|_| DiagnosticCollectorError::InvalidRuntimeDirectory)?;
    if !metadata.is_dir()
        || metadata.file_type().is_symlink()
        || metadata.file_type().is_socket()
        || metadata.mode() & 0o777 != DIAGNOSTIC_RUNTIME_DIRECTORY_MODE
        || metadata.uid() != expected_uid
        || metadata.gid() != expected_gid
    {
        return Err(DiagnosticCollectorError::InvalidRuntimeDirectory);
    }
    Ok(())
}

fn validate_socket(cleanup: &SocketCleanup) -> Result<(), DiagnosticCollectorError> {
    let metadata =
        fs::symlink_metadata(&cleanup.path).map_err(|_| DiagnosticCollectorError::SocketIo)?;
    if !cleanup.matches(&metadata) || metadata.mode() & 0o777 != DIAGNOSTIC_SOCKET_MODE {
        return Err(DiagnosticCollectorError::SocketIo);
    }
    Ok(())
}

struct SocketCleanup {
    path: PathBuf,
    expected_uid: u32,
    expected_gid: u32,
    identity: Option<(u64, u64)>,
}

impl SocketCleanup {
    fn new(path: &Path, expected_uid: u32, expected_gid: u32) -> Self {
        Self {
            path: path.to_path_buf(),
            expected_uid,
            expected_gid,
            identity: None,
        }
    }

    fn capture_identity(&mut self) -> Result<(), DiagnosticCollectorError> {
        let metadata =
            fs::symlink_metadata(&self.path).map_err(|_| DiagnosticCollectorError::SocketIo)?;
        if !metadata.file_type().is_socket()
            || metadata.uid() != self.expected_uid
            || metadata.gid() != self.expected_gid
        {
            return Err(DiagnosticCollectorError::SocketIo);
        }
        self.identity = Some((metadata.dev(), metadata.ino()));
        Ok(())
    }

    fn matches(&self, metadata: &fs::Metadata) -> bool {
        metadata.file_type().is_socket()
            && metadata.uid() == self.expected_uid
            && metadata.gid() == self.expected_gid
            && self
                .identity
                .is_none_or(|identity| identity == (metadata.dev(), metadata.ino()))
    }
}

impl Drop for SocketCleanup {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.path) {
            if self.matches(&metadata) {
                let _ = fs::remove_file(&self.path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write};

    use super::*;

    #[tokio::test]
    async fn binds_exact_socket_mode_and_removes_only_the_same_inode() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(
            directory.path(),
            Permissions::from_mode(DIAGNOSTIC_RUNTIME_DIRECTORY_MODE),
        )
        .unwrap();
        let socket = directory.path().join("collector.sock");
        let (listener, cleanup) = bind_listener_at(
            directory.path(),
            &socket,
            geteuid().as_raw(),
            getegid().as_raw(),
        )
        .unwrap();
        let metadata = fs::symlink_metadata(&socket).unwrap();
        assert!(metadata.file_type().is_socket());
        assert_eq!(metadata.mode() & 0o777, DIAGNOSTIC_SOCKET_MODE);
        drop(listener);
        drop(cleanup);
        assert!(!socket.exists());
    }

    #[tokio::test]
    async fn cleanup_refuses_a_replaced_non_socket() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(
            directory.path(),
            Permissions::from_mode(DIAGNOSTIC_RUNTIME_DIRECTORY_MODE),
        )
        .unwrap();
        let socket = directory.path().join("collector.sock");
        let (listener, cleanup) = bind_listener_at(
            directory.path(),
            &socket,
            geteuid().as_raw(),
            getegid().as_raw(),
        )
        .unwrap();
        drop(listener);
        fs::remove_file(&socket).unwrap();
        let mut replacement = File::create(&socket).unwrap();
        replacement.write_all(b"replacement").unwrap();
        drop(replacement);
        drop(cleanup);
        assert!(socket.is_file());
    }

    #[test]
    fn rejects_wrong_directory_mode_symlink_and_existing_socket_path() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("collector.sock");
        assert_eq!(
            bind_listener_at(
                directory.path(),
                &socket,
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .err(),
            Some(DiagnosticCollectorError::InvalidRuntimeDirectory)
        );
        fs::set_permissions(
            directory.path(),
            Permissions::from_mode(DIAGNOSTIC_RUNTIME_DIRECTORY_MODE),
        )
        .unwrap();
        File::create(&socket).unwrap();
        assert_eq!(
            bind_listener_at(
                directory.path(),
                &socket,
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .err(),
            Some(DiagnosticCollectorError::SocketAlreadyExists)
        );

        let target = tempfile::tempdir().unwrap();
        fs::set_permissions(
            target.path(),
            Permissions::from_mode(DIAGNOSTIC_RUNTIME_DIRECTORY_MODE),
        )
        .unwrap();
        let parent = directory.path().join("linked");
        std::os::unix::fs::symlink(target.path(), &parent).unwrap();
        assert_eq!(
            bind_listener_at(
                &parent,
                &parent.join("collector.sock"),
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .err(),
            Some(DiagnosticCollectorError::InvalidRuntimeDirectory)
        );
    }
}
