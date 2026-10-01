//! Control protocol v2 routing for the denial-by-default broker.
//!
//! Shipped builds construct [`ControlV2State::disabled`], which rejects every
//! v2 request with `capability_disabled` and opens no backend. A separate
//! `hid-transition-profile` feature can attach a HID control policy built from
//! the dedicated credential and sandbox; even then, leases require a current
//! target attestation and the capability is enabled only for that profile.

use std::time::Instant;

#[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
use std::time::Duration;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{
    AtxConfirmation, AtxControlAction, HidLeaseRequest, PlatformControlRequestV2,
    VideoSettingsChange,
};

use crate::{DEFAULT_MAX_ATTESTATIONS, TargetAttestationRegistry};

#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
use bli_kvm_atx_policy::{AtxEligibility, AtxOperation, AtxRejection};
#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
use bli_kvm_atx_transition::{AtxTransitionEngine, AtxTransitionOutcome};
#[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
use bli_kvm_hid_adapter::HidLeaseIdentity;
#[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
use bli_kvm_hid_policy::{HidControlPolicy, HidDecision, HidEligibility};
#[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
use bli_kvm_media_policy::{MediaEligibility, MediaOperation, MediaRejection};
#[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
use bli_kvm_media_transition::{MediaTransitionEngine, MediaTransitionOutcome};
#[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
use rand_core::{OsRng, RngCore};

/// The ATX engine behind a lock of its own: a pulse holds the line for up to
/// five seconds and must not hold the state HID, media and settings share.
#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
pub type SharedAtx = std::sync::Arc<tokio::sync::Mutex<AtxTransitionEngine>>;

#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
fn share_atx(engine: AtxTransitionEngine) -> SharedAtx {
    std::sync::Arc::new(tokio::sync::Mutex::new(engine))
}

pub struct ControlV2State {
    enabled: bool,
    attestations: TargetAttestationRegistry,
    #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
    atx: Option<SharedAtx>,
    #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
    media: Option<MediaTransitionEngine>,
    #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
    hid: Option<HidControlPolicy>,
    #[cfg(feature = "system-action-profile")]
    system_action: Option<
        crate::system_action::SystemActionState<Box<dyn crate::system_action::ActionRunner>>,
    >,
    #[cfg(feature = "network-write-profile")]
    network_write: Option<
        crate::network_write::NetworkWriteState<Box<dyn crate::network_write::NetworkWriter>>,
    >,
    video_settings: Option<crate::video_settings::VideoSettingsState>,
    mdns: Option<crate::device_settings::MdnsState>,
    mouse_jiggler: Option<crate::mouse_jiggler::MouseJigglerState>,
    hid_only: Option<crate::device_settings::HidOnlyState>,
    admin_features: Option<crate::admin_features::AdminFeaturesState>,
    ssh: Option<crate::ssh_settings::SshSettings>,
    quic: Option<crate::quic_settings::QuicSettings>,
    ipv4: Option<crate::ipv4_settings::SharedIpv4>,
    hostname: Option<crate::hostname_settings::HostnameSettings>,
    wifi: Option<crate::network_settings::SharedWifi>,
    tls: Option<crate::tls_settings::TlsState>,
    firewall: Option<crate::firewall_settings::SharedFirewall>,
    /// The mode runtime of `firewall`, kept apart so finding it never waits
    /// on the firewall's lock.
    firewall_modes: Option<crate::firewall_runtime::SharedModeRuntime>,
    system_log: Option<crate::system_log_settings::SystemLogState>,
    tailscale: Option<crate::tailscale::TailscaleState>,
    board_status: Option<crate::board_status::BoardStatusClient>,
    #[cfg(feature = "network-write-profile")]
    wol: Option<crate::network_write::WolState<Box<dyn crate::network_write::WolSender>>>,
}

impl ControlV2State {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
            atx: None,
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
            hid: None,
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    /// Arm the privileged system-action surface with a runner.
    #[cfg(feature = "system-action-profile")]
    pub fn with_system_action_runner(
        runner: Box<dyn crate::system_action::ActionRunner>,
        capability_enabled: bool,
        backend_ready: bool,
        boot_binding: [u8; 16],
    ) -> Self {
        let mut state = Self::enabled_base();
        state.system_action = Some(crate::system_action::SystemActionState::new(
            runner,
            capability_enabled,
            backend_ready,
            boot_binding,
        ));
        state
    }

    /// Attach the reviewed video-settings surface to the board state.
    pub fn with_video_settings(&mut self, state: crate::video_settings::VideoSettingsState) {
        self.video_settings = Some(state);
    }

    /// Attach the reviewed device toggles to the board state.
    pub fn with_device_settings(&mut self, mdns: crate::device_settings::MdnsState) {
        self.mdns = Some(mdns);
    }

    /// Attach the reviewed Tailscale surface to the board state.
    pub fn with_tailscale(&mut self, tailscale: crate::tailscale::TailscaleState) {
        self.tailscale = Some(tailscale);
    }

    /// Attach the reviewed system-log actions to the board state.
    pub fn with_system_log(&mut self, system_log: crate::system_log_settings::SystemLogState) {
        self.system_log = Some(system_log);
    }

    /// Attach the reviewed firewall baseline to the board state.
    pub fn with_firewall(&mut self, firewall: crate::firewall_settings::FirewallState) {
        self.firewall_modes = firewall.mode_runtime();
        self.firewall = Some(std::sync::Arc::new(std::sync::Mutex::new(firewall)));
    }

    /// The firewall state for the server's blocking worker, while enabled.
    pub fn firewall_shared(&self) -> Option<crate::firewall_settings::SharedFirewall> {
        if !self.enabled {
            return None;
        }
        self.firewall.clone()
    }

    /// Attach the reviewed HTTPS surface to the board state.
    pub fn with_tls(&mut self, tls: crate::tls_settings::TlsState) {
        self.tls = Some(tls);
    }

    /// Attach the reviewed WiFi surface to the board state.
    pub fn with_wifi(&mut self, wifi: crate::network_settings::WifiState) {
        self.wifi = Some(std::sync::Arc::new(std::sync::Mutex::new(wifi)));
    }

    /// The ATX engine for the server, which runs a pulse without the state's
    /// lock, while enabled.
    #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
    pub fn atx_shared(&self) -> Option<SharedAtx> {
        if !self.enabled {
            return None;
        }
        self.atx.clone()
    }

    /// The WiFi state for the server's blocking worker, while enabled.
    pub fn wifi_shared(&self) -> Option<crate::network_settings::SharedWifi> {
        if !self.enabled {
            return None;
        }
        self.wifi.clone()
    }

    /// Attach the reviewed HID-only switch to the board state.
    pub fn with_hid_only(&mut self, hid_only: crate::device_settings::HidOnlyState) {
        self.hid_only = Some(hid_only);
    }

    /// Attach the administration feature switches to the board state.
    pub fn with_hostname(&mut self, hostname: crate::hostname_settings::HostnameSettings) {
        self.hostname = Some(hostname);
    }

    pub fn with_ipv4(&mut self, ipv4: crate::ipv4_settings::Ipv4Settings) {
        self.ipv4 = Some(std::sync::Arc::new(std::sync::Mutex::new(ipv4)));
    }

    /// The network settings for the server's blocking worker, while enabled.
    pub fn ipv4_shared(&self) -> Option<crate::ipv4_settings::SharedIpv4> {
        if !self.enabled {
            return None;
        }
        self.ipv4.clone()
    }

