#![forbid(unsafe_code)]

//! Fixed-path process boundary for the unshipped diagnostic collector.
//!
//! The systemd service owns account and runtime-directory creation. This crate
//! validates that exact directory, atomically binds fixed producer and export
//! Unix sockets, and delegates peer authentication, bounded I/O, time, and
//! shared window state to the verified runtime/listener libraries.

use std::{
    error::Error,
    fmt,
    fs::{self, Permissions},
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    thread,
};

use bli_kvm_diagnostic_export_listener::{DiagnosticExportListener, DiagnosticExportListenerError};
use bli_kvm_diagnostic_peer_adapter::{DiagnosticCollectorRuntime, DiagnosticListenerError};
use nix::unistd::{getegid, geteuid};
use signal_hook::{
    consts::signal::{SIGINT, SIGTERM},
    iterator::Signals,
};
use tokio::{net::UnixListener, sync::watch, task::JoinError};

pub const DIAGNOSTIC_RUNTIME_DIRECTORY: &str = "/run/hardened-blikvm-diagnostics";
pub const DIAGNOSTIC_SOCKET_PATH: &str = "/run/hardened-blikvm-diagnostics/collector.sock";
pub const DIAGNOSTIC_EXPORT_SOCKET_PATH: &str = "/run/hardened-blikvm-diagnostics/export.sock";
pub const DIAGNOSTIC_RUNTIME_DIRECTORY_MODE: u32 = 0o750;
pub const DIAGNOSTIC_SOCKET_MODE: u32 = 0o660;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticCollectorError {
    InvalidRuntimeDirectory,
    SocketAlreadyExists,
    SocketIo,
    SignalUnavailable,
    Runtime(DiagnosticListenerError),
    ExportRuntime(DiagnosticExportListenerError),
    RuntimeTaskFailed,
}

impl fmt::Display for DiagnosticCollectorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::InvalidRuntimeDirectory => "diagnostic runtime directory is invalid",
            Self::SocketAlreadyExists => "diagnostic collector socket already exists",
            Self::SocketIo => "diagnostic collector socket I/O failed",
            Self::SignalUnavailable => "diagnostic shutdown signal is unavailable",
            Self::Runtime(_) => "diagnostic listener runtime failed",
            Self::ExportRuntime(_) => "diagnostic export listener runtime failed",
            Self::RuntimeTaskFailed => "diagnostic runtime task failed",
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

impl From<DiagnosticExportListenerError> for DiagnosticCollectorError {
    fn from(error: DiagnosticExportListenerError) -> Self {
        Self::ExportRuntime(error)
    }
}

pub async fn run() -> Result<(), DiagnosticCollectorError> {
    let runtime = Arc::new(DiagnosticCollectorRuntime::from_current_process()?);
    let export = Arc::new(DiagnosticExportListener::from_current_process(Arc::clone(
        &runtime,
    ))?);
    let (producer_listener, producer_cleanup, export_listener, export_cleanup) =
        bind_fixed_listeners()?;
    let mut signals =
        Signals::new([SIGTERM, SIGINT]).map_err(|_| DiagnosticCollectorError::SignalUnavailable)?;
    let signal_handle = signals.handle();
    let (shutdown_sender, shutdown) = watch::channel(false);
    let signal_shutdown = shutdown_sender.clone();
    let signal_thread = thread::Builder::new()
        .name("diagnostic-signal".into())
        .spawn(move || {
            if signals.forever().next().is_some() {
                let _ = signal_shutdown.send(true);
            }
        })
        .map_err(|_| DiagnosticCollectorError::SignalUnavailable)?;

    let producer_shutdown = shutdown.clone();
    let producer_done = shutdown_sender.clone();
    let producer_runtime = Arc::clone(&runtime);
    let producer_task = tokio::spawn(async move {
        let _shutdown_on_drop = ShutdownOnDrop(producer_done);
        producer_runtime
            .serve_until(producer_listener, wait_for_shutdown(producer_shutdown))
            .await
    });
    let export_done = shutdown_sender.clone();
    let export_task = tokio::spawn(async move {
        let _shutdown_on_drop = ShutdownOnDrop(export_done);
        export
            .serve_until(export_listener, wait_for_shutdown(shutdown))
            .await
    });

    let (producer_result, export_result) = tokio::join!(producer_task, export_task);
    let result = combine_runtime_results(producer_result, export_result);
    signal_handle.close();
    let signal_result = signal_thread.join();
    drop(export_cleanup);
    drop(producer_cleanup);
    if signal_result.is_err() && result.is_ok() {
        return Err(DiagnosticCollectorError::SignalUnavailable);
    }
    result
}

async fn wait_for_shutdown(mut shutdown: watch::Receiver<bool>) {
    while !*shutdown.borrow() {
        if shutdown.changed().await.is_err() {
            break;
        }
    }
}

