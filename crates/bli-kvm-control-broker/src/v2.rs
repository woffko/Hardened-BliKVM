//! Control protocol v2 routing for the denial-by-default broker.
//!
//! Shipped builds construct [`ControlV2State::disabled`], which rejects every
//! v2 request with `capability_disabled` and opens no backend. A separate test
//! profile can construct an enabled state that stores target attestations in
//! the bounded registry while HID leases and ATX actions still fail closed
//! because no backend is attached.

use std::time::Instant;

use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
use bli_kvm_core::PlatformControlRequestV2;

use crate::{DEFAULT_MAX_ATTESTATIONS, TargetAttestationRegistry};

pub struct ControlV2State {
    enabled: bool,
    attestations: TargetAttestationRegistry,
}

impl ControlV2State {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
        }
    }

    #[cfg(test)]
    fn enabled_for_test() -> Self {
        Self {
            enabled: true,
            attestations: TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn handle(&mut self, request: &PlatformControlRequestV2, now: Instant) -> ControlResponse {
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
            PlatformControlRequestV2::HidLease { .. } => {
                reject(ControlRejectionCode::BackendUnavailable)
            }
            PlatformControlRequestV2::Atx { .. } => {
                reject(ControlRejectionCode::BackendUnavailable)
            }
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

    #[test]
    fn disabled_state_rejects_every_v2_request() {
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
                state.handle(request, now),
                ControlResponse::Rejected {
                    code: ControlRejectionCode::CapabilityDisabled
                }
            );
        }
    }

    #[test]
    fn enabled_state_stores_and_releases_attestations_only() {
        let now = Instant::now();
        let mut state = ControlV2State::enabled_for_test();
        assert_eq!(
            state.handle(
                &PlatformControlRequestV2::TargetAttest {
                    attestation: attestation(30)
                },
                now
            ),
            ControlResponse::Applied
        );
        assert!(state.attestations.authorize(9, &TARGET, now));
        assert_eq!(
            state.handle(
                &PlatformControlRequestV2::TargetRelease {
                    release: bli_kvm_core::TargetRelease {
                        session_binding: 9,
                        target_binding: TARGET,
                    }
                },
                now
            ),
            ControlResponse::Applied
        );
        assert!(!state.attestations.authorize(9, &TARGET, now));

        assert_eq!(
            state.handle(
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
            ),
            ControlResponse::Rejected {
                code: ControlRejectionCode::BackendUnavailable
            }
        );
    }

    #[test]
    fn invalid_attestation_is_rejected_in_enabled_state() {
        let now = Instant::now();
        let mut state = ControlV2State::enabled_for_test();
        assert_eq!(
            state.handle(
                &PlatformControlRequestV2::TargetAttest {
                    attestation: attestation(0)
                },
                now
            ),
            ControlResponse::Rejected {
                code: ControlRejectionCode::InvalidState
            }
        );
    }
}