    pub fn with_ssh(&mut self, ssh: crate::ssh_settings::SshSettings) {
        self.ssh = Some(ssh);
    }

    pub fn with_quic(&mut self, quic: crate::quic_settings::QuicSettings) {
        self.quic = Some(quic);
    }

    pub fn with_admin_features(&mut self, features: crate::admin_features::AdminFeaturesState) {
        self.admin_features = Some(features);
    }

    /// Attach the reviewed mouse jiggler to the board state.
    pub fn with_mouse_jiggler(&mut self, jiggler: crate::mouse_jiggler::MouseJigglerState) {
        self.mouse_jiggler = Some(jiggler);
    }

    /// One jiggler interval: the serve loop calls this, and it is a no-op when
    /// no jiggler is attached or the jiggler is off.
    pub fn jiggle_tick(&mut self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        match self.mouse_jiggler.as_mut() {
            Some(jiggler) => jiggler.tick(),
            None => Ok(()),
        }
    }

    /// Close command admission before releasing held HID state. Call while
    /// holding the owner state lock, then drain in-flight external workers
    /// before stopping the process. Repeating this retries persisted releases;
    /// an error never reopens admission or resumes the jiggler.
    pub async fn begin_quiescence(&mut self) -> Result<(), ControlRejectionCode> {
        self.enabled = false;
        #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
        if let Some(policy) = self.hid.as_mut() {
            return match policy.disconnect().await {
                HidDecision::Applied => Ok(()),
                HidDecision::Reject(rejection) => Err(rejection.rejection_code()),
            };
        }
        Ok(())
    }

    /// Release held HID state when an idle lease reaches its deadline. The
    /// broker serve loop calls this independently of client traffic.
    #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
    pub async fn hid_expiry_tick(&mut self, now: Instant) -> Result<(), ControlRejectionCode> {
        let Some(policy) = self.hid.as_mut() else {
            return Ok(());
        };
        match policy.expire(now).await {
            HidDecision::Applied => Ok(()),
            HidDecision::Reject(rejection) => Err(rejection.rejection_code()),
        }
    }

    #[cfg(not(any(feature = "hid-transition-profile", feature = "gadget-hid-profile")))]
    pub async fn hid_expiry_tick(&mut self, _now: Instant) -> Result<(), ControlRejectionCode> {
        Ok(())
    }

    #[cfg(feature = "system-action-profile")]
    fn enabled_base() -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
            atx: None,
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
            hid: None,
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
    pub fn with_hid_policy(policy: HidControlPolicy) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
            atx: None,
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            hid: Some(policy),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
    pub fn with_atx_engine(engine: AtxTransitionEngine) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            atx: Some(share_atx(engine)),
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
            hid: None,
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    /// Attaches the persisted media engine to an already enabled state.
    #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
    pub fn with_media_engine(mut self, engine: MediaTransitionEngine) -> Self {
        self.enabled = true;
        self.media = Some(engine);
        self
    }

