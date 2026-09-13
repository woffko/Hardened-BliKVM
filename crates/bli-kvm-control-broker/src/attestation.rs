//! Bounded, expiring, fail-closed storage for broker-side target attestations.
//!
//! Attestation entries are created only by the network server after an explicit
//! operator safe-target/focus confirmation. This registry never enables a
//! control capability by itself; HID lease and ATX requests still require the
//! compile-time gate and the runtime policy.

use std::{
    fmt,
    time::{Duration, Instant},
};

use bli_kvm_core::{ControlV2ValidationError, TargetAttestation};

pub const DEFAULT_MAX_ATTESTATIONS: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetAttestationError {
    Invalid(ControlV2ValidationError),
    Capacity,
}

impl fmt::Display for TargetAttestationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid(_) => "target attestation is invalid",
            Self::Capacity => "target attestation capacity is exhausted",
        })
    }
}

impl std::error::Error for TargetAttestationError {}

#[derive(Debug, Clone, Copy)]
struct AttestationEntry {
    session_binding: u64,
    target_binding: [u8; 32],
    focus_binding: [u8; 16],
    expires_at: Instant,
}

#[derive(Debug)]
pub struct TargetAttestationRegistry {
    max_entries: usize,
    entries: Vec<AttestationEntry>,
}

impl TargetAttestationRegistry {
    pub fn new(max_entries: usize) -> Self {
        Self {
            max_entries: max_entries.max(1),
            entries: Vec::new(),
        }
    }

    pub fn attest(
        &mut self,
        attestation: &TargetAttestation,
        now: Instant,
    ) -> Result<(), TargetAttestationError> {
        attestation
            .validate()
            .map_err(TargetAttestationError::Invalid)?;
        self.sweep(now);
        self.entries
            .retain(|entry| entry.session_binding != attestation.session_binding);
        if self.entries.len() >= self.max_entries {
            return Err(TargetAttestationError::Capacity);
        }
        self.entries.push(AttestationEntry {
            session_binding: attestation.session_binding,
            target_binding: attestation.target_binding,
            focus_binding: attestation.focus_binding,
            expires_at: now + Duration::from_secs(u64::from(attestation.ttl_seconds)),
        });
        Ok(())
    }

    pub fn release(
        &mut self,
        session_binding: u64,
        target_binding: &[u8; 32],
        now: Instant,
    ) -> bool {
        self.sweep(now);
        let before = self.entries.len();
        self.entries.retain(|entry| {
            !(entry.session_binding == session_binding && &entry.target_binding == target_binding)
        });
        self.entries.len() != before
    }

    pub fn authorize(&self, session_binding: u64, target_binding: &[u8; 32], now: Instant) -> bool {
        self.entries.iter().any(|entry| {
            entry.session_binding == session_binding
                && &entry.target_binding == target_binding
                && now < entry.expires_at
        })
    }

    pub fn sweep(&mut self, now: Instant) {
        self.entries.retain(|entry| now < entry.expires_at);
    }

    pub fn focus_binding(&self, session_binding: u64, now: Instant) -> Option<[u8; 16]> {
        self.entries
            .iter()
            .find(|entry| entry.session_binding == session_binding && now < entry.expires_at)
            .map(|entry| entry.focus_binding)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: [u8; 32] = [0x11; 32];
    const OTHER_TARGET: [u8; 32] = [0x12; 32];
    const FOCUS: [u8; 16] = [0x22; 16];

    fn attestation(
        session_binding: u64,
        target_binding: [u8; 32],
        ttl_seconds: u16,
    ) -> TargetAttestation {
        TargetAttestation {
            session_binding,
            target_binding,
            focus_binding: FOCUS,
            ttl_seconds,
        }
    }

    #[test]
    fn authorize_requires_the_exact_session_and_target_pair() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS);
        assert!(registry.attest(&attestation(9, TARGET, 30), now).is_ok());
        assert!(registry.authorize(9, &TARGET, now));
        assert!(!registry.authorize(10, &TARGET, now));
        assert!(!registry.authorize(9, &OTHER_TARGET, now));
        assert_eq!(registry.focus_binding(9, now), Some(FOCUS));
        assert_eq!(registry.focus_binding(10, now), None);
    }

    #[test]
    fn expired_attestation_is_denied_and_swept() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS);
        registry.attest(&attestation(9, TARGET, 5), now).unwrap();
        let later = now + Duration::from_secs(5);
        assert!(!registry.authorize(9, &TARGET, later));
        registry.sweep(later);
        assert!(registry.is_empty());
    }

    #[test]
    fn re_attestation_replaces_and_can_move_the_target() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS);
        registry.attest(&attestation(9, TARGET, 30), now).unwrap();
        registry
            .attest(&attestation(9, OTHER_TARGET, 30), now)
            .unwrap();
        assert_eq!(registry.len(), 1);
        assert!(!registry.authorize(9, &TARGET, now));
        assert!(registry.authorize(9, &OTHER_TARGET, now));
    }

    #[test]
    fn release_is_identity_scoped_and_idempotent() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS);
        registry.attest(&attestation(9, TARGET, 30), now).unwrap();
        assert!(!registry.release(9, &OTHER_TARGET, now));
        assert!(registry.release(9, &TARGET, now));
        assert!(!registry.release(9, &TARGET, now));
        assert!(!registry.authorize(9, &TARGET, now));
    }

    #[test]
    fn capacity_is_bounded_and_expired_entries_are_reclaimed_first() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(1);
        registry.attest(&attestation(9, TARGET, 30), now).unwrap();
        assert_eq!(
            registry.attest(&attestation(10, TARGET, 30), now),
            Err(TargetAttestationError::Capacity)
        );
        let later = now + Duration::from_secs(31);
        assert!(registry.attest(&attestation(10, TARGET, 30), later).is_ok());
        assert_eq!(registry.len(), 1);
        assert!(registry.authorize(10, &TARGET, later));
    }

    #[test]
    fn invalid_attestation_is_rejected_before_storage() {
        let now = Instant::now();
        let mut registry = TargetAttestationRegistry::new(DEFAULT_MAX_ATTESTATIONS);
        assert_eq!(
            registry.attest(&attestation(0, TARGET, 30), now),
            Err(TargetAttestationError::Invalid(
                ControlV2ValidationError::InvalidSessionBinding
            ))
        );
        assert!(registry.is_empty());
    }
}
