use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{AtxControlAction, HidReport, VirtualMediaCommand};

pub const HID_LEASE_MAX_TTL_SECONDS: u16 = 30;
pub const TARGET_ATTESTATION_MIN_TTL_SECONDS: u16 = 5;
pub const TARGET_ATTESTATION_MAX_TTL_SECONDS: u16 = 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetAttestation {
    pub session_binding: u64,
    pub target_binding: [u8; 32],
    pub focus_binding: [u8; 16],
    pub ttl_seconds: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TargetRelease {
    pub session_binding: u64,
    pub target_binding: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum HidLeaseRequest {
    Acquire {
        session_binding: u64,
        target_binding: [u8; 32],
        ttl_seconds: u16,
    },
    Renew {
        lease_id: [u8; 16],
        session_binding: u64,
        target_binding: [u8; 32],
        ttl_seconds: u16,
    },
    Report {
        lease_id: [u8; 16],
        session_binding: u64,
        target_binding: [u8; 32],
        report: HidReport,
    },
    Release {
        lease_id: [u8; 16],
        session_binding: u64,
        target_binding: [u8; 32],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AtxConfirmation {
    pub session_binding: u64,
    pub confirmation_binding: [u8; 32],
    pub confirmed_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaRequest {
    pub command: VirtualMediaCommand,
    pub session_binding: u64,
    pub target_binding: [u8; 32],
    pub issued_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MediaObserveRequest {
    pub session_binding: u64,
    pub target_binding: [u8; 32],
    pub issued_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemActionKind {
    Reboot,
    Shutdown,
}

/// Operator confirmation for a device-wide system action. The origin boot
/// binding is device-derived, so a request prepared before a reboot cannot be
/// replayed after it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SystemActionWireRequest {
    pub action: SystemActionKind,
    pub session_binding: u64,
    pub confirmation_binding: [u8; 32],
    pub confirmed_at_unix_ms: u64,
}

impl SystemActionWireRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        if self.confirmation_binding.iter().all(|byte| *byte == 0) {
            return Err(ControlV2ValidationError::InvalidConfirmationBinding);
        }
        if self.confirmed_at_unix_ms == 0 {
            return Err(ControlV2ValidationError::InvalidConfirmationTimestamp);
        }
        Ok(())
    }
}

/// One bounded network write. The owner validates the target content itself;
/// this type only carries the reviewed confirmation bindings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkWriteRequest {
    pub target: NetworkWriteTarget,
    pub session_binding: u64,
    pub confirmation_binding: [u8; 32],
    pub issued_at_unix_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NetworkWriteTarget {
    Dns {
        servers: Vec<String>,
    },
    Ipv6 {
        interface: String,
        disabled: bool,
    },
    /// One Wake-on-LAN magic packet for the given MAC address.
    Wol {
        mac: String,
    },
}

impl NetworkWriteRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        if self.confirmation_binding.iter().all(|byte| *byte == 0) {
            return Err(ControlV2ValidationError::InvalidConfirmationBinding);
        }
        if self.issued_at_unix_ms == 0 {
            return Err(ControlV2ValidationError::InvalidConfirmationTimestamp);
        }
        Ok(())
    }
}

/// Operator input for the post-boot acceptance: whether the reviewed guards are
/// idle again. The boot identity and the uptime come from the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AcceptPostBootRequest {
    pub guards_ready: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum PlatformControlRequestV2 {
    TargetAttest {
        attestation: TargetAttestation,
    },
    TargetRelease {
        release: TargetRelease,
    },
    HidLease {
        lease: HidLeaseRequest,
    },
    Atx {
        action: AtxControlAction,
        confirmation: AtxConfirmation,
    },
    VirtualMedia {
        request: MediaRequest,
    },
    VirtualMediaObserve {
        request: MediaObserveRequest,
    },
    SystemAction {
        request: SystemActionWireRequest,
    },
    NetworkWrite {
        request: NetworkWriteRequest,
    },
    /// Read-only board status from the daemon that owns the board lines.
    BoardStatus {},
    /// Reviewed post-boot acceptance: only the owner holds the journal, so the
    /// operator attests the guards and the owner closes the pending record.
    AcceptPostBoot {
        request: AcceptPostBootRequest,
    },
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ControlV2ValidationError {
    #[error("session binding is invalid")]
    InvalidSessionBinding,
    #[error("target binding is invalid")]
    InvalidTargetBinding,
    #[error("focus binding is invalid")]
    InvalidFocusBinding,
    #[error("lease ID is invalid")]
    InvalidLeaseId,
    #[error("target attestation TTL is invalid")]
    InvalidAttestationTtl,
    #[error("HID lease TTL is invalid")]
    InvalidLeaseTtl,
    #[error("ATX confirmation binding is invalid")]
    InvalidConfirmationBinding,
    #[error("ATX confirmation timestamp is invalid")]
    InvalidConfirmationTimestamp,
    #[error("HID lease report is invalid")]
    InvalidHidReport,
    #[error("virtual-media command is invalid")]
    InvalidMediaCommand,
    #[error("virtual-media request timestamp is invalid")]
    InvalidMediaTimestamp,
}

impl PlatformControlRequestV2 {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        match self {
            Self::TargetAttest { attestation } => attestation.validate(),
            Self::TargetRelease { release } => release.validate(),
            Self::HidLease { lease } => lease.validate(),
            Self::Atx { confirmation, .. } => confirmation.validate(),
            Self::VirtualMedia { request } => request.validate(),
            Self::VirtualMediaObserve { request } => request.validate(),
            Self::SystemAction { request } => request.validate(),
            Self::NetworkWrite { request } => request.validate(),
            Self::BoardStatus {} => Ok(()),
            Self::AcceptPostBoot { .. } => Ok(()),
        }
    }
}

impl TargetAttestation {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        validate_target_binding(&self.target_binding)?;
        if self.focus_binding.iter().all(|byte| *byte == 0) {
            return Err(ControlV2ValidationError::InvalidFocusBinding);
        }
        if !(TARGET_ATTESTATION_MIN_TTL_SECONDS..=TARGET_ATTESTATION_MAX_TTL_SECONDS)
            .contains(&self.ttl_seconds)
        {
            return Err(ControlV2ValidationError::InvalidAttestationTtl);
        }
        Ok(())
    }
}

impl TargetRelease {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        validate_target_binding(&self.target_binding)
    }
}

