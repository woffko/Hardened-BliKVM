use std::time::{Duration, Instant};

use bli_kvm_core::HidReport;

use crate::{HidReportTranslator, HidTranslationError, KvmdHidEvent};

pub const MAX_HID_LEASE_TTL: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HidLeaseIdentity {
    lease_id: [u8; 16],
    session_binding: u64,
    target_binding: [u8; 32],
}

impl HidLeaseIdentity {
    pub fn new(
        lease_id: [u8; 16],
        session_binding: u64,
        target_binding: [u8; 32],
    ) -> Result<Self, HidLeaseError> {
        if lease_id.iter().all(|byte| *byte == 0)
            || session_binding == 0
            || target_binding.iter().all(|byte| *byte == 0)
        {
            return Err(HidLeaseError::InvalidIdentity);
        }
        Ok(Self {
            lease_id,
            session_binding,
            target_binding,
        })
    }

    pub const fn session_binding(self) -> u64 {
        self.session_binding
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HidLeaseError {
    InvalidIdentity,
    InvalidTtl,
    Busy,
    NotActive,
    Mismatch,
    Expired,
    Translation(HidTranslationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidLeaseAcquisition {
    pub release_events: Vec<KvmdHidEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidLeaseBatch {
    pub events: Vec<KvmdHidEvent>,
    pub error: Option<HidLeaseError>,
}

impl HidLeaseBatch {
    pub fn accepted(&self) -> bool {
        self.error.is_none()
    }
}

#[derive(Debug, Clone, Copy)]
struct ActiveLease {
    identity: HidLeaseIdentity,
    expires_at: Instant,
}

#[derive(Debug, Default)]
pub struct HidLeaseManager {
    active: Option<ActiveLease>,
    translator: HidReportTranslator,
}

impl HidLeaseManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn acquire(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        ttl: Duration,
    ) -> Result<HidLeaseAcquisition, HidLeaseError> {
        validate_ttl(ttl)?;
        let release_events = if self.active.is_some_and(|lease| now >= lease.expires_at) {
            self.active = None;
            self.translator.release_all()
        } else {
            if self.active.is_some() {
                return Err(HidLeaseError::Busy);
            }
            Vec::new()
        };
        self.active = Some(ActiveLease {
            identity,
            expires_at: now + ttl,
        });
        Ok(HidLeaseAcquisition { release_events })
    }

    pub fn renew(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        ttl: Duration,
    ) -> HidLeaseBatch {
        if let Err(error) = validate_ttl(ttl) {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(error),
            };
        }
        let Some(active) = self.active else {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::NotActive),
            };
        };
        if active.identity != identity {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::Mismatch),
            };
        }
        if now >= active.expires_at {
            return self.expire_with_error(HidLeaseError::Expired);
        }
        self.active = Some(ActiveLease {
            identity,
            expires_at: now + ttl,
        });
        HidLeaseBatch {
            events: Vec::new(),
            error: None,
        }
    }

    pub fn translate(
        &mut self,
        identity: HidLeaseIdentity,
        now: Instant,
        report: &HidReport,
    ) -> HidLeaseBatch {
        let Some(active) = self.active else {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::NotActive),
            };
        };
        if active.identity != identity {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::Mismatch),
            };
        }
        if now >= active.expires_at {
            return self.expire_with_error(HidLeaseError::Expired);
        }
        let translated = self.translator.translate(report);
        if let Some(error) = translated.error {
            self.active = None;
            return HidLeaseBatch {
                events: translated.events,
                error: Some(HidLeaseError::Translation(error)),
            };
        }
        HidLeaseBatch {
            events: translated.events,
            error: None,
        }
    }

    pub fn release(&mut self, identity: HidLeaseIdentity) -> HidLeaseBatch {
        let Some(active) = self.active else {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::NotActive),
            };
        };
        if active.identity != identity {
            return HidLeaseBatch {
                events: Vec::new(),
                error: Some(HidLeaseError::Mismatch),
            };
        }
        self.active = None;
        HidLeaseBatch {
            events: self.translator.release_all(),
            error: None,
        }
    }

    pub fn revoke_session(&mut self, session_binding: u64) -> Vec<KvmdHidEvent> {
        if self
            .active
            .is_some_and(|active| active.identity.session_binding() == session_binding)
        {
            self.disconnect()
        } else {
            Vec::new()
        }
    }

    pub fn expire(&mut self, now: Instant) -> Vec<KvmdHidEvent> {
        if self.active.is_some_and(|active| now >= active.expires_at) {
            self.disconnect()
        } else {
            Vec::new()
        }
    }

    pub fn disconnect(&mut self) -> Vec<KvmdHidEvent> {
        self.active = None;
        self.translator.release_all()
    }

    pub fn release_snapshot(&self) -> Vec<KvmdHidEvent> {
        self.translator.release_snapshot()
    }

    fn expire_with_error(&mut self, error: HidLeaseError) -> HidLeaseBatch {
        HidLeaseBatch {
            events: self.disconnect(),
            error: Some(error),
        }
    }
}