struct ShutdownOnDrop(watch::Sender<bool>);

impl Drop for ShutdownOnDrop {
    fn drop(&mut self) {
        let _ = self.0.send(true);
    }
}

fn combine_runtime_results(
    producer: Result<Result<(), DiagnosticListenerError>, JoinError>,
    export: Result<Result<(), DiagnosticExportListenerError>, JoinError>,
) -> Result<(), DiagnosticCollectorError> {
    let producer = producer.map_err(|_| DiagnosticCollectorError::RuntimeTaskFailed)?;
    let export = export.map_err(|_| DiagnosticCollectorError::RuntimeTaskFailed)?;
    producer.map_err(DiagnosticCollectorError::Runtime)?;
    export.map_err(DiagnosticCollectorError::ExportRuntime)
}

fn bind_fixed_listeners()
-> Result<(UnixListener, SocketCleanup, UnixListener, SocketCleanup), DiagnosticCollectorError> {
    bind_listener_pair_at(
        Path::new(DIAGNOSTIC_RUNTIME_DIRECTORY),
        Path::new(DIAGNOSTIC_SOCKET_PATH),
        Path::new(DIAGNOSTIC_EXPORT_SOCKET_PATH),
        geteuid().as_raw(),
        getegid().as_raw(),
    )
}

fn bind_listener_pair_at(
    runtime_directory: &Path,
    producer_path: &Path,
    export_path: &Path,
    expected_uid: u32,
    expected_gid: u32,
) -> Result<(UnixListener, SocketCleanup, UnixListener, SocketCleanup), DiagnosticCollectorError> {
    if producer_path == export_path {
        return Err(DiagnosticCollectorError::SocketAlreadyExists);
    }
    let (producer, producer_cleanup) =
        bind_listener_at(runtime_directory, producer_path, expected_uid, expected_gid)?;
    let (export, export_cleanup) =
        bind_listener_at(runtime_directory, export_path, expected_uid, expected_gid)?;
    Ok((producer, producer_cleanup, export, export_cleanup))
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
    async fn dual_socket_bind_is_atomic_and_cleanup_is_exact() {
        let directory = tempfile::tempdir().unwrap();
        fs::set_permissions(
            directory.path(),
            Permissions::from_mode(DIAGNOSTIC_RUNTIME_DIRECTORY_MODE),
        )
        .unwrap();
        let producer = directory.path().join("collector.sock");
        let export = directory.path().join("export.sock");
        let (producer_listener, producer_cleanup, export_listener, export_cleanup) =
            bind_listener_pair_at(
                directory.path(),
                &producer,
                &export,
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .unwrap();
        for socket in [&producer, &export] {
            let metadata = fs::symlink_metadata(socket).unwrap();
            assert!(metadata.file_type().is_socket());
            assert_eq!(metadata.mode() & 0o777, DIAGNOSTIC_SOCKET_MODE);
        }
        drop(export_listener);
        drop(export_cleanup);
        drop(producer_listener);
        drop(producer_cleanup);
        assert!(!producer.exists());
        assert!(!export.exists());

        File::create(&export).unwrap();
        assert_eq!(
            bind_listener_pair_at(
                directory.path(),
                &producer,
                &export,
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .err(),
            Some(DiagnosticCollectorError::SocketAlreadyExists)
        );
        assert!(!producer.exists());
        assert!(export.is_file());
        assert_eq!(
            bind_listener_pair_at(
                directory.path(),
                &producer,
                &producer,
                geteuid().as_raw(),
                getegid().as_raw(),
            )
            .err(),
            Some(DiagnosticCollectorError::SocketAlreadyExists)
        );
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

    #[tokio::test]
    async fn shutdown_broadcast_and_runtime_errors_are_typed() {
        let (sender, receiver) = watch::channel(false);
        let waiting = tokio::spawn(wait_for_shutdown(receiver));
        drop(ShutdownOnDrop(sender));
        waiting.await.unwrap();

        let (sender, receiver) = watch::channel(false);
        let waiting = tokio::spawn(wait_for_shutdown(receiver));
        let panicked = tokio::spawn(async move {
            let _shutdown_on_drop = ShutdownOnDrop(sender);
            panic!("test runtime panic");
        });
        assert!(panicked.await.unwrap_err().is_panic());
        waiting.await.unwrap();

        assert!(matches!(
            combine_runtime_results(Ok(Err(DiagnosticListenerError::ClockOverflow)), Ok(Ok(()))),
            Err(DiagnosticCollectorError::Runtime(
                DiagnosticListenerError::ClockOverflow
            ))
        ));
        assert!(matches!(
            combine_runtime_results(
                Ok(Ok(())),
                Ok(Err(DiagnosticExportListenerError::AlreadyStarted))
            ),
            Err(DiagnosticCollectorError::ExportRuntime(
                DiagnosticExportListenerError::AlreadyStarted
            ))
        ));
    }
}
