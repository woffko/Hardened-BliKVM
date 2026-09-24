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

pub type SharedModeRuntime = Arc<Mutex<ModeRuntime>>;
pub struct ModeRuntime {
    transaction: FirewallTransaction<PendingFileStore, NftModeExecutor>,
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
        Ok(Self { transaction })
    }
    pub fn open(
        directory: PathBuf,
        configuration: ModeConfiguration,
        boot_id: [u8; 16],
    ) -> Result<Self, String> {
        let executor = NftModeExecutor::open_frozen(
            Box::new(SystemFirewallRunner),
            directory.clone(),
            configuration,
        )?;
        let transaction =
            FirewallTransaction::open(PendingFileStore::new(&directory)?, executor, boot_id)
                .map_err(|error| format!("{error:?}"))?;
        Ok(Self { transaction })
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
