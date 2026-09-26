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

/// Administration features that stay off until the operator switches them on
/// (`docs/design/admin-features-v1.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdminFeature {
    Terminal,
    Scripts,
    RemoteDownload,
    Picoclaw,
}

impl AdminFeature {
    pub const ALL: [Self; 4] = [
        Self::Terminal,
        Self::Scripts,
        Self::RemoteDownload,
        Self::Picoclaw,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Scripts => "scripts",
            Self::RemoteDownload => "remote_download",
            Self::Picoclaw => "picoclaw",
        }
    }
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
    /// Reviewed video-engine settings change (desired frame rate, JPEG quality).
    VideoSettings {
        change: crate::video_settings::VideoSettingsChange,
    },
    /// Read back the settings the owner will run the streamer with.
    VideoSettingsState {},
    /// Publish the device name over mDNS.
    Mdns {
        enabled: bool,
    },
    /// Read back whether mDNS publishing is on.
    MdnsState {},
    /// Keep the target awake with the reviewed relative nudge.
    MouseJiggler {
        change: crate::video_settings::MouseJigglerChange,
    },
    /// Read back the accepted jiggler state.
    MouseJigglerState {},
    /// Present HID only and refuse every virtual-media attach.
    HidOnly {
        enabled: bool,
    },
    /// Read back whether HID-only mode is on.
    HidOnlyState {},
    /// Switch one administration feature on or off.
    AdminFeature {
        feature: AdminFeature,
        enabled: bool,
    },
    /// Read back the administration feature switches.
    AdminFeatureState {},
    /// Enable and start, or stop and disable, the OpenSSH server.
    Ssh {
        enabled: bool,
    },
    /// Read back the OpenSSH server state.
    SshState {},
    /// The connected interfaces' IPv4 addressing and DNS.
    NetworkInfo {},
    /// Set one interface to DHCP, or to manual DNS with an optional static
    /// address; an addressing change reverts unless confirmed.
    Ipv4Set {
        change: Ipv4Change,
    },
    /// Keep the addressing change that is waiting for confirmation.
    Ipv4Confirm {},
    /// Read the WiFi device state.
    WifiState {},
    /// Connect to a WiFi network through NetworkManager.
    WifiConnect {
        ssid: String,
        password: String,
    },
    /// Disconnect the profile the owner brought up.
    WifiDisconnect {},
    /// Read the HTTPS redirect state and the public certificate.
    TlsState {},
    /// Switch the HTTP listener between serving and redirecting to HTTPS.
    TlsSet {
        enabled: bool,
    },
    /// Read the reviewed firewall baseline state.
    FirewallState {},
    /// Enable or disable the reviewed firewall baseline.
    FirewallBaseline {
        enabled: bool,
    },
    FirewallModeChange {
        request: crate::FirewallChangeRequest,
    },
    FirewallModeConfirm {
        request: crate::FirewallConfirmRequest,
    },
    FirewallModeStatus {},
    /// Write one reviewed test line through the system log.
    SystemLogTest {},
    /// Read the Tailscale surface state.
    TailscaleStatus {},
    /// Run one reviewed Tailscale action.
    TailscaleAction {
        action: String,
    },
    /// Read-only board status from the daemon that owns the board lines.
    BoardStatus {},
    /// Read pending HID release obligations without actuating or acquiring a lease.
    HidReconciliationState {},
    /// Reviewed post-boot acceptance: only the owner holds the journal, so the
    /// operator attests the guards and the owner closes the pending record.
    AcceptPostBoot {
        request: AcceptPostBootRequest,
    },
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ControlV2ValidationError {
    #[error("firewall operation identity or timestamp is invalid")]
    InvalidFirewallOperation,
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
    #[error("video-settings change is invalid")]
    InvalidVideoSettings,
    #[error("IPv4 change is invalid")]
    InvalidIpv4Change,
}

/// One IPv4 change: DHCP, or manual DNS servers with an optional static
/// address, prefix and router.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ipv4Change {
    pub interface: String,
    pub dhcp: bool,
    pub dns: Vec<String>,
    pub address: Option<String>,
    pub prefix: Option<u8>,
    pub gateway: Option<String>,
}

pub const MAX_IPV4_DNS_SERVERS: usize = 3;

impl Ipv4Change {
    pub fn validate(&self) -> Result<(), ControlV2ValidationError> {
        use std::net::{IpAddr, Ipv4Addr};
        let invalid = ControlV2ValidationError::InvalidIpv4Change;
        let interface_ok = (1..=15).contains(&self.interface.len())
            && self
                .interface
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !interface_ok || self.dns.len() > MAX_IPV4_DNS_SERVERS {
            return Err(invalid);
        }
        if self
            .dns
            .iter()
            .any(|server| server.parse::<IpAddr>().is_err())
        {
            return Err(invalid);
        }
        let address = self.address.as_deref().map(str::parse::<Ipv4Addr>);
        let gateway = self.gateway.as_deref().map(str::parse::<Ipv4Addr>);
        if self.dhcp {
            return if self.dns.is_empty()
                && self.address.is_none()
                && self.prefix.is_none()
                && self.gateway.is_none()
            {
                Ok(())
            } else {
                Err(invalid)
            };
        }
        if self.dns.is_empty() {
            return Err(invalid);
        }
        match (address, self.prefix, gateway) {
            (None, None, None) => Ok(()),
            (Some(Ok(address)), Some(prefix @ 1..=30), Some(Ok(gateway))) => {
                let mask = u32::MAX << (32 - u32::from(prefix));
                let (address, gateway) = (u32::from(address), u32::from(gateway));
                let host = address & !mask;
                if address & mask == gateway & mask
                    && address != gateway
                    && host != 0
                    && host != !mask
                {
                    Ok(())
                } else {
                    Err(invalid)
                }
            }
            _ => Err(invalid),
        }
    }
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
            Self::VideoSettings { change } => change
                .validate()
                .map_err(|_| ControlV2ValidationError::InvalidVideoSettings),
            Self::VideoSettingsState {} => Ok(()),
            Self::Mdns { .. } => Ok(()),
            Self::MdnsState {} => Ok(()),
            Self::MouseJiggler { .. } => Ok(()),
            Self::MouseJigglerState {} => Ok(()),
            Self::HidOnly { .. } => Ok(()),
            Self::HidOnlyState {} => Ok(()),
            Self::AdminFeature { .. } => Ok(()),
            Self::AdminFeatureState {} => Ok(()),
            Self::Ssh { .. } => Ok(()),
            Self::SshState {} => Ok(()),
            Self::NetworkInfo {} => Ok(()),
            Self::Ipv4Set { change } => change.validate(),
            Self::Ipv4Confirm {} => Ok(()),
            Self::WifiState {} => Ok(()),
            Self::WifiConnect { .. } => Ok(()),
            Self::WifiDisconnect {} => Ok(()),
            Self::TlsState {} => Ok(()),
            Self::TlsSet { .. } => Ok(()),
            Self::FirewallState {} => Ok(()),
            Self::FirewallBaseline { .. } => Ok(()),
            Self::FirewallModeChange { request } => request.validate(),
            Self::FirewallModeConfirm { request } => request.validate(),
            Self::FirewallModeStatus {} => Ok(()),
            Self::SystemLogTest {} => Ok(()),
            Self::TailscaleStatus {} => Ok(()),
            Self::TailscaleAction { .. } => Ok(()),
            Self::BoardStatus {} => Ok(()),
            Self::HidReconciliationState {} => Ok(()),
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
