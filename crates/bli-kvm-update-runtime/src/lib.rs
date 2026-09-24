pub mod board_readiness;
mod challenge;
pub mod composition;
pub mod composition_systemd;
mod kernel;
pub mod native_execution;
pub mod native_identity;
pub mod native_ipc;
pub mod owner_readiness;
pub mod process_identity;
pub mod proxy_readiness;
mod supervisor;
pub mod web_probe;

use std::io;

pub use challenge::{
    CONFIRMED_APPLY_FILE_NAME, ChallengePresence, ConfirmedApplyRequest, HealthChallengeStatus,
    HealthChallengeStore, IssuedHealthChallenge, MAX_APPLY_REQUEST_BYTES, VerifiedHealthResponse,
};
pub use kernel::LinuxKernelEvidence;
pub use supervisor::{
    ChallengeReconciliation, NativeBootOutcome, SupervisorActivation, SupervisorAdmission,
    SupervisorRecovery, UpdateSupervisor,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum UpdateRuntimeError {
    #[error("native application transition failed: {0}")]
    Composition(#[from] composition::CompositionError),
    #[error("verified slot lacks the complete native application payload")]
    IncompleteNativePayload,
    #[error("activation confirmation refers to an outdated journal generation")]
    StaleActivationGeneration,
    #[error("activation target is already selected")]
    ActivationTargetAlreadyActive,
    #[error("confirmed service apply request is invalid")]
    InvalidApplyRequest,
    #[error("kernel boot identity is unavailable or invalid")]
    InvalidBootIdentity,
    #[error("kernel boot identity changed while evidence was sampled")]
    BootIdentityChanged,
    #[error("kernel boot-time clock is unavailable or invalid")]
    InvalidBootClock,
    #[error("health runtime root path is invalid")]
    InvalidRuntimeRoot,
    #[error("health runtime root is unsafe")]
    UnsafeRuntimeRoot,
    #[error("health runtime is owned by another supervisor")]
    RuntimeBusy,
    #[error("health challenge state is incomplete, unsafe, or invalid")]
    InvalidChallengeState,
    #[error("a health challenge already exists")]
    ChallengeExists,
    #[error("no health challenge exists")]
    ChallengeMissing,
    #[error("health challenge operation does not match")]
    OperationMismatch,
    #[error("health challenge belongs to another boot")]
    BootMismatch,
    #[error("health challenge has expired")]
    ChallengeExpired,
    #[error("health response is invalid")]
    InvalidHealthResponse,
    #[error("operating-system entropy is unavailable")]
    EntropyUnavailable,
    #[error("health challenge metadata serialization failed")]
    Serialization,
    #[error("health runtime I/O failed")]
    Io(#[from] io::Error),
    #[error("application update manager rejected the runtime operation")]
    Manager(#[from] bli_kvm_update_manager::UpdateManagerError),
    #[error("health response must contain exactly 32 raw bytes")]
    InvalidHealthResponseLength,
    #[error("health challenge and persisted manager state disagree")]
    SupervisorStateMismatch,
}
