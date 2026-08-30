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

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: RwLock<HashMap<String, Session>>,
}

impl SessionStore {
    pub fn new() -> Self {
        Self::default()
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
        self.sessions.write().await.insert(token, session.clone());
        session
    }

    pub async fn validate(&self, token: &str) -> Option<Session> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get(token).cloned()?;
        if !session.is_active() {
            if let Some(expired) = sessions.remove(token) {
                expired.revoke();
            }
            return None;
        }
        Some(session)
    }

    pub async fn revoke(&self, token: &str) {
        if let Some(session) = self.sessions.write().await.remove(token) {
            session.revoke();
        }
    }

    pub async fn revoke_user(&self, username: &str) {
        self.sessions.write().await.retain(|_, session| {
            if session.username == username {
                session.revoke();
                false
            } else {
                true
            }
        });
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

        store.revoke_user("operator").await;

        assert!(!first.is_active());
        assert!(!second.is_active());
        assert!(other.is_active());
    }
}
