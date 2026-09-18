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

pub struct ControlV2State {
    enabled: bool,
    attestations: TargetAttestationRegistry,
    #[cfg(any(feature = "atx-transition-profile", feature = "gpio-atx-profile"))]
    atx: Option<AtxTransitionEngine>,
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
            atx: Some(engine),
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
            atx: Some(atx),
            #[cfg(any(feature = "media-transition-profile", feature = "gadget-media-profile"))]
            media: None,
            hid: Some(hid),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
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
            atx: Some(atx),
            media: Some(media),
            hid: Some(hid),
            #[cfg(feature = "system-action-profile")]
            system_action: None,
            #[cfg(feature = "network-write-profile")]
            #[cfg(feature = "network-write-profile")]
            network_write: None,
            video_settings: None,
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

    fn handle_video_settings_state(&self) -> ControlResponse {
        let Some(state) = self.video_settings.as_ref() else {
            return reject(ControlRejectionCode::CapabilityDisabled);
        };
        let current = state.current();
        ControlResponse::VideoState {
            fps: current.fps,
            quality: current.quality,
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
            board_status: None,
            #[cfg(feature = "network-write-profile")]
            wol: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
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
        let Some(engine) = self.atx.as_mut() else {
            return reject(ControlRejectionCode::BackendUnavailable);
        };
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
        match engine.execute(operation, eligibility, now_ms).await {
            Ok(AtxTransitionOutcome::Applied | AtxTransitionOutcome::ReplayedApplied) => {
                ControlResponse::Applied
            }
            Ok(AtxTransitionOutcome::ReplayedRejected | AtxTransitionOutcome::BackendFailed) => {
                reject(ControlRejectionCode::BackendRejected)
            }
            Ok(AtxTransitionOutcome::Rejected(rejection)) => reject(atx_code(rejection)),
            Err(_) => reject(ControlRejectionCode::Internal),
        }
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
            Ok(
                MediaTransitionOutcome::ReplayedRejected | MediaTransitionOutcome::BackendFailed,
            ) => reject(ControlRejectionCode::BackendRejected),
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
    use bli_kvm_core::{
        AtxConfirmation, AtxControlAction, HidLeaseRequest, HidReport, TargetAttestation,
    };

    use super::*;

    const TARGET: [u8; 32] = [0x11; 32];
    const FOCUS: [u8; 16] = [0x22; 16];
    const LEASE: [u8; 16] = [0x33; 16];

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
}