impl HidLeaseRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        match self {
            Self::Acquire {
                session_binding,
                target_binding,
                ttl_seconds,
            } => {
                validate_session_binding(*session_binding)?;
                validate_target_binding(target_binding)?;
                validate_lease_ttl(*ttl_seconds)
            }
            Self::Renew {
                lease_id,
                session_binding,
                target_binding,
                ttl_seconds,
            } => {
                validate_lease_id(lease_id)?;
                validate_session_binding(*session_binding)?;
                validate_target_binding(target_binding)?;
                validate_lease_ttl(*ttl_seconds)
            }
            Self::Report {
                lease_id,
                session_binding,
                target_binding,
                report,
            } => {
                validate_lease_id(lease_id)?;
                validate_session_binding(*session_binding)?;
                validate_target_binding(target_binding)?;
                report
                    .validate()
                    .map_err(|_| ControlV2ValidationError::InvalidHidReport)
            }
            Self::Release {
                lease_id,
                session_binding,
                target_binding,
            } => {
                validate_lease_id(lease_id)?;
                validate_session_binding(*session_binding)?;
                validate_target_binding(target_binding)
            }
        }
    }
}

impl AtxConfirmation {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        if self.confirmation_binding.iter().all(|byte| *byte == 0) {
            return Err(ControlV2ValidationError::InvalidConfirmationBinding);
        }
        if self.confirmed_at_unix_ms == 0 {
            return Err(ControlV2ValidationError::InvalidConfirmationTimestamp);
        }
        Ok(())
    }
}

impl MediaRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        validate_target_binding(&self.target_binding)?;
        if self.issued_at_unix_ms == 0 {
            return Err(ControlV2ValidationError::InvalidMediaTimestamp);
        }
        self.command
            .validate()
            .map_err(|_| ControlV2ValidationError::InvalidMediaCommand)
    }
}

impl MediaObserveRequest {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        validate_session_binding(self.session_binding)?;
        validate_target_binding(&self.target_binding)?;
        if self.issued_at_unix_ms == 0 {
            return Err(ControlV2ValidationError::InvalidMediaTimestamp);
        }
        Ok(())
    }
}

fn validate_session_binding(session_binding: u64) -> Result<(), ControlV2ValidationError> {
    if session_binding == 0 {
        Err(ControlV2ValidationError::InvalidSessionBinding)
    } else {
        Ok(())
    }
}

fn validate_target_binding(target_binding: &[u8; 32]) -> Result<(), ControlV2ValidationError> {
    if target_binding.iter().all(|byte| *byte == 0) {
        Err(ControlV2ValidationError::InvalidTargetBinding)
    } else {
        Ok(())
    }
}

fn validate_lease_id(lease_id: &[u8; 16]) -> Result<(), ControlV2ValidationError> {
    if lease_id.iter().all(|byte| *byte == 0) {
        Err(ControlV2ValidationError::InvalidLeaseId)
    } else {
        Ok(())
    }
}

