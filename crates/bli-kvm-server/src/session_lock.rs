//! Device-wide session auto-lock: how long an idle operator session may live
//! before the GUI asks for the password again.
//!
//! The reference settings expose this as a duration plus an explicit forced
//! lock; both are unprivileged, so the server owns them directly.

use axum::{
    Extension, Json,
    extract::State,
    http::{HeaderMap, header},
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::{cookie::session_cookie, middleware::CurrentSession},
    error::{ApiResponse, AppError, AppResult},
};

/// Largest accepted idle duration: one day.
pub const MAX_LOCK_SECONDS: u64 = 86_400;
/// The operator's choice survives server restarts and updates.
pub const SESSION_LOCK_FILE: &str = "session-lock.json";

#[derive(Debug, Serialize, Deserialize)]
struct Persisted {
    duration: u64,
}

/// The persisted idle duration, or the reference default.
pub fn load(state_dir: &std::path::Path) -> u64 {
    std::fs::read(state_dir.join(SESSION_LOCK_FILE))
        .ok()
        .filter(|bytes| bytes.len() <= 256)
        .and_then(|bytes| serde_json::from_slice::<Persisted>(&bytes).ok())
        .map(|persisted| persisted.duration)
        .filter(|duration| *duration <= MAX_LOCK_SECONDS)
        .unwrap_or(crate::auth::session::DEFAULT_LOCK_AFTER_SECONDS)
}

fn store(state_dir: &std::path::Path, duration: u64) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(state_dir)?;
    let path = state_dir.join(SESSION_LOCK_FILE);
    let temporary = path.with_extension("new");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&serde_json::to_vec(&Persisted { duration }).map_err(std::io::Error::other)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temporary, path)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionLockView {
    /// Seconds a session lives, and idles at most, before a forced re-login;
    /// `0` disables the auto-lock.
    pub duration: u64,
    /// After a change, when the caller's retimed session ends.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionLockBody {
    pub duration: u64,
    /// When true, every other active session is revoked immediately.
    #[serde(default)]
    pub lock_now: bool,
}

pub(crate) async fn get_session_lock(
    State(state): State<AppState>,
) -> Json<ApiResponse<SessionLockView>> {
    Json(ApiResponse::ok(SessionLockView {
        duration: state.sessions.lock_after_seconds().await,
        expires_at: None,
    }))
}

pub(crate) async fn set_session_lock(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SessionLockBody>,
) -> AppResult<(HeaderMap, Json<ApiResponse<SessionLockView>>)> {
    if body.duration > MAX_LOCK_SECONDS {
        return Err(AppError::BadRequest(
            "the session lock duration is too large".into(),
        ));
    }
    state.sessions.set_lock_after_seconds(body.duration).await;
    if let Err(error) = store(&state.options.state_dir, body.duration) {
        eprintln!("session lock: the setting could not be saved: {error}");
        return Err(AppError::ServiceUnavailable(
            "the session lock setting could not be saved".into(),
        ));
    }
    if body.lock_now {
        let revoked = state.sessions.revoke_others(&session.token).await;
        for token in revoked {
            crate::hid::release_session_hid(&state, &token).await;
        }
    }
    // As in the reference, the caller's own session takes the new lifetime.
    let mut headers = HeaderMap::new();
    let lifetime = state
        .sessions
        .lifetime_seconds(state.options.session_ttl_secs)
        .await;
    let retimed = state.sessions.retime(&session.token, lifetime).await;
    if let Some(retimed) = &retimed {
        headers.insert(
            header::SET_COOKIE,
            session_cookie(&retimed.token, lifetime, state.options.secure_cookies)?,
        );
    }
    Ok((
        headers,
        Json(ApiResponse::ok(SessionLockView {
            duration: state.sessions.lock_after_seconds().await,
            expires_at: retimed.map(|session| session.expires_at_unix),
        })),
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_duration_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(super::load(dir.path()), 3600);
        super::store(&dir.path().join("state"), 0).unwrap();
        assert_eq!(super::load(&dir.path().join("state")), 0);
        super::store(&dir.path().join("state"), 900).unwrap();
        assert_eq!(super::load(&dir.path().join("state")), 900);
        std::fs::write(
            dir.path().join("state/session-lock.json"),
            b"{\"duration\":999999}",
        )
        .unwrap();
        assert_eq!(super::load(&dir.path().join("state")), 3600);
    }
}
