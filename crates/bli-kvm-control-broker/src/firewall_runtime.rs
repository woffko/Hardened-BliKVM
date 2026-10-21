//! Independently synchronized firewall runtime; nft work must not hold HID state.
use crate::{
    firewall_confirmation::{ConfirmationError, PendingFileStore},
    firewall_executor::{ModeConfiguration, NftModeExecutor},
    firewall_settings::SystemFirewallRunner,
    firewall_transaction::{FirewallTransaction, TransactionError},
};
use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::PlatformControlRequestV2;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// The mode the operator last confirmed, kept across reboots in the owner's
/// state directory; the kernel rules and the pending journal under /run are not.
pub const PERSISTED_MODE_PATH: &str = "/var/lib/hardened-blikvm-gadget/firewall-mode.json";

pub type SharedModeRuntime = Arc<Mutex<ModeRuntime>>;
pub struct ModeRuntime {
    transaction: FirewallTransaction<PendingFileStore, NftModeExecutor>,
    persisted: Option<PathBuf>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PersistedMode {
    mode: bli_kvm_core::FirewallMode,
}

/// Reads the persisted mode; anything but a small, well-formed file is none.
pub fn read_persisted_mode(path: &std::path::Path) -> Option<bli_kvm_core::FirewallMode> {
    let metadata = std::fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > 256 {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<PersistedMode>(&text)
        .ok()
        .map(|stored| stored.mode)
}

fn write_persisted_mode(
    path: &std::path::Path,
    mode: bli_kvm_core::FirewallMode,
) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let text = serde_json::to_string(&PersistedMode { mode }).map_err(|error| error.to_string())?;
    let temporary = path.with_extension("json.new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o644)
        .open(&temporary)
        .map_err(|error| error.to_string())?;
    file.write_all(text.as_bytes())
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_all())
        .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

impl ModeRuntime {
    pub fn open_configured(
        directory: PathBuf,
        path: &std::path::Path,
        boot_id: [u8; 16],
    ) -> Result<Self, String> {
        match ModeConfiguration::read_reviewed(path) {
            Ok(configuration) => Self::open(directory, configuration, boot_id),
            Err(error) => {
                let mut store = PendingFileStore::new(&directory)?;
                if crate::firewall_confirmation::PendingStore::load(&mut store)?.is_some() {
                    Self::open_recovery(directory, boot_id)
                } else {
                    Err(error)
                }
            }
        }
    }
    pub fn open_recovery(directory: PathBuf, boot_id: [u8; 16]) -> Result<Self, String> {
        let executor = NftModeExecutor::reopen_for_recovery(
            Box::new(SystemFirewallRunner),
            directory.clone(),
        )?;
        let transaction =
            FirewallTransaction::open(PendingFileStore::new(&directory)?, executor, boot_id)
                .map_err(|error| format!("{error:?}"))?;
        Ok(Self {
            transaction,
            persisted: None,
        })
    }
    pub fn open(
        directory: PathBuf,
        configuration: ModeConfiguration,
        boot_id: [u8; 16],
    ) -> Result<Self, String> {
        Self::open_with_runner(
            directory,
            configuration,
            boot_id,
            Box::new(SystemFirewallRunner),
        )
    }

    fn open_with_runner(
        directory: PathBuf,
        configuration: ModeConfiguration,
        boot_id: [u8; 16],
        runner: Box<dyn crate::firewall_settings::FirewallRunner>,
    ) -> Result<Self, String> {
        let executor = NftModeExecutor::open_frozen(runner, directory.clone(), configuration)?;
        let transaction =
            FirewallTransaction::open(PendingFileStore::new(&directory)?, executor, boot_id)
                .map_err(|error| format!("{error:?}"))?;
        Ok(Self {
            transaction,
            persisted: None,
        })
    }

    /// Keeps confirmed modes in `path` and, when the kernel holds no owned
    /// table (after a reboot), re-applies the mode stored there. A table that
    /// an earlier build of the owner wrote (after an update without a
    /// reboot) is brought up to this build's rules for its mode.
    pub fn persist_to(&mut self, path: PathBuf, now_ms: u64) {
        match self.transaction.refresh() {
            Ok(Some(mode)) => {
                eprintln!("firewall: the rules of the {mode:?} mode were brought up to date")
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("firewall: the rules in force could not be brought up to date: {error:?}")
            }
        }
        if let Some(mode) = read_persisted_mode(&path) {
            match self.transaction.restore(mode, now_ms) {
                Ok(true) => eprintln!("firewall: restored the confirmed {mode:?} mode"),
                Ok(false) => {}
                Err(error) => {
                    eprintln!("firewall: restoring the confirmed {mode:?} mode failed: {error:?}")
                }
            }
        }
        self.persisted = Some(path);
    }

    fn remember(&self, mode: bli_kvm_core::FirewallMode) -> Result<(), String> {
        match &self.persisted {
            Some(path) => write_persisted_mode(path, mode),
            None => Ok(()),
        }
    }

    /// Puts back the mode persisted before a change that did not happen.
    fn forget(&self, previous: Option<bli_kvm_core::FirewallMode>) {
        let Some(path) = &self.persisted else {
            return;
        };
        let restored = match previous {
            Some(mode) => write_persisted_mode(path, mode),
            None => std::fs::remove_file(path).map_err(|error| error.to_string()),
        };
        if let Err(error) = restored {
            eprintln!("firewall: the persisted mode could not be put back: {error}");
        }
    }

    pub fn tick(&mut self, now_ms: u64) -> Result<(), TransactionError> {
        self.transaction.tick(now_ms)
    }

    pub fn dispatch(&mut self, request: &PlatformControlRequestV2, now_ms: u64) -> ControlResponse {
        if matches!(request, PlatformControlRequestV2::FirewallModeStatus {}) {
            return match self.transaction.status(now_ms) {
                Ok(status) => ControlResponse::FirewallModeState { status },
                Err(_) => reject(ControlRejectionCode::BackendRejected),
            };
        }
        let issued = match request {
            PlatformControlRequestV2::FirewallModeChange { request } => request.issued_at_unix_ms,
            PlatformControlRequestV2::FirewallModeConfirm { request } => request.issued_at_unix_ms,
            _ => return reject(ControlRejectionCode::CapabilityDisabled),
        };
        if request.validate().is_err() || issued > now_ms || now_ms.saturating_sub(issued) > 30_000
        {
            return reject(ControlRejectionCode::InvalidState);
        }
        // A mode is remembered once confirmed: at once when it needs no
        // confirmation, otherwise when the operator confirms it. It is
        // written before the change takes effect, so a mode that a reboot
        // would not keep is refused rather than applied or confirmed (code
        // audit of 2026-10-20, AUD-14).
        let confirmed = match request {
            PlatformControlRequestV2::FirewallModeChange { request } => {
                (!request.mode.requires_confirmation()).then_some(request.mode)
            }
            _ => self
                .transaction
                .pending()
                .map(crate::firewall_confirmation::PendingChange::requested_mode),
        };
        let previous = self.persisted.as_deref().and_then(read_persisted_mode);
        if let Some(mode) = confirmed
            && let Err(error) = self.remember(mode)
        {
            eprintln!("firewall: the {mode:?} mode cannot be persisted: {error}");
            return reject(ControlRejectionCode::BackendRejected);
        }
        let result = match request {
            PlatformControlRequestV2::FirewallModeChange { request } => self.transaction.request(
                request.operation_id,
                request.session_binding,
                request.mode,
                now_ms,
            ),
            PlatformControlRequestV2::FirewallModeConfirm { request } => {
                self.transaction
                    .confirm(request.operation_id, request.session_binding, now_ms)
            }
            _ => unreachable!(),
        };
        if result.is_err() && confirmed.is_some() {
            self.forget(previous);
        }
        match result {
            Ok(()) => ControlResponse::Applied,
            Err(
                TransactionError::RecoveryRequired
                | TransactionError::Journal(
                    ConfirmationError::PendingExists | ConfirmationError::RollbackRequired,
                ),
            ) => reject(ControlRejectionCode::PendingUnreconciled),
            Err(TransactionError::Journal(
                ConfirmationError::WrongOperation
                | ConfirmationError::WrongSession
                | ConfirmationError::InvalidIdentity,
            )) => reject(ControlRejectionCode::InvalidState),
            Err(_) => reject(ControlRejectionCode::BackendRejected),
        }
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}
pub fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn confirmed_modes_persist_and_malformed_files_read_as_none() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("firewall-mode.json");
        assert_eq!(read_persisted_mode(&path), None);
        write_persisted_mode(&path, bli_kvm_core::FirewallMode::Restricted).unwrap();
        assert_eq!(
            read_persisted_mode(&path),
            Some(bli_kvm_core::FirewallMode::Restricted)
        );
        for bad in [
            "{\"mode\":\"open\"}",
            "{\"mode\":\"moderate\",\"x\":1}",
            "junk",
        ] {
            std::fs::write(&path, bad).unwrap();
            assert_eq!(read_persisted_mode(&path), None);
        }
        std::fs::write(&path, " ".repeat(300)).unwrap();
        assert_eq!(read_persisted_mode(&path), None);
    }

    #[test]
    fn missing_launch_configuration_keeps_pending_recovery_available() {
        use crate::firewall_confirmation::{PendingChange, PendingStore};
        let directory = tempfile::tempdir().unwrap();
        let configuration = ModeConfiguration {
            https_probe: None,
            https_enabled: true,
            https_port: 8443,
            syslog_port: 1514,
            backend_isolation: None,
            webrtc_media: None,
        };
        ModeRuntime::open(directory.path().into(), configuration, [3; 16]).unwrap();
        let pending = PendingChange::prepare(
            [1; 16],
            [2; 32],
            [3; 16],
            bli_kvm_core::FirewallMode::Moderate,
            bli_kvm_core::FirewallMode::Restricted,
            100,
        )
        .unwrap();
        PendingFileStore::new(directory.path())
            .unwrap()
            .save(&pending.encode().unwrap())
            .unwrap();
        let runtime = ModeRuntime::open_configured(
            directory.path().into(),
            &directory.path().join("missing-launch.json"),
            [3; 16],
        )
        .unwrap();
        assert!(runtime.transaction.pending().is_some());
    }
    #[test]
    fn a_mode_that_cannot_be_persisted_is_not_applied_or_confirmed() {
        use crate::firewall_executor::tests::{Kernel, configuration};
        use bli_kvm_core::{FirewallChangeRequest, FirewallMode};
        let change = |mode, operation| PlatformControlRequestV2::FirewallModeChange {
            request: FirewallChangeRequest {
                operation_id: [operation; 16],
                session_binding: [2; 32],
                mode,
                issued_at_unix_ms: 100_000,
            },
        };
        let directory = tempfile::tempdir().unwrap();
        // The persisted mode's directory is a regular file: no write works.
        let blocker = directory.path().join("not-a-directory");
        std::fs::write(&blocker, "").unwrap();
        let kernel = Kernel::holding(None);
        let mut runtime = ModeRuntime::open_with_runner(
            directory.path().join("run"),
            configuration(),
            [3; 16],
            Box::new(kernel.clone()),
        )
        .unwrap();
        runtime.persisted = Some(blocker.join("firewall-mode.json"));
        // A mode that needs no confirmation is not applied.
        assert_ne!(
            runtime.dispatch(&change(FirewallMode::Moderate, 1), 100_000),
            ControlResponse::Applied
        );
        assert_eq!(kernel.table(), None);

        // A mode that needs one is not confirmed: its revert stays due.
        use crate::firewall_confirmation::{PendingChange, PendingStore};
        let run = directory.path().join("confirming");
        std::fs::create_dir(&run).unwrap();
        // The first owner freezes the configuration the change is made with.
        ModeRuntime::open_with_runner(
            run.clone(),
            configuration(),
            [3; 16],
            Box::new(Kernel::holding(None)),
        )
        .unwrap();
        let pending = PendingChange::prepare(
            [4; 16],
            [2; 32],
            [3; 16],
            FirewallMode::Moderate,
            FirewallMode::Restricted,
            100_000,
        )
        .unwrap();
        PendingFileStore::new(&run)
            .unwrap()
            .save(&pending.encode().unwrap())
            .unwrap();
        let kernel = Kernel::holding(None);
        let mut runtime =
            ModeRuntime::open_with_runner(run, configuration(), [3; 16], Box::new(kernel)).unwrap();
        assert!(runtime.transaction.pending().is_some());
        runtime.persisted = Some(blocker.join("firewall-mode.json"));
        let confirm = PlatformControlRequestV2::FirewallModeConfirm {
            request: bli_kvm_core::FirewallConfirmRequest {
                operation_id: [4; 16],
                session_binding: [2; 32],
                issued_at_unix_ms: 100_000,
            },
        };
        assert_ne!(
            runtime.dispatch(&confirm, 100_000),
            ControlResponse::Applied
        );
        assert!(runtime.transaction.pending().is_some());
    }

    #[test]
    fn stale_future_and_invalid_requests_are_refused_before_nft() {
        let directory = tempfile::tempdir().unwrap();
        let configuration = ModeConfiguration {
            https_probe: None,
            https_enabled: true,
            https_port: 8443,
            syslog_port: 1514,
            backend_isolation: None,
            webrtc_media: None,
        };
        let mut runtime =
            ModeRuntime::open(directory.path().into(), configuration, [3; 16]).unwrap();
        for issued in [0, 1, 100_001] {
            let request = PlatformControlRequestV2::FirewallModeChange {
                request: bli_kvm_core::FirewallChangeRequest {
                    operation_id: [1; 16],
                    session_binding: [2; 32],
                    mode: bli_kvm_core::FirewallMode::Restricted,
                    issued_at_unix_ms: issued,
                },
            };
            assert_eq!(
                runtime.dispatch(&request, 100_000),
                reject(ControlRejectionCode::InvalidState)
            );
        }
        assert!(!directory.path().join("pending.json").exists());
        assert!(!directory.path().join("mode-candidate.nft").exists());
    }
}