fn validate_lease_ttl(ttl_seconds: u16) -> Result<(), ControlV2ValidationError> {
    if (1..=HID_LEASE_MAX_TTL_SECONDS).contains(&ttl_seconds) {
        Ok(())
    } else {
        Err(ControlV2ValidationError::InvalidLeaseTtl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: [u8; 32] = [0x11; 32];
    const FOCUS: [u8; 16] = [0x22; 16];
    const LEASE: [u8; 16] = [0x33; 16];
    const CONFIRMATION: [u8; 32] = [0x44; 32];

    #[test]
    fn media_request_validation_is_bounded() {
        let request = MediaRequest {
            command: VirtualMediaCommand::SetConnected { connected: true },
            session_binding: 9,
            target_binding: TARGET,
            issued_at_unix_ms: 1_700,
        };
        assert_eq!(request.validate(), Ok(()));
        assert_eq!(
            MediaRequest {
                session_binding: 0,
                ..request.clone()
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidSessionBinding)
        );
        assert_eq!(
            MediaRequest {
                target_binding: [0; 32],
                ..request.clone()
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidTargetBinding)
        );
        assert_eq!(
            MediaRequest {
                issued_at_unix_ms: 0,
                ..request.clone()
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidMediaTimestamp)
        );
        assert!(
            PlatformControlRequestV2::VirtualMedia {
                request: request.clone()
            }
            .validate()
            .is_ok()
        );
    }

    fn attestation() -> TargetAttestation {
        TargetAttestation {
            session_binding: 9,
            target_binding: TARGET,
            focus_binding: FOCUS,
            ttl_seconds: 30,
        }
    }

    #[test]
    fn attestation_requires_nonzero_bindings_and_bounded_ttl() {
        assert_eq!(attestation().validate(), Ok(()));
        for invalid in [
            (
                TargetAttestation {
                    session_binding: 0,
                    ..attestation()
                },
                ControlV2ValidationError::InvalidSessionBinding,
            ),
            (
                TargetAttestation {
                    target_binding: [0; 32],
                    ..attestation()
                },
                ControlV2ValidationError::InvalidTargetBinding,
            ),
            (
                TargetAttestation {
                    focus_binding: [0; 16],
                    ..attestation()
                },
                ControlV2ValidationError::InvalidFocusBinding,
            ),
            (
                TargetAttestation {
                    ttl_seconds: 4,
                    ..attestation()
                },
                ControlV2ValidationError::InvalidAttestationTtl,
            ),
            (
                TargetAttestation {
                    ttl_seconds: 61,
                    ..attestation()
                },
                ControlV2ValidationError::InvalidAttestationTtl,
            ),
        ] {
            assert_eq!(invalid.0.validate(), Err(invalid.1));
        }
    }

    #[test]
    fn lease_requests_require_exact_nonzero_identity_and_bounded_ttl() {
        assert_eq!(
            HidLeaseRequest::Acquire {
                session_binding: 9,
                target_binding: TARGET,
                ttl_seconds: 30,
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            HidLeaseRequest::Acquire {
                session_binding: 9,
                target_binding: TARGET,
                ttl_seconds: 0,
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidLeaseTtl)
        );
        assert_eq!(
            HidLeaseRequest::Acquire {
                session_binding: 9,
                target_binding: TARGET,
                ttl_seconds: 31,
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidLeaseTtl)
        );
        assert_eq!(
            HidLeaseRequest::Renew {
                lease_id: [0; 16],
                session_binding: 9,
                target_binding: TARGET,
                ttl_seconds: 30,
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidLeaseId)
        );
    }

    #[test]
    fn lease_report_uses_the_fixed_hid_validator() {
        assert_eq!(
            HidLeaseRequest::Report {
                lease_id: LEASE,
                session_binding: 9,
                target_binding: TARGET,
                report: HidReport::Keyboard {
                    data: [0, 0, 4, 0, 0, 0, 0, 0],
                },
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            HidLeaseRequest::Report {
                lease_id: LEASE,
                session_binding: 9,
                target_binding: TARGET,
                report: HidReport::Keyboard {
                    data: [0, 1, 4, 0, 0, 0, 0, 0],
                },
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidHidReport)
        );
    }

    #[test]
    fn atx_confirmation_requires_binding_and_timestamp() {
        assert_eq!(
            AtxConfirmation {
                session_binding: 9,
                confirmation_binding: CONFIRMATION,
                confirmed_at_unix_ms: 1_700_000_000_000,
            }
            .validate(),
            Ok(())
        );
        assert_eq!(
            AtxConfirmation {
                session_binding: 9,
                confirmation_binding: [0; 32],
                confirmed_at_unix_ms: 1,
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidConfirmationBinding)
        );
        assert_eq!(
            AtxConfirmation {
                session_binding: 9,
                confirmation_binding: CONFIRMATION,
                confirmed_at_unix_ms: 0,
            }
            .validate(),
            Err(ControlV2ValidationError::InvalidConfirmationTimestamp)
        );
    }
}