    /// Serves both transition backends from one state, for the restricted
    /// control test profile. Only a build with both profile features compiles
    /// this constructor; HID requests still require a current attestation and
    /// ATX requests still require a reviewed confirmation.
    #[cfg(all(
        any(feature = "hid-transition-profile", feature = "gadget-hid-profile"),
        any(feature = "atx-transition-profile", feature = "gpio-atx-profile")
    ))]
    pub fn with_hid_and_atx(hid: HidControlPolicy, atx: AtxTransitionEngine) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            atx: Some(share_atx(atx)),
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            hid: Some(hid),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    /// Serves the media and HID backends from one state, for the gadget owner.
    /// Media requests still require a reviewed eligibility and HID requests
    /// still require a current attestation.
    #[cfg(all(feature = "gadget-media-profile", feature = "gadget-hid-profile"))]
    pub fn with_media_and_hid(media: MediaTransitionEngine, hid: HidControlPolicy) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
            atx: None,
            media: Some(media),
            hid: Some(hid),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    /// Serves media, HID and ATX from one state: the single gadget and board
    /// owner. Each backend keeps its own eligibility gate.
    #[cfg(all(
        feature = "gadget-media-profile",
        feature = "gadget-hid-profile",
        any(feature = "gpio-atx-profile", feature = "chardev-atx-profile")
    ))]
    pub fn with_board_backends(
        media: MediaTransitionEngine,
        hid: HidControlPolicy,
        atx: AtxTransitionEngine,
    ) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            atx: Some(share_atx(atx)),
            media: Some(media),
            hid: Some(hid),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    /// Attach the armed system-action surface to an already built state, so
    /// the board owner keeps serving the gadget, media and ATX while the
    /// privileged reboot/shutdown path is available.
    #[cfg(feature = "system-action-profile")]
    pub fn attach_system_action(
        &mut self,
        state: crate::system_action::SystemActionState<Box<dyn crate::system_action::ActionRunner>>,
    ) {
        self.system_action = Some(state);
    }

    /// Attach the reviewed Wake-on-LAN sender, so the owner can serve the
    /// bounded magic-packet write behind the capability gate.
    #[cfg(feature = "network-write-profile")]
    pub fn attach_wol(
        &mut self,
        state: crate::network_write::WolState<Box<dyn crate::network_write::WolSender>>,
    ) {
        self.wol = Some(state);
    }

    /// Attach the reviewed network-write surface, so the owner serves DNS and
    /// IPv6 writes behind the capability gate.
    #[cfg(feature = "network-write-profile")]
    pub fn attach_network_write(
        &mut self,
        state: crate::network_write::NetworkWriteState<
            Box<dyn crate::network_write::NetworkWriter>,
        >,
    ) {
        self.network_write = Some(state);
    }

    /// Attach the read-only board status proxy to the board daemon.
    pub fn attach_board_status(&mut self, client: crate::board_status::BoardStatusClient) {
        self.board_status = Some(client);
    }

    #[cfg(feature = "system-action-profile")]
    fn handle_accept_post_boot(&mut self, guards_ready: bool) -> ControlResponse {
        let Some(state) = self.system_action.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        state.accept_post_boot(guards_ready)
    }

    #[cfg(not(feature = "system-action-profile"))]
    fn handle_accept_post_boot(&mut self, _guards_ready: bool) -> ControlResponse {
        reject(ControlRejectionCode::CapabilityDisabled)
    }

    fn handle_video_settings(&mut self, change: &VideoSettingsChange) -> ControlResponse {
        let Some(state) = self.video_settings.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        state.handle(change)
    }

    fn handle_tailscale_status(&mut self) -> ControlResponse {
        let Some(tailscale) = self.tailscale.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        let status = tailscale.status();
        ControlResponse::TailscaleState {
            installed: status.installed,
            running: status.running,
            enabled: status.enabled,
            login_name: status.login_name,
            device_ip: status.device_ip,
            device_name: status.device_name,
            message: status.message,
        }
    }

    fn handle_tailscale_action(&mut self, action: &str) -> ControlResponse {
        let Some(tailscale) = self.tailscale.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        match crate::tailscale::TailscaleAction::parse(action) {
            Some(action) => tailscale.act(action),
            None => reject(ControlRejectionCode::InvalidState),
        }
    }

    fn handle_system_log_test(&mut self) -> ControlResponse {
        let Some(system_log) = self.system_log.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        system_log.test_message()
    }

    fn handle_firewall(&mut self, request: &PlatformControlRequestV2) -> ControlResponse {
        let Some(firewall) = self.firewall.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        match firewall.lock() {
            Ok(mut firewall) => firewall.dispatch(request),
            Err(_) => reject(ControlRejectionCode::BackendRejected),
        }
    }

    fn handle_tls_state(&mut self) -> ControlResponse {
        let Some(tls) = self.tls.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        let enabled = match tls.enabled() {
            Ok(enabled) => enabled,
            Err(reason) => {
                eprintln!("tls: state read failed: {reason}");
                return reject(ControlRejectionCode::BackendRejected);
            }
        };
        let certificate = tls.certificate().unwrap_or_default();
        ControlResponse::TlsState {
            enabled,
            subject: certificate.subject,
            issuer: certificate.issuer,
            not_after: certificate.not_after,
            fingerprint_sha256: certificate.fingerprint_sha256,
        }
    }

    fn handle_tls_set(&mut self, enabled: bool) -> ControlResponse {
        let Some(tls) = self.tls.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        tls.set(enabled)
    }

    fn handle_wifi(&mut self, request: &PlatformControlRequestV2) -> ControlResponse {
        let Some(wifi) = self.wifi.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        match wifi.lock() {
            Ok(mut wifi) => wifi.dispatch(request),
            Err(_) => reject(ControlRejectionCode::BackendRejected),
        }
    }

    fn handle_hid_only(&mut self, enabled: bool) -> ControlResponse {
        let Some(state) = self.hid_only.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        state.handle(enabled)
    }

    fn handle_hid_only_state(&self) -> ControlResponse {
        let Some(state) = self.hid_only.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        ControlResponse::HidOnlyState {
            enabled: state.enabled(),
        }
    }

    fn handle_mouse_jiggler(
        &mut self,
        change: &bli_kvm_core::MouseJigglerChange,
    ) -> ControlResponse {
        let Some(jiggler) = self.mouse_jiggler.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        jiggler.handle(change)
    }

    fn handle_mouse_jiggler_state(&self) -> ControlResponse {
        let Some(jiggler) = self.mouse_jiggler.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        ControlResponse::MouseJigglerState {
            enabled: jiggler.enabled(),
            mode: jiggler.mode(),
        }
    }

    fn handle_mdns(&mut self, enabled: bool) -> ControlResponse {
        let Some(state) = self.mdns.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        state.handle(enabled)
    }

    fn handle_mdns_state(&self) -> ControlResponse {
        let Some(state) = self.mdns.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        ControlResponse::MdnsState {
            enabled: state.current(),
        }
    }

    fn handle_video_settings_state(&self) -> ControlResponse {
        let Some(state) = self.video_settings.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        let current = match state.readback() {
            Ok(current) => current,
            Err(code) => return reject(code),
        };
        ControlResponse::VideoState {
            fps: current.fps,
            quality: current.quality,
            resolution: Some(current.resolution),
        }
    }

    fn handle_board_status(&self) -> ControlResponse {
        // The operator configuration is the capability: an unattached client
        // is resolved from the environment on first use, so the proxy works on
        // every board-profile entry point.
        let resolved = self
            .board_status
            .clone()
            .or_else(crate::board_status::board_status_client_from_environment);
        let Some(client) = resolved.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        match client.read() {
            Ok(status) => ControlResponse::BoardStatus {
                power_led_present: status.power_led_present,
                power_led_on: status.power_led_on,
                hdd_led_present: status.hdd_led_present,
                hdd_led_on: status.hdd_led_on,
            },
            Err(_) => reject(ControlRejectionCode::BackendUnavailable),
        }
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
            atx: None,
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
            hid: None,
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
            mdns: None,
            mouse_jiggler: None,
            hid_only: None,
            admin_features: None,
            ssh: None,
            quic: None,
            ipv4: None,
            hostname: None,
            wifi: None,
            tls: None,
            firewall: None,
            firewall_modes: None,
            system_log: None,
            tailscale: None,
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    fn hid_reconciliation_state(&self) -> ControlResponse {
        #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
        if let Some(hid) = &self.hid {
            return match u64::try_from(hid.pending_release_count()) {
                Ok(pending_releases) => {
                    ControlResponse::HidReconciliationState { pending_releases }
                }
                Err(_) => reject(ControlRejectionCode::Internal),
            };
        }
        reject(ControlRejectionCode::CapabilityDisabled)
    }

    pub fn firewall_mode_runtime(&self) -> Option<crate::firewall_runtime::SharedModeRuntime> {
        if !self.enabled {
            return None;
        }
        self.firewall_modes.clone()
    }

    pub async fn handle(
        &mut self,
        request: &PlatformControlRequestV2,
        now: Instant,
    ) -> ControlResponse {
        self.handle_request("", request, now).await
    }

    pub async fn handle_request(
        &mut self,
        operation_id: &str,
        request: &PlatformControlRequestV2,
        now: Instant,
    ) -> ControlResponse {
        if !self.enabled {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        match request {
            PlatformControlRequestV2::HidReconciliationState {} => self.hid_reconciliation_state(),
            PlatformControlRequestV2::TargetAttest { attestation } => {
                match self.attestations.attest(attestation, now) {
                    Ok(()) => ControlResponse::Applied,
                    Err(_) => reject(ControlRejectionCode::InvalidState),
                }
            }
            PlatformControlRequestV2::TargetRelease { release } => {
                if release.validate().is_err() {
                    return reject(ControlRejectionCode::InvalidState);
                }
                let _ = self.attestations.release(
                    release.session_binding,
                    &release.target_binding,
                    now,
                );
                ControlResponse::Applied
            }
            PlatformControlRequestV2::HidLease { lease } => self.handle_hid_lease(lease, now).await,
            PlatformControlRequestV2::Atx {
                action,
                confirmation,
            } => {
                self.handle_atx(operation_id, *action, confirmation, now)
                    .await
            }
            PlatformControlRequestV2::VirtualMedia { request } => {
                self.handle_media(operation_id, request, now).await
            }
            PlatformControlRequestV2::VirtualMediaObserve { request } => {
                self.handle_media_observe(request).await
            }
            PlatformControlRequestV2::SystemAction { request } => {
                self.handle_system_action(operation_id, request, now)
            }
            PlatformControlRequestV2::NetworkWrite { request } => {
                self.handle_network_write(request)
            }
            PlatformControlRequestV2::VideoSettings { change } => {
                self.handle_video_settings(change)
            }
            PlatformControlRequestV2::VideoSettingsState {} => self.handle_video_settings_state(),
            PlatformControlRequestV2::Mdns { enabled } => self.handle_mdns(*enabled),
            PlatformControlRequestV2::MdnsState {} => self.handle_mdns_state(),
            PlatformControlRequestV2::MouseJiggler { change } => self.handle_mouse_jiggler(change),
            PlatformControlRequestV2::MouseJigglerState {} => self.handle_mouse_jiggler_state(),
            PlatformControlRequestV2::HidOnly { enabled } => self.handle_hid_only(*enabled),
            PlatformControlRequestV2::HidOnlyState {} => self.handle_hid_only_state(),
            PlatformControlRequestV2::AdminFeature { feature, enabled } => {
                match self.admin_features.as_mut() {
                    Some(features) => features.handle(*feature, *enabled),
                    None => reject(ControlRejectionCode::CapabilityDisabled),
                }
            }
            PlatformControlRequestV2::Ssh { enabled } => match self.ssh.as_mut() {
                Some(ssh) => ssh.handle(*enabled),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::Hostname { name } => match self.hostname.as_mut() {
                Some(hostname) => hostname.set(name),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::NetworkInfo {}
            | PlatformControlRequestV2::Ipv4Set { .. }
            | PlatformControlRequestV2::Ipv6Set { .. }
            | PlatformControlRequestV2::Ipv4Confirm {} => match self.ipv4.as_ref() {
                Some(ipv4) => match ipv4.lock() {
                    Ok(mut ipv4) => ipv4.dispatch(request),
                    Err(_) => reject(ControlRejectionCode::BackendRejected),
                },
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::SshState {} => match self.ssh.as_mut() {
                Some(ssh) => ssh.state(),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::Quic { enabled } => match self.quic.as_mut() {
                Some(quic) => quic.handle(*enabled),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::QuicState {} => match self.quic.as_mut() {
                Some(quic) => quic.state(),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::AdminFeatureState {} => match self.admin_features.as_ref() {
                Some(features) => features.state(),
                None => reject(ControlRejectionCode::CapabilityDisabled),
            },
            PlatformControlRequestV2::WifiState {}
            | PlatformControlRequestV2::WifiScan { .. }
            | PlatformControlRequestV2::WifiSaved {}
            | PlatformControlRequestV2::WifiConnect { .. }
            | PlatformControlRequestV2::WifiDisconnect {}
            | PlatformControlRequestV2::WifiProfile { .. }
            | PlatformControlRequestV2::WifiRadio { .. }
            | PlatformControlRequestV2::WifiPreferred { .. }
            | PlatformControlRequestV2::WifiPreferredConfirm {} => self.handle_wifi(request),
            PlatformControlRequestV2::TlsState {} => self.handle_tls_state(),
            PlatformControlRequestV2::TlsSet { enabled } => self.handle_tls_set(*enabled),
            PlatformControlRequestV2::FirewallState {}
            | PlatformControlRequestV2::FirewallRules { .. }
            | PlatformControlRequestV2::FirewallBaseline { .. } => self.handle_firewall(request),
            PlatformControlRequestV2::FirewallModeChange { .. }
            | PlatformControlRequestV2::FirewallModeStatus {}
            | PlatformControlRequestV2::FirewallModeConfirm { .. } => ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled,
            },
            PlatformControlRequestV2::SystemLogTest {} => self.handle_system_log_test(),
            PlatformControlRequestV2::TailscaleStatus {} => self.handle_tailscale_status(),
            PlatformControlRequestV2::TailscaleAction { action } => {
                self.handle_tailscale_action(action)
            }
            PlatformControlRequestV2::BoardStatus {} => self.handle_board_status(),
            PlatformControlRequestV2::AcceptPostBoot { request } => {
                self.handle_accept_post_boot(request.guards_ready)
            }
        }
    }

    #[cfg(feature = "system-action-profile")]
    fn handle_system_action(
        &mut self,
        operation_id: &str,
        request: &bli_kvm_core::SystemActionWireRequest,
        now: Instant,
    ) -> ControlResponse {
        let Some(state) = self.system_action.as_mut() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        let _ = now;
        let now_ms = unix_millis();
        state.handle(operation_id, request, now_ms)
    }

    #[cfg(feature = "network-write-profile")]
    fn handle_network_write(&self, request: &bli_kvm_core::NetworkWriteRequest) -> ControlResponse {
        let Some(state) = self.network_write.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        if request.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        use crate::network_write::NetworkWriteOutcome;
        use bli_kvm_core::NetworkWriteTarget;
        let outcome = match &request.target {
            NetworkWriteTarget::Dns { servers } => state.set_dns(servers),
            NetworkWriteTarget::Ipv6 {
                interface,
                disabled,
            } => state.set_ipv6(interface, *disabled),
            NetworkWriteTarget::Wol { mac } => match self.wol.as_ref() {
                Some(wol) => wol.send(mac),
                None => NetworkWriteOutcome::Unavailable,
            },
        };
        match outcome {
            NetworkWriteOutcome::Applied { .. } => ControlResponse::Applied,
            NetworkWriteOutcome::Refused => reject(ControlRejectionCode::BackendRejected),
            NetworkWriteOutcome::Unavailable => reject(ControlRejectionCode::CapabilityDisabled),
        }
    }

    #[cfg(not(feature = "network-write-profile"))]
    fn handle_network_write(
        &self,
        _request: &bli_kvm_core::NetworkWriteRequest,
    ) -> ControlResponse {
        reject(ControlRejectionCode::CapabilityDisabled)
    }

    #[cfg(not(feature = "system-action-profile"))]
    fn handle_system_action(
        &mut self,
        _operation_id: &str,
        _request: &bli_kvm_core::SystemActionWireRequest,
        _now: Instant,
    ) -> ControlResponse {
        reject(ControlRejectionCode::CapabilityDisabled)
    }

    #[cfg(not(any(feature = "hid-transition-profile", feature = "gadget-hid-profile")))]
    async fn handle_hid_lease(
        &mut self,
        _lease: &HidLeaseRequest,
        _now: Instant,
    ) -> ControlResponse {
        reject(ControlRejectionCode::BackendUnavailable)
    }

    #[cfg(any(feature = "hid-transition-profile", feature = "gadget-hid-profile"))]
    async fn handle_hid_lease(&mut self, lease: &HidLeaseRequest, now: Instant) -> ControlResponse {
        let Some(policy) = self.hid.as_mut() else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
        if let HidLeaseRequest::Release {
            lease_id,
            session_binding,
            target_binding,
        } = lease
        {
            let Ok(identity) = HidLeaseIdentity::new(*lease_id, *session_binding, *target_binding)
            else {
                return reject(ControlRejectionCode::InvalidState);
            };
            return match policy.release(identity).await {
                HidDecision::Applied => ControlResponse::Applied,
                HidDecision::Reject(rejection) => reject(rejection.rejection_code()),
            };
        }

        // Lazy gadget recovery: a gadget lost to an external unbind or a USB
        // controller reset is rebuilt before the lease is served, so the first
        // HID request after a reboot still finds a keyboard and a mouse.
        #[cfg(feature = "gadget-media-profile")]
        if let Some(media) = self.media.as_ref()
            && let Err(error) = media.ensure_hid_gadget()
        {
            eprintln!("gadget: lazy HID bind failed: {error:?}");
        }

        let (session_binding, target_binding, ttl_seconds) = match lease {
            HidLeaseRequest::Acquire {
                session_binding,
                target_binding,
                ttl_seconds,
            }
            | HidLeaseRequest::Renew {
                session_binding,
                target_binding,
                ttl_seconds,
                ..
            } => (*session_binding, target_binding, *ttl_seconds),
            HidLeaseRequest::Report {
                session_binding,
                target_binding,
                ..
            } => (*session_binding, target_binding, 1),
            HidLeaseRequest::Release { .. } => unreachable!("release handled above"),
        };
        if !self
            .attestations
            .authorize(session_binding, target_binding, now)
        {
            return reject(ControlRejectionCode::NotAttested);
        }
        // A live lease proves the session is active, so its target attestation
        // is extended while input flows: the reference keeps the keyboard and
        // mouse alive for as long as the page is open, and a renewal that
        // arrives inside the lease window must not depend on the client having
        // refreshed its focus confirmation first.
        if matches!(
            lease,
            HidLeaseRequest::Renew { .. } | HidLeaseRequest::Report { .. }
        ) && let Some(focus_binding) = self.attestations.focus_binding(session_binding, now)
        {
            let attestation = bli_kvm_core::TargetAttestation {
                session_binding,
                target_binding: *target_binding,
                focus_binding,
                ttl_seconds: bli_kvm_core::TARGET_ATTESTATION_MAX_TTL_SECONDS,
            };
            let _ = self.attestations.attest(&attestation, now);
        }
        let eligibility = HidEligibility::verified_for_test_profile();
        let ttl = Duration::from_secs(u64::from(ttl_seconds));

        let (identity, report) = match lease {
            HidLeaseRequest::Acquire { .. } => {
                let mut lease_id = [0_u8; 16];
                OsRng.fill_bytes(&mut lease_id);
                if lease_id.iter().all(|byte| *byte == 0) {
                    return reject(ControlRejectionCode::Internal);
                }
                let Ok(identity) =
                    HidLeaseIdentity::new(lease_id, session_binding, *target_binding)
                else {
                    return reject(ControlRejectionCode::InvalidState);
                };
                let decision = policy.acquire(identity, eligibility, now, ttl).await;
                return match decision {
                    HidDecision::Applied => ControlResponse::LeaseGranted {
                        lease_id: identity.lease_id(),
                    },
                    HidDecision::Reject(rejection) => reject(rejection.rejection_code()),
                };
            }
            HidLeaseRequest::Renew {
                lease_id,
                session_binding,
                target_binding,
                ..
            } => {
                let Ok(identity) =
                    HidLeaseIdentity::new(*lease_id, *session_binding, *target_binding)
                else {
                    return reject(ControlRejectionCode::InvalidState);
                };
                return match policy.renew(identity, eligibility, now, ttl).await {
                    HidDecision::Applied => ControlResponse::Applied,
                    HidDecision::Reject(rejection) => reject(rejection.rejection_code()),
                };
            }
            HidLeaseRequest::Report {
                lease_id,
                session_binding,
                target_binding,
                report,
            } => {
                // A report proves the session is active, so it refreshes the
                // lease to the reviewed maximum: input must not depend on the
                // client's renewal cadence.
                let Ok(refresh) =
                    HidLeaseIdentity::new(*lease_id, *session_binding, *target_binding)
                else {
                    return reject(ControlRejectionCode::InvalidState);
                };
                if let HidDecision::Reject(rejection) = policy
                    .renew(
                        refresh,
                        eligibility,
                        now,
                        Duration::from_secs(u64::from(bli_kvm_core::HID_LEASE_MAX_TTL_SECONDS)),
                    )
                    .await
                {
                    return reject(rejection.rejection_code());
                }
                let Ok(identity) =
                    HidLeaseIdentity::new(*lease_id, *session_binding, *target_binding)
                else {
                    return reject(ControlRejectionCode::InvalidState);
                };
                (identity, report)
            }
            HidLeaseRequest::Release { .. } => unreachable!("release handled above"),
        };
        match policy.report(identity, eligibility, now, report).await {
            HidDecision::Applied => ControlResponse::Applied,
            HidDecision::Reject(rejection) => reject(rejection.rejection_code()),
        }
    }
}

fn reject(code: ControlRejectionCode) -> ControlResponse {
    ControlResponse::Rejected { code }
}

#[cfg(not(any(feature = "atx-transition-profile", feature = "gpio-atx-profile")))]
impl ControlV2State {
    async fn handle_atx(
        &mut self,
        _operation_id: &str,
        _action: AtxControlAction,
        _confirmation: &AtxConfirmation,
        _now: Instant,
    ) -> ControlResponse {
        reject(ControlRejectionCode::BackendUnavailable)
    }
}

#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
impl ControlV2State {
    async fn handle_atx(
        &mut self,
        operation_id: &str,
        action: AtxControlAction,
        confirmation: &AtxConfirmation,
        _now: Instant,
    ) -> ControlResponse {
        let Some(engine) = self.atx.clone() else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
        run_atx(engine, operation_id, action, confirmation).await
    }
}

/// Validates one ATX request and runs its pulse on the blocking pool while
/// holding only the engine's own lock. A request that finds a pulse running
/// is refused at once instead of waiting behind it.
#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
pub async fn run_atx(
    engine: SharedAtx,
    operation_id: &str,
    action: AtxControlAction,
    confirmation: &AtxConfirmation,
) -> ControlResponse {
    let Some(operation_id) = hex_operation_id(operation_id) else {
        return reject(ControlRejectionCode::InvalidState);
    };
    let now_ms = unix_millis();
    let Some(eligibility) = AtxEligibility::reviewed_for_confirmation(
        confirmation.session_binding,
        confirmation.confirmation_binding,
        confirmation.confirmed_at_unix_ms,
        now_ms,
        true,
        true,
    ) else {
        return reject(ControlRejectionCode::ConfirmationStale);
    };
    let Ok(operation) = AtxOperation::new(
        operation_id,
        confirmation.session_binding,
        confirmation.confirmation_binding,
        confirmation.confirmed_at_unix_ms,
        action,
    ) else {
        return reject(ControlRejectionCode::InvalidState);
    };
    let Ok(mut guard) = engine.try_lock_owned() else {
        return reject(ControlRejectionCode::CooldownActive);
    };
    let runtime = tokio::runtime::Handle::current();
    let outcome = tokio::task::spawn_blocking(move || {
        runtime.block_on(guard.execute(operation, eligibility, now_ms))
    })
    .await;
    match outcome {
        Ok(Ok(AtxTransitionOutcome::Applied | AtxTransitionOutcome::ReplayedApplied)) => {
            ControlResponse::Applied
        }
        Ok(Ok(AtxTransitionOutcome::ReplayedRejected | AtxTransitionOutcome::BackendFailed)) => {
            reject(ControlRejectionCode::BackendRejected)
        }
        Ok(Ok(AtxTransitionOutcome::Rejected(rejection))) => reject(atx_code(rejection)),
        Ok(Err(_)) | Err(_) => reject(ControlRejectionCode::Internal),
    }
}

#[cfg(not(any(feature = "media-transition-profile", feature = "gadget-media-profile")))]
impl ControlV2State {
    async fn handle_media_observe(
        &mut self,
        _request: &bli_kvm_core::MediaObserveRequest,
    ) -> ControlResponse {
        reject(ControlRejectionCode::BackendUnavailable)
    }

    async fn handle_media(
        &mut self,
        _operation_id: &str,
        _request: &bli_kvm_core::MediaRequest,
        _now: Instant,
    ) -> ControlResponse {
        reject(ControlRejectionCode::BackendUnavailable)
    }
}

#[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
impl ControlV2State {
    async fn handle_media_observe(
        &mut self,
        request: &bli_kvm_core::MediaObserveRequest,
    ) -> ControlResponse {
        if request.validate().is_err() {
            return reject(ControlRejectionCode::InvalidState);
        }
        let Some(engine) = self.media.as_mut() else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
        match engine.state().await {
            Some(state) => ControlResponse::MediaState {
                file: state.file,
                connected: state.connected,
                cdrom: state.cdrom,
                writable: state.writable,
            },
            None => reject(ControlRejectionCode::BackendUnavailable),
        }
    }

    async fn handle_media(
        &mut self,
        operation_id: &str,
        request: &bli_kvm_core::MediaRequest,
        _now: Instant,
    ) -> ControlResponse {
        // HID-only mode keeps the keyboard and mouse and refuses every media
        // attach; a detach stays available so an attached image can be removed.
        let detaching = matches!(
            request.command,
            bli_kvm_core::VirtualMediaCommand::SetConnected { connected: false }
        );
        if self.hid_only.as_ref().is_some_and(|state| state.enabled()) && !detaching {
            return reject(ControlRejectionCode::CapabilityDisabled);
        }
        let Some(engine) = self.media.as_mut() else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
        let Some(operation_id) = hex_operation_id(operation_id) else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let Ok(operation) = MediaOperation::new(
            operation_id,
            request.session_binding,
            request.target_binding,
            request.issued_at_unix_ms,
            request.command,
        ) else {
            return reject(ControlRejectionCode::InvalidState);
        };
        let now_ms = unix_millis();
        let Ok(observed) = engine.observe().await else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
        match engine
            .execute(
                operation,
                MediaEligibility::verified_for_test_profile(),
                observed,
                now_ms,
            )
            .await
        {
            Ok(
                MediaTransitionOutcome::Applied
                | MediaTransitionOutcome::Noop
                | MediaTransitionOutcome::ReplayedApplied,
            ) => ControlResponse::Applied,
            Ok(MediaTransitionOutcome::BackendFailed(failure)) => reject(match failure {
                bli_kvm_media_transition::MediaBackendFailure::ImageMissing => {
                    ControlRejectionCode::ImageMissing
                }
                bli_kvm_media_transition::MediaBackendFailure::ImageTooSmall => {
                    ControlRejectionCode::ImageTooSmall
                }
                bli_kvm_media_transition::MediaBackendFailure::Other => {
                    ControlRejectionCode::BackendRejected
                }
            }),
            Ok(MediaTransitionOutcome::ReplayedRejected) => {
                reject(ControlRejectionCode::BackendRejected)
            }
            Ok(MediaTransitionOutcome::Rejected(rejection)) => reject(media_code(rejection)),
            Err(_) => reject(ControlRejectionCode::Internal),
        }
    }
}

#[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
fn media_code(rejection: MediaRejection) -> ControlRejectionCode {
    match rejection {
        MediaRejection::CapabilityDisabled => ControlRejectionCode::CapabilityDisabled,
        MediaRejection::BackendUnavailable => ControlRejectionCode::BackendUnavailable,
        MediaRejection::ConfirmationInvalid => ControlRejectionCode::NotAttested,
        MediaRejection::ConfirmationExpired => ControlRejectionCode::ConfirmationStale,
        MediaRejection::OperationIdConflict => ControlRejectionCode::Duplicate,
        MediaRejection::IndeterminatePending => ControlRejectionCode::PendingUnreconciled,
        MediaRejection::Busy | MediaRejection::MustDisconnect => ControlRejectionCode::InvalidState,
        MediaRejection::CooldownActive => ControlRejectionCode::CooldownActive,
    }
}

#[cfg(any(
    any(feature = "atx-transition-profile", feature = "gpio-atx-profile"),
    any(feature = "media-transition-profile", feature = "gadget-media-profile")
))]
fn hex_operation_id(value: &str) -> Option<[u8; 16]> {
    if value.len() != 32 {
        return None;
    }
    let mut output = [0_u8; 16];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let text = std::str::from_utf8(chunk).ok()?;
        output[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(output)
}

#[cfg(any(
    any(feature = "atx-transition-profile", feature = "gpio-atx-profile"),
    any(feature = "media-transition-profile", feature = "gadget-media-profile"),
    feature = "system-action-profile"
))]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
fn atx_code(rejection: AtxRejection) -> ControlRejectionCode {
    match rejection {
        AtxRejection::CapabilityDisabled => ControlRejectionCode::CapabilityDisabled,
        AtxRejection::BackendUnavailable => ControlRejectionCode::BackendUnavailable,
        AtxRejection::PhysicalActionsDisabled => ControlRejectionCode::InvalidState,
        AtxRejection::ConfirmationInvalid => ControlRejectionCode::NotAttested,
        AtxRejection::ConfirmationExpired => ControlRejectionCode::ConfirmationStale,
        AtxRejection::OperationIdConflict => ControlRejectionCode::Duplicate,
        AtxRejection::IndeterminatePending => ControlRejectionCode::PendingUnreconciled,
        AtxRejection::CooldownActive => ControlRejectionCode::CooldownActive,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "gadget-hid-profile")]
    #[tokio::test]
    async fn hid_reconciliation_observation_preserves_pending_ledger_without_device_access() {
        use bli_kvm_hid_adapter::{KvmdHidEvent, KvmdKeyCode};
        use bli_kvm_hid_transition::{
            GadgetHidConfig, GadgetHidExecutor, HidExecutor, HidReleaseLedger, HidTransitionEngine,
        };
        for pending in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("ledger.json");
            let ledger = HidReleaseLedger::new(&path).unwrap();
            let events = if pending {
                vec![KvmdHidEvent::Key {
                    key: KvmdKeyCode::from_usb_usage(4).unwrap(),
                    pressed: false,
                }]
            } else {
                vec![]
            };
            ledger.store(&events).unwrap();
            let before = std::fs::read(&path).unwrap();
            let engine = HidTransitionEngine::new_with_executor(
                HidExecutor::Gadget(GadgetHidExecutor::new(GadgetHidConfig {
                    hidg_dir: directory.path().join("absent-devices"),
                    keyboard_function: "hid.usb0".into(),
                    absolute_mouse_function: "hid.usb1".into(),
                    relative_mouse_function: "hid.usb2".into(),
                })),
                ledger,
            )
            .unwrap();
            let mut state = ControlV2State::with_hid_policy(HidControlPolicy::new(engine));
            assert_eq!(
                state
                    .handle(
                        &PlatformControlRequestV2::HidReconciliationState {},
                        Instant::now()
                    )
                    .await,
                ControlResponse::HidReconciliationState {
                    pending_releases: u64::from(pending)
                }
            );
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        let mut disabled = ControlV2State::disabled();
        assert_eq!(
            disabled
                .handle(
                    &PlatformControlRequestV2::HidReconciliationState {},
                    Instant::now()
                )
                .await,
            reject(ControlRejectionCode::CapabilityDisabled)
        );
    }
    use bli_kvm_core::{
        AtxConfirmation, AtxControlAction, HidLeaseRequest, HidReport, TargetAttestation,
    };

    use super::*;

    const TARGET: [u8; 32] = [0x11; 32];
    const FOCUS: [u8; 16] = [0x22; 16];
    const LEASE: [u8; 16] = [0x33; 16];

    #[tokio::test]
    async fn firewall_modes_require_an_attached_runtime() {
        let mut state = ControlV2State::enabled_for_test();
        let change = PlatformControlRequestV2::FirewallModeChange {
            request: bli_kvm_core::FirewallChangeRequest {
                operation_id: [1; 16],
                session_binding: [2; 32],
                mode: bli_kvm_core::FirewallMode::Restricted,
                issued_at_unix_ms: 100,
            },
        };
        let confirm = PlatformControlRequestV2::FirewallModeConfirm {
            request: bli_kvm_core::FirewallConfirmRequest {
                operation_id: [1; 16],
                session_binding: [2; 32],
                issued_at_unix_ms: 101,
            },
        };
        for request in [change, confirm] {
            assert_eq!(
                state.handle(&request, Instant::now()).await,
                reject(ControlRejectionCode::CapabilityDisabled)
            );
        }
    }

    fn attestation(ttl_seconds: u16) -> TargetAttestation {
        TargetAttestation {
            session_binding: 9,
            target_binding: TARGET,
            focus_binding: FOCUS,
            ttl_seconds,
        }
    }

    async fn state() -> ControlV2State {
        let mut state = ControlV2State::enabled_for_test();
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::TargetAttest {
                        attestation: attestation(30)
                    },
                    Instant::now()
                )
                .await,
            ControlResponse::Applied
        );
        state
    }

    #[tokio::test]
    async fn disabled_state_rejects_every_v2_request() {
        let now = Instant::now();
        let mut state = ControlV2State::disabled();
        assert!(!state.is_enabled());
        let requests = [
            PlatformControlRequestV2::TargetAttest {
                attestation: attestation(30),
            },
            PlatformControlRequestV2::TargetRelease {
                release: bli_kvm_core::TargetRelease {
                    session_binding: 9,
                    target_binding: TARGET,
                },
            },
            PlatformControlRequestV2::HidLease {
                lease: HidLeaseRequest::Acquire {
                    session_binding: 9,
                    target_binding: TARGET,
                    ttl_seconds: 30,
                },
            },
            PlatformControlRequestV2::Atx {
                action: AtxControlAction::PowerShort,
                confirmation: AtxConfirmation {
                    session_binding: 9,
                    confirmation_binding: [0x44; 32],
                    confirmed_at_unix_ms: 1,
                },
            },
        ];
        for request in &requests {
            assert_eq!(
                state.handle(request, now).await,
                ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled
                }
            );
        }
    }

    #[tokio::test]
    async fn virtual_media_is_unavailable_without_a_media_profile() {
        use bli_kvm_core::{MediaRequest, VirtualMediaCommand};

        let state_result = state().await;
        let mut state = state_result;
        let response = state
            .handle(
                &PlatformControlRequestV2::VirtualMedia {
                    request: MediaRequest {
                        command: VirtualMediaCommand::SetConnected { connected: false },
                        session_binding: 9,
                        target_binding: TARGET,
                        issued_at_unix_ms: 1_700,
                    },
                },
                Instant::now(),
            )
            .await;
        assert_eq!(
            response,
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendUnavailable
            }
        );
    }

    #[tokio::test]
    async fn enabled_state_without_a_backend_denies_hid_and_stores_attestations() {
        let now = Instant::now();
        let mut state = state().await;
        assert!(state.attestations.authorize(9, &TARGET, now));
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id: LEASE,
                            session_binding: 9,
                            target_binding: TARGET,
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        }
                    },
                    now
                )
                .await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendUnavailable
            }
        );
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::TargetRelease {
                        release: bli_kvm_core::TargetRelease {
                            session_binding: 9,
                            target_binding: TARGET,
                        }
                    },
                    now
                )
                .await,
            ControlResponse::Applied
        );
        assert!(!state.attestations.authorize(9, &TARGET, now));
    }

    #[tokio::test]
    async fn invalid_attestation_is_rejected_in_enabled_state() {
        let now = Instant::now();
        let mut state = ControlV2State::enabled_for_test();
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::TargetAttest {
                        attestation: attestation(0)
                    },
                    now
                )
                .await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
    }

    #[cfg(feature = "network-write-profile")]
    #[tokio::test]
    async fn network_writes_are_reviewed_and_fail_closed() {
        use crate::network_write::{NetworkWriteState, RefusingWriter};

        let mut state = ControlV2State::enabled_for_test();
        state.attach_network_write(NetworkWriteState::new(Box::new(RefusingWriter), true));
        let request = |servers: Vec<String>| PlatformControlRequestV2::NetworkWrite {
            request: bli_kvm_core::NetworkWriteRequest {
                target: bli_kvm_core::NetworkWriteTarget::Dns { servers },
                session_binding: 9,
                confirmation_binding: [0x11; 32],
                issued_at_unix_ms: 1_000,
            },
        };
        // A bounded request reaches the writer, which refuses because no
        // reviewed writer is installed.
        assert_eq!(
            state
                .handle(&request(vec!["1.1.1.1".to_string()]), Instant::now())
                .await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendRejected
            }
        );
        // The Wake-on-LAN target needs the reviewed sender; without it the
        // capability is reported unavailable instead of sending anything.
        let wol = PlatformControlRequestV2::NetworkWrite {
            request: bli_kvm_core::NetworkWriteRequest {
                target: bli_kvm_core::NetworkWriteTarget::Wol {
                    mac: "00:11:22:aa:bb:cc".to_string(),
                },
                session_binding: 9,
                confirmation_binding: [0x11; 32],
                issued_at_unix_ms: 1_000,
            },
        };
        assert_eq!(
            state.handle(&wol, Instant::now()).await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::CapabilityDisabled
            }
        );

        // A request without the reviewed bindings never reaches the writer.
        let mut unbounded = request(vec!["not-an-address".to_string()]);
        if let PlatformControlRequestV2::NetworkWrite { request } = &mut unbounded {
            request.confirmation_binding = [0; 32];
        }
        assert_eq!(
            state.handle(&unbounded, Instant::now()).await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_lease_requires_attestation_and_returns_a_lease_id() {
        use bli_kvm_hid_transition::HidReleaseLedger;
        use bli_kvm_kvmd_client::KvmdCredentials;
        use bli_kvm_kvmd_hid_client::KvmdHidClient;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("absent-kvmd.sock");
        let policy = HidControlPolicy::new(
            bli_kvm_hid_transition::HidTransitionEngine::new(
                KvmdHidClient::new(&socket).unwrap(),
                KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
                HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap(),
            )
            .unwrap(),
        );
        let now = Instant::now();
        let mut state = ControlV2State::with_hid_policy(policy);

        // A lease without a current attestation fails closed.
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Acquire {
                            session_binding: 9,
                            target_binding: TARGET,
                            ttl_seconds: 30,
                        }
                    },
                    now
                )
                .await,
            ControlResponse::Rejected {
                code: ControlRejectionCode::NotAttested
            }
        );

        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::TargetAttest {
                        attestation: attestation(30)
                    },
                    now
                )
                .await,
            ControlResponse::Applied
        );
        let response = state
            .handle(
                &PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: TARGET,
                        ttl_seconds: 30,
                    },
                },
                now,
            )
            .await;
        let ControlResponse::LeaseGranted { lease_id } = response else {
            panic!("expected a lease grant, got {response:?}");
        };
        assert!(lease_id.iter().any(|byte| *byte != 0));
        assert!(!socket.exists());
    }

    #[cfg(feature = "hid-transition-profile")]
    async fn serve_hid_statuses(
        socket: &std::path::Path,
        count: usize,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        serve_hid_outcomes(socket, vec![true; count]).await
    }

    #[cfg(feature = "hid-transition-profile")]
    async fn serve_hid_outcomes(
        socket: &std::path::Path,
        outcomes: Vec<bool>,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::{
            io::{AsyncReadExt, AsyncWriteExt},
            net::UnixListener,
        };

        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            for success in outcomes {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0_u8; 1024];
                    let read = stream.read(&mut chunk).await.unwrap();
                    request.extend_from_slice(&chunk[..read]);
                    if request.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                let body = if success {
                    r#"{"ok":true}"#
                } else {
                    r#"{"ok":false}"#
                };
                let status = if success { "200 OK" } else { "503 Unavailable" };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                requests.push(String::from_utf8(request).unwrap());
            }
            requests
        })
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn hid_expiry_tick_releases_held_state_without_client_traffic() {
        check_hid_safety_release(false, false).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn quiescence_releases_held_state_and_rejects_late_input() {
        check_hid_safety_release(true, false).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    #[tokio::test]
    async fn quiescence_failure_keeps_admission_closed_and_retries_release() {
        check_hid_safety_release(true, true).await;
    }

    #[cfg(feature = "hid-transition-profile")]
    async fn check_hid_safety_release(quiesce: bool, fail_release: bool) {
        use std::time::Duration;

        use bli_kvm_hid_transition::HidReleaseLedger;
        use bli_kvm_kvmd_client::KvmdCredentials;
        use bli_kvm_kvmd_hid_client::KvmdHidClient;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("kvmd.sock");
        let server = if fail_release {
            serve_hid_outcomes(&socket, vec![true, false, false, true]).await
        } else {
            serve_hid_statuses(&socket, 2).await
        };
        let policy = HidControlPolicy::new(
            bli_kvm_hid_transition::HidTransitionEngine::new(
                KvmdHidClient::new(&socket).unwrap(),
                KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
                HidReleaseLedger::new(directory.path().join("ledger.json")).unwrap(),
            )
            .unwrap(),
        );
        let now = Instant::now();
        let mut state = ControlV2State::with_hid_policy(policy);
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::TargetAttest {
                        attestation: attestation(30),
                    },
                    now,
                )
                .await,
            ControlResponse::Applied
        );
        let ControlResponse::LeaseGranted { lease_id } = state
            .handle(
                &PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Acquire {
                        session_binding: 9,
                        target_binding: TARGET,
                        ttl_seconds: 30,
                    },
                },
                now,
            )
            .await
        else {
            panic!("expected a lease grant");
        };
        assert_eq!(
            state
                .handle(
                    &PlatformControlRequestV2::HidLease {
                        lease: HidLeaseRequest::Report {
                            lease_id,
                            session_binding: 9,
                            target_binding: TARGET,
                            report: HidReport::Keyboard {
                                data: [0, 0, 4, 0, 0, 0, 0, 0],
                            },
                        },
                    },
                    now,
                )
                .await,
            ControlResponse::Applied
        );

        if quiesce {
            if fail_release {
                assert!(state.begin_quiescence().await.is_err());
                assert!(!state.is_enabled());
                assert!(state.hid.as_ref().unwrap().pending_release_count() > 0);
            }
            state.begin_quiescence().await.unwrap();
            state.begin_quiescence().await.unwrap();
            assert!(!state.is_enabled());
            assert_eq!(
                state
                    .handle(
                        &PlatformControlRequestV2::HidLease {
                            lease: HidLeaseRequest::Report {
                                lease_id,
                                session_binding: 9,
                                target_binding: TARGET,
                                report: HidReport::Keyboard {
                                    data: [0, 0, 5, 0, 0, 0, 0, 0]
                                },
                            },
                        },
                        now
                    )
                    .await,
                reject(ControlRejectionCode::CapabilityDisabled)
            );
        } else {
            state
                .hid_expiry_tick(now + Duration::from_secs(31))
                .await
                .unwrap();
        }
        // Further ticks are no-ops and do not need a backend connection.
        state
            .hid_expiry_tick(now + Duration::from_secs(32))
            .await
            .unwrap();

        let requests = server.await.unwrap();
        assert!(requests[0].contains("key=KeyA&state=1"));
        assert!(requests[1].contains("key=KeyA&state=0"));
        if fail_release {
            assert!(requests[2].contains("key=KeyA&state=0"));
            assert!(requests[3].contains("key=KeyA&state=0"));
            assert_eq!(state.hid.as_ref().unwrap().pending_release_count(), 0);
        }
    }

    #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
    #[cfg(feature = "atx-transition-profile")]
    async fn serve_atx(socket: &std::path::Path) -> tokio::task::JoinHandle<Vec<String>> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::UnixListener;

        let listener = UnixListener::bind(socket).unwrap();
        tokio::spawn(async move {
            let mut requests = Vec::new();
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0_u8; 1024];
                let read = stream.read(&mut chunk).await.unwrap();
                request.extend_from_slice(&chunk[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}")
                .await
                .unwrap();
            requests.push(String::from_utf8(request).unwrap());
            requests
        })
    }

    #[cfg(feature = "atx-transition-profile")]
    #[tokio::test]
    async fn atx_executes_once_and_replays_duplicates() {
        use bli_kvm_atx_transition::AtxTransitionEngine;
        use bli_kvm_kvmd_atx_client::KvmdAtxClient;
        use bli_kvm_kvmd_client::KvmdCredentials;

        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("atx.sock");
        let server = serve_atx(&socket).await;
        let engine = AtxTransitionEngine::open(
            directory.path().join("journal.json"),
            KvmdAtxClient::new(&socket).unwrap(),
            KvmdCredentials::new("test-user", "not-a-real-password").unwrap(),
        )
        .unwrap();
        let confirmed_at = unix_millis();
        let confirmation = AtxConfirmation {
            session_binding: 9,
            confirmation_binding: [0x44; 32],
            confirmed_at_unix_ms: confirmed_at,
        };
        let mut state = ControlV2State::with_atx_engine(engine);
        let request = PlatformControlRequestV2::Atx {
            action: AtxControlAction::PowerShort,
            confirmation,
        };
        let operation_id = "0123456789abcdef0123456789abcdef";
        assert_eq!(
            state
                .handle_request(operation_id, &request, Instant::now())
                .await,
            ControlResponse::Applied
        );
        assert_eq!(
            state
                .handle_request(operation_id, &request, Instant::now())
                .await,
            ControlResponse::Applied
        );
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].starts_with("POST /atx/click?button=power&wait=1 HTTP/1.1\r\n"));
        assert!(requests[0].contains("x-kvmd-user: test-user"));
    }

    #[cfg(feature = "gpio-atx-profile")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_running_pulse_holds_neither_the_state_nor_a_second_pulse() {
        use bli_kvm_atx_transition::{AtxExecutor, AtxPulseRunner, AtxTransitionEngine};

        struct SlowRunner;
        impl AtxPulseRunner for SlowRunner {
            fn execute(&mut self, _action: AtxControlAction) -> Result<(), String> {
                std::thread::sleep(std::time::Duration::from_millis(1500));
                Ok(())
            }
        }

        let directory = tempfile::tempdir().unwrap();
        let engine = AtxTransitionEngine::open_with_executor(
            directory.path().join("journal.json"),
            AtxExecutor::Gpio(std::sync::Mutex::new(Box::new(SlowRunner))),
        )
        .unwrap();
        let state = std::sync::Arc::new(tokio::sync::Mutex::new(ControlV2State::with_atx_engine(
            engine,
        )));
        let atx = state.lock().await.atx_shared().unwrap();
        let confirmation = AtxConfirmation {
            session_binding: 9,
            confirmation_binding: [0x44; 32],
            confirmed_at_unix_ms: unix_millis(),
        };
        let first = tokio::spawn({
            let atx = atx.clone();
            async move {
                run_atx(
                    atx,
                    "0123456789abcdef0123456789abcdef",
                    AtxControlAction::PowerLong,
                    &confirmation,
                )
                .await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        // The shared state is free while the pulse runs.
        let begun = Instant::now();
        drop(state.lock().await);
        assert!(begun.elapsed() < std::time::Duration::from_millis(100));
        // A second pulse is refused at once.
        let begun = Instant::now();
        let second = run_atx(
            atx.clone(),
            "fedcba9876543210fedcba9876543210",
            AtxControlAction::Reset,
            &confirmation,
        )
        .await;
        assert_eq!(second, reject(ControlRejectionCode::CooldownActive));
        assert!(begun.elapsed() < std::time::Duration::from_millis(100));
        assert_eq!(first.await.unwrap(), ControlResponse::Applied);
    }
}
