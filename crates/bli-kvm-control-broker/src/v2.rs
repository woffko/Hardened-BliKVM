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
use bli_kvm_core::{HidLeaseRequest, PlatformControlRequestV2};

use crate::{DEFAULT_MAX_ATTESTATIONS, TargetAttestationRegistry};

#[cfg(feature = "hid-transition-profile")]
use bli_kvm_hid_adapter::HidLeaseIdentity;
#[cfg(feature = "hid-transition-profile")]
use bli_kvm_hid_policy::{HidControlPolicy, HidDecision, HidEligibility};
#[cfg(feature = "hid-transition-profile")]
use rand_core::{OsRng, RngCore};

pub struct ControlV2State {
    enabled: bool,
    attestations: TargetAttestationRegistry,
    #[cfg(feature = "hid-transition-profile")]
    hid: Option<HidControlPolicy>,
}

impl ControlV2State {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            #[cfg(feature = "hid-transition-profile")]
            hid: None,
        }
    }

    #[cfg(feature = "hid-transition-profile")]
    pub fn with_hid_policy(policy: HidControlPolicy) -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
            hid: Some(policy),
        }
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
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
            PlatformControlRequestV2::Atx { .. } => {
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
}
