use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tokio::sync::RwLock;

use super::token::random_token;

#[derive(Debug, Clone)]
pub struct Session {
    pub token: String,
    pub username: String,
    pub csrf_token: String,
    pub expires_at: Instant,
    pub expires_at_unix: u64,
    revoked: Arc<AtomicBool>,
}

impl Session {
    pub fn is_active(&self) -> bool {
        !self.revoked.load(Ordering::Acquire) && Instant::now() < self.expires_at
    }

    fn revoke(&self) {
        self.revoked.store(true, Ordering::Release);
    }
}

/// Idle auto-lock the reference applies until the operator changes it.
pub const DEFAULT_LOCK_AFTER_SECONDS: u64 = 3600;

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
    /// Device-wide idle auto-lock: after this many seconds without a request
    /// the session is revoked and the operator must log in again. `None` turns
    /// the policy off.
    lock_after_seconds: RwLock<Option<u64>>,
    last_seen: RwLock<HashMap<String, Instant>>,
}

impl SessionStore {
    /// Starts with the reference default idle auto-lock of one hour.
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_lock_after_seconds(DEFAULT_LOCK_AFTER_SECONDS)
    }

    /// A store whose idle auto-lock starts at `seconds` (zero: off).
    pub fn with_lock_after_seconds(seconds: u64) -> Self {
        Self {
            lock_after_seconds: RwLock::new((seconds != 0).then_some(seconds)),
            ..Self::default()
        }
    }

    pub async fn issue(&self, username: &str, ttl_secs: u64) -> Session {
        let token = random_token(32);
        let csrf_token = random_token(32);
        let ttl = Duration::from_secs(ttl_secs);
        let session = Session {
            token: token.clone(),
            username: username.into(),
            csrf_token,
            expires_at: Instant::now() + ttl,
            expires_at_unix: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .saturating_add(ttl)
                .as_secs(),
            revoked: Arc::new(AtomicBool::new(false)),
        };
        self.sessions
            .write()
            .await
            .insert(token.clone(), session.clone());
        self.last_seen
            .write()
            .await
            .insert(token.clone(), Instant::now());
        session
    }

    /// The configured idle auto-lock in seconds; `0` means disabled.
    pub async fn lock_after_seconds(&self) -> u64 {
        self.lock_after_seconds.read().await.unwrap_or(0)
    }

    /// Set the idle auto-lock policy. Zero disables it and clears the activity
    /// bookkeeping.
    pub async fn set_lock_after_seconds(&self, seconds: u64) {
        if seconds == 0 {
            *self.lock_after_seconds.write().await = None;
            self.last_seen.write().await.clear();
        } else {
            *self.lock_after_seconds.write().await = Some(seconds);
        }
    }

    pub async fn validate(&self, token: &str) -> Option<Session> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get(token).cloned()?;
        if !session.is_active() {
            if let Some(expired) = sessions.remove(token) {
                expired.revoke();
            }
            self.forget_activity(token).await;
            return None;
        }
        if let Some(seconds) = *self.lock_after_seconds.read().await {
            let idle = Duration::from_secs(seconds);
            let stale = {
                let mut seen = self.last_seen.write().await;
                match seen.get_mut(token) {
                    Some(last) if last.elapsed() >= idle => true,
                    Some(last) => {
                        *last = Instant::now();
                        false
                    }
                    None => {
                        seen.insert(token.to_string(), Instant::now());
                        false
                    }
                }
            };
            if stale {
                if let Some(expired) = sessions.remove(token) {
                    expired.revoke();
                }
                self.forget_activity(token).await;
                return None;
            }
        }
        Some(session)
    }

    async fn forget_activity(&self, token: &str) {
        self.last_seen.write().await.remove(token);
    }

    pub async fn revoke(&self, token: &str) {
        if let Some(session) = self.sessions.write().await.remove(token) {
            session.revoke();
        }
    }

    pub async fn revoke_user(&self, username: &str) -> Vec<String> {
        let mut revoked = Vec::new();
        self.sessions.write().await.retain(|_, session| {
            if session.username == username {
                session.revoke();
                revoked.push(session.token.clone());
                false
            } else {
                true
            }
        });
        let revoked_tokens = &revoked;
        self.last_seen
            .write()
            .await
            .retain(|token, _| !revoked_tokens.contains(token));
        revoked
    }

    /// Revoke every session except the caller's, used by a forced lock.
    pub async fn revoke_others(&self, token: &str) -> Vec<String> {
        let mut revoked = Vec::new();
        self.sessions.write().await.retain(|other, session| {
            if other == token {
                true
            } else {
                session.revoke();
                revoked.push(other.clone());
                false
            }
        });
        self.last_seen
            .write()
            .await
            .retain(|other, _| other == token);
        revoked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn revocation_is_visible_to_existing_session_clones() {
        let store = SessionStore::new();
        let session = store.issue("operator", 60).await;
        let existing_stream_session = session.clone();
        assert!(existing_stream_session.is_active());

        store.revoke(&session.token).await;

        assert!(!existing_stream_session.is_active());
        assert!(store.validate(&session.token).await.is_none());
    }

    #[tokio::test]
    async fn user_revocation_invalidates_every_existing_clone() {
        let store = SessionStore::new();
        let first = store.issue("operator", 60).await;
        let second = store.issue("operator", 60).await;
        let other = store.issue("other", 60).await;

        let mut revoked = store.revoke_user("operator").await;
        revoked.sort();

        assert!(!first.is_active());
        assert!(!second.is_active());
        assert!(other.is_active());
        let mut expected = vec![first.token, second.token];
        expected.sort();
        assert_eq!(revoked, expected);
    }
}
