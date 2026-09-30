//! Derived session, target, and focus bindings from an explicit operator
//! safe-target/focus confirmation.
//!
//! The network server is the only intended caller. It supplies the
//! authenticated session identity, a stable hashed target identity, a fresh
//! focus nonce, and the operator's confirmation time. The opaque bindings are
//! stable for one authenticated session and target while the focus binding is
//! refreshed by every confirmation. Clients cannot supply any binding. This
//! crate never enables HID; the broker still gates every mutation.

use bli_kvm_core::TargetAttestation;
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const MAX_FOCUS_CONFIRMATION_AGE_MS: u64 = 15_000;
const DOMAIN: &[u8] = b"hardened-blikvm-hid-focus-v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusConfirmation {
    pub operator_confirmed: bool,
    pub session_id: u64,
    pub target_id: [u8; 16],
    pub focus_nonce: [u8; 16],
    pub confirmed_at_unix_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FocusBindings {
    pub session_binding: u64,
    pub target_binding: [u8; 32],
    pub focus_binding: [u8; 16],
}

impl FocusBindings {
    pub const fn attestation(self, ttl_seconds: u16) -> TargetAttestation {
        TargetAttestation {
            session_binding: self.session_binding,
            target_binding: self.target_binding,
            focus_binding: self.focus_binding,
            ttl_seconds,
        }
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum FocusError {
    #[error("operator confirmation is missing")]
    NotConfirmed,
    #[error("session identity is invalid")]
    InvalidSession,
    #[error("target identity is invalid")]
    InvalidTarget,
    #[error("focus nonce is invalid")]
    InvalidNonce,
    #[error("focus confirmation is in the future")]
    ConfirmationInFuture,
    #[error("focus confirmation has expired")]
    ConfirmationExpired,
    #[error("derived binding is degenerate")]
    DegenerateBindings,
}

pub fn derive_focus_bindings(
    confirmation: &FocusConfirmation,
    now_ms: u64,
) -> Result<FocusBindings, FocusError> {
    if !confirmation.operator_confirmed {
        return Err(FocusError::NotConfirmed);
    }
    if confirmation.session_id == 0 {
        return Err(FocusError::InvalidSession);
    }
    if confirmation.target_id.iter().all(|byte| *byte == 0) {
        return Err(FocusError::InvalidTarget);
    }
    if confirmation.focus_nonce.iter().all(|byte| *byte == 0) {
        return Err(FocusError::InvalidNonce);
    }
    if confirmation.confirmed_at_unix_ms == 0 || confirmation.confirmed_at_unix_ms > now_ms {
        return Err(FocusError::ConfirmationInFuture);
    }
    if now_ms - confirmation.confirmed_at_unix_ms > MAX_FOCUS_CONFIRMATION_AGE_MS {
        return Err(FocusError::ConfirmationExpired);
    }

    let session_binding = u64::from_be_bytes(
        derive_session(confirmation)[..8]
            .try_into()
            .map_err(|_| FocusError::DegenerateBindings)?,
    );
    let target_binding: [u8; 32] = derive_target(confirmation)[..32]
        .try_into()
        .map_err(|_| FocusError::DegenerateBindings)?;
    let focus_binding: [u8; 16] = derive_focus(confirmation)[..16]
        .try_into()
        .map_err(|_| FocusError::DegenerateBindings)?;

    if session_binding == 0
        || target_binding.iter().all(|byte| *byte == 0)
        || focus_binding.iter().all(|byte| *byte == 0)
    {
        return Err(FocusError::DegenerateBindings);
    }

    Ok(FocusBindings {
        session_binding,
        target_binding,
        focus_binding,
    })
}

fn binding_hasher(binding: &[u8]) -> Sha256 {
    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    hasher.update((binding.len() as u64).to_be_bytes());
    hasher.update(binding);
    hasher
}

fn derive_session(confirmation: &FocusConfirmation) -> [u8; 32] {
    let mut hasher = binding_hasher(b"session");
    hasher.update(confirmation.session_id.to_be_bytes());
    hasher.finalize().into()
}

fn derive_target(confirmation: &FocusConfirmation) -> [u8; 32] {
    let mut hasher = binding_hasher(b"target");
    hasher.update(confirmation.session_id.to_be_bytes());
    hasher.update(confirmation.target_id);
    hasher.finalize().into()
}

fn derive_focus(confirmation: &FocusConfirmation) -> [u8; 32] {
    let mut hasher = binding_hasher(b"focus");
    hasher.update(confirmation.session_id.to_be_bytes());
    hasher.update(confirmation.target_id);
    hasher.update(confirmation.focus_nonce);
    hasher.update(confirmation.confirmed_at_unix_ms.to_be_bytes());
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confirmation() -> FocusConfirmation {
        FocusConfirmation {
            operator_confirmed: true,
            session_id: 7,
            target_id: [0x11; 16],
            focus_nonce: [0x22; 16],
            confirmed_at_unix_ms: 1_000,
        }
    }

    #[test]
    fn derivation_is_deterministic_and_domain_separated() {
        let bindings = derive_focus_bindings(&confirmation(), 1_500).unwrap();
        assert_eq!(
            bindings,
            derive_focus_bindings(&confirmation(), 1_500).unwrap()
        );
        assert_ne!(bindings.target_binding, {
            let mut bytes = [0_u8; 32];
            bytes[..16].copy_from_slice(&bindings.focus_binding);
            bytes
        });
        assert!(bindings.session_binding != 0);
    }

    #[test]
    fn focus_refresh_changes_only_the_focus_binding() {
        let base = derive_focus_bindings(&confirmation(), 1_500).unwrap();
        let mut changed = confirmation();
        changed.focus_nonce = [0x23; 16];
        let refreshed = derive_focus_bindings(&changed, 1_500).unwrap();
        assert_eq!(base.session_binding, refreshed.session_binding);
        assert_eq!(base.target_binding, refreshed.target_binding);
        assert_ne!(base.focus_binding, refreshed.focus_binding);

        let mut changed = confirmation();
        changed.confirmed_at_unix_ms = 1_001;
        let refreshed = derive_focus_bindings(&changed, 1_500).unwrap();
        assert_eq!(base.session_binding, refreshed.session_binding);
        assert_eq!(base.target_binding, refreshed.target_binding);
        assert_ne!(base.focus_binding, refreshed.focus_binding);
    }

    #[test]
    fn session_and_target_changes_update_the_lease_identity() {
        let base = derive_focus_bindings(&confirmation(), 1_500).unwrap();
        let mut changed = confirmation();
        changed.session_id = 8;
        let other_session = derive_focus_bindings(&changed, 1_500).unwrap();
        assert_ne!(base.session_binding, other_session.session_binding);
        assert_ne!(base.target_binding, other_session.target_binding);
        assert_ne!(base.focus_binding, other_session.focus_binding);

        let mut changed = confirmation();
        changed.target_id = [0x12; 16];
        let other_target = derive_focus_bindings(&changed, 1_500).unwrap();
        assert_eq!(base.session_binding, other_target.session_binding);
        assert_ne!(base.target_binding, other_target.target_binding);
        assert_ne!(base.focus_binding, other_target.focus_binding);
    }

    #[test]
    fn confirmation_must_be_explicit_recent_and_bound() {
        for (input, now, error) in [
            (
                FocusConfirmation {
                    operator_confirmed: false,
                    ..confirmation()
                },
                1_500,
                FocusError::NotConfirmed,
            ),
            (
                FocusConfirmation {
                    session_id: 0,
                    ..confirmation()
                },
                1_500,
                FocusError::InvalidSession,
            ),
            (
                FocusConfirmation {
                    target_id: [0; 16],
                    ..confirmation()
                },
                1_500,
                FocusError::InvalidTarget,
            ),
            (
                FocusConfirmation {
                    focus_nonce: [0; 16],
                    ..confirmation()
                },
                1_500,
                FocusError::InvalidNonce,
            ),
            (confirmation(), 999, FocusError::ConfirmationInFuture),
            (
                confirmation(),
                1_000 + MAX_FOCUS_CONFIRMATION_AGE_MS + 1,
                FocusError::ConfirmationExpired,
            ),
        ] {
            assert_eq!(derive_focus_bindings(&input, now), Err(error));
        }
    }

    #[test]
    fn derived_attestation_is_acceptable_to_the_v2_validator() {
        let bindings = derive_focus_bindings(&confirmation(), 1_500).unwrap();
        let attestation = bindings.attestation(30);
        assert_eq!(attestation.validate(), Ok(()));
    }
}
