//! Device-wide session auto-lock: how long an idle operator session may live
//! before the GUI asks for the password again.
//!
//! The reference settings expose this as a duration plus an explicit forced
//! lock; both are unprivileged, so the server owns them directly.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

/// Largest accepted idle duration: one day.
pub const MAX_LOCK_SECONDS: u64 = 86_400;

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionLockView {
    /// Idle seconds before a forced re-login; `0` disables the auto-lock.
    pub duration: u64,
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
    }))
}

pub(crate) async fn set_session_lock(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SessionLockBody>,
) -> AppResult<Json<ApiResponse<SessionLockView>>> {
    if body.duration > MAX_LOCK_SECONDS {
        return Err(AppError::BadRequest(
            "the session lock duration is too large".into(),
        ));
    }
    state.sessions.set_lock_after_seconds(body.duration).await;
    if body.lock_now {
        let revoked = state.sessions.revoke_others(&session.token).await;
        for token in revoked {
            crate::hid::release_session_hid(&state, &token).await;
        }
    }
    Ok(Json(ApiResponse::ok(SessionLockView {
        duration: state.sessions.lock_after_seconds().await,
    })))
}