fn validate_ttl(ttl: Duration) -> Result<(), HidLeaseError> {
    if ttl.is_zero() || ttl > MAX_HID_LEASE_TTL {
        Err(HidLeaseError::InvalidTtl)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{KvmdHidEvent, KvmdKeyCode};

    fn identity(seed: u8, session: u64) -> HidLeaseIdentity {
        HidLeaseIdentity::new([seed; 16], session, [seed; 32]).unwrap()
    }

    fn key(name: &'static str, pressed: bool) -> KvmdHidEvent {
        let usage = match name {
            "KeyA" => 4,
            "KeyB" => 5,
            _ => panic!("unsupported test key"),
        };
        KvmdHidEvent::Key {
            key: KvmdKeyCode::from_usb_usage(usage).unwrap(),
            pressed,
        }
    }

    #[test]
    fn identity_and_ttl_are_bounded() {
        assert_eq!(
            HidLeaseIdentity::new([0; 16], 1, [1; 32]),
            Err(HidLeaseError::InvalidIdentity)
        );
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        assert_eq!(
            manager.acquire(identity(1, 1), now, Duration::ZERO),
            Err(HidLeaseError::InvalidTtl)
        );
        assert_eq!(
            manager.acquire(identity(1, 1), now, Duration::from_secs(31)),
            Err(HidLeaseError::InvalidTtl)
        );
    }

    #[test]
    fn exact_lease_identity_is_required_for_reports_and_renewal() {
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        let owner = identity(1, 7);
        let other = identity(2, 7);
        manager
            .acquire(owner, now, Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            manager
                .translate(other, now, &HidReport::Keyboard { data: [0; 8] })
                .error,
            Some(HidLeaseError::Mismatch)
        );
        assert_eq!(
            manager.renew(other, now, Duration::from_secs(10)).error,
            Some(HidLeaseError::Mismatch)
        );
    }

    #[test]
    fn active_lease_is_exclusive_and_exact_renewal_extends_it() {
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        let owner = identity(1, 7);
        manager
            .acquire(owner, now, Duration::from_secs(10))
            .unwrap();
        assert_eq!(
            manager.acquire(identity(2, 8), now, Duration::from_secs(10)),
            Err(HidLeaseError::Busy)
        );
        assert!(
            manager
                .renew(owner, now + Duration::from_secs(9), Duration::from_secs(10))
                .accepted()
        );
        assert!(
            manager
                .translate(
                    owner,
                    now + Duration::from_secs(15),
                    &HidReport::Keyboard { data: [0; 8] }
                )
                .accepted()
        );
    }

    #[test]
    fn expiry_releases_pressed_state_and_clears_the_lease() {
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        let owner = identity(1, 7);
        manager.acquire(owner, now, Duration::from_secs(2)).unwrap();
        let pressed = manager.translate(
            owner,
            now,
            &HidReport::Keyboard {
                data: [0, 0, 4, 0, 0, 0, 0, 0],
            },
        );
        assert_eq!(pressed.events, vec![key("KeyA", true)]);

        let expired = manager.translate(
            owner,
            now + Duration::from_secs(2),
            &HidReport::Keyboard { data: [0; 8] },
        );
        assert_eq!(expired.error, Some(HidLeaseError::Expired));
        assert_eq!(expired.events, vec![key("KeyA", false)]);
        assert_eq!(
            manager
                .translate(owner, now, &HidReport::Keyboard { data: [0; 8] })
                .error,
            Some(HidLeaseError::NotActive)
        );
    }

    #[test]
    fn translation_failure_revokes_lease_after_release_all() {
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        let owner = identity(1, 7);
        manager
            .acquire(owner, now, Duration::from_secs(10))
            .unwrap();
        let _ = manager.translate(
            owner,
            now,
            &HidReport::Keyboard {
                data: [0, 0, 4, 0, 0, 0, 0, 0],
            },
        );
        let rejected = manager.translate(
            owner,
            now,
            &HidReport::Keyboard {
                data: [0, 0, 50, 0, 0, 0, 0, 0],
            },
        );
        assert_eq!(
            rejected.error,
            Some(HidLeaseError::Translation(
                HidTranslationError::UnsupportedKeycode
            ))
        );
        assert_eq!(rejected.events, vec![key("KeyA", false)]);
        assert_eq!(
            manager.renew(owner, now, Duration::from_secs(10)).error,
            Some(HidLeaseError::NotActive)
        );
    }

    #[test]
    fn session_revoke_disconnect_and_expired_reacquire_release_state() {
        let mut manager = HidLeaseManager::new();
        let now = Instant::now();
        let first = identity(1, 7);
        manager.acquire(first, now, Duration::from_secs(1)).unwrap();
        let _ = manager.translate(
            first,
            now,
            &HidReport::Keyboard {
                data: [0, 0, 4, 0, 0, 0, 0, 0],
            },
        );
        assert_eq!(
            manager
                .acquire(
                    identity(2, 8),
                    now + Duration::from_secs(1),
                    Duration::from_secs(5)
                )
                .unwrap()
                .release_events,
            vec![key("KeyA", false)]
        );
        let _ = manager.translate(
            identity(2, 8),
            now + Duration::from_secs(1),
            &HidReport::Keyboard {
                data: [0, 0, 5, 0, 0, 0, 0, 0],
            },
        );
        assert_eq!(manager.revoke_session(8), vec![key("KeyB", false)]);
        assert!(manager.disconnect().is_empty());
    }
}
