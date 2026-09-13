//! Control protocol v2 routing for the denial-by-default broker.
//!
//! Shipped builds construct [`ControlV2State::disabled`], which rejects every
//! v2 request with `capability_disabled` and opens no backend. A separate
//! `hid-transition-profile` feature can attach a HID control policy built from
//! the dedicated credential and sandbox; even then, leases require a current
//! target attestation and the capability is enabled only for that profile.

use std::time::Instant;

#[cfg(feature = "hid-transition-profile")]
use std::time::Duration;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::{AtxConfirmation, AtxControlAction, HidLeaseRequest, PlatformControlRequestV2};

use crate::{DEFAULT_MAX_ATTESTATIONS, TargetAttestationRegistry};

#[cfg(feature = "atx-transition-profile")]
use bli_kvm_atx_policy::{AtxEligibility, AtxOperation, AtxRejection};
#[cfg(feature = "atx-transition-profile")]
use bli_kvm_atx_transition::{AtxTransitionEngine, AtxTransitionOutcome};
#[cfg(feature = "hid-transition-profile")]
use bli_kvm_hid_adapter::HidLeaseIdentity;
#[cfg(feature = "hid-transition-profile")]
use bli_kvm_hid_policy::{HidControlPolicy, HidDecision, HidEligibility};
#[cfg(feature = "hid-transition-profile")]
use rand_core::{OsRng, RngCore};

pub struct ControlV2State {
    enabled: bool,
    attestations: TargetAttestationRegistry,
    #[cfg(feature = "atx-transition-profile")]
    atx: Option<AtxTransitionEngine>,
    #[cfg(feature = "hid-transition-profile")]
    hid: Option<HidControlPolicy>,
}

impl ControlV2State {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(feature = "atx-transition-profile")]
            atx: None,
            #[cfg(feature = "hid-transition-profile")]
            hid: None,
        }
    }

    #[cfg(feature = "hid-transition-profile")]
    pub fn with_hid_policy(policy: HidControlPolicy) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(feature = "atx-transition-profile")]
            atx: None,
            hid: Some(policy),
        }
    }

    #[cfg(feature = "atx-transition-profile")]
    pub fn with_atx_engine(engine: AtxTransitionEngine) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            atx: Some(engine),
            #[cfg(feature = "hid-transition-profile")]
            hid: None,
        }
    }

    /// Serves both transition backends from one state, for the restricted
    /// control test profile. Only a build with both profile features compiles
    /// this constructor; HID requests still require a current attestation and
    /// ATX requests still require a reviewed confirmation.
    #[cfg(all(feature = "hid-transition-profile", feature = "atx-transition-profile"))]
    pub fn with_hid_and_atx(hid: HidControlPolicy, atx: AtxTransitionEngine) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            atx: Some(atx),
            hid: Some(hid),
        }
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(feature = "atx-transition-profile")]
            atx: None,
            #[cfg(feature = "hid-transition-profile")]
            hid: None,
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
            PlatformControlRequestV2::VirtualMedia { .. } => {
                reject(ControlRejectionCode::BackendUnavailable)
            }
        }
    }

    #[cfg(not(feature = "hid-transition-profile"))]
    async fn handle_hid_lease(
        &mut self,
        _lease: &HidLeaseRequest,
        _now: Instant,
    ) -> ControlResponse {
        reject(ControlRejectionCode::BackendUnavailable)
    }

    #[cfg(feature = "hid-transition-profile")]
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

#[cfg(not(feature = "atx-transition-profile"))]
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

#[cfg(feature = "atx-transition-profile")]
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

#[cfg(feature = "atx-transition-profile")]
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

#[cfg(feature = "atx-transition-profile")]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(feature = "atx-transition-profile")]
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
