//! Privileged reboot and shutdown endpoints.
//!
//! The server never performs the action itself: it derives the operator
//! confirmation from the authenticated session, forwards it to the board
//! owner's system-action profile and maps the answer. The capability and every
//! request stay unavailable unless the profile is explicitly armed, and a
//! request without an explicit confirmation is refused before any IPC.

use axum::{Extension, Json, extract::State};
use bli_kvm_core::SystemActionKind;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::SystemActionWireRequest;
use serde::{Deserialize, Serialize};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::PlatformControlRequestV2;
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity};

#[cfg(feature = "restricted-control-surface")]
const SYSTEM_ACTION_DOMAIN: &[u8] = b"hardened-blikvm-system-action-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct SystemActionBody {
    /// The operator confirms the action explicitly; a missing or false value
    /// is refused before the owner is asked.
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemActionOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

/// Operation id for one forwarded request: 32 lowercase hex characters that
/// are unique per session and call, derived from the session and the clock.
#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str, domain: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(domain);
    hasher.update(session_token.as_bytes());
    hasher.update(unix_millis().to_le_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "restricted-control-surface")]
fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(feature = "restricted-control-surface")]
fn confirmation_binding(session_token: &str, action: SystemActionKind) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(SYSTEM_ACTION_DOMAIN);
    hasher.update(session_token.as_bytes());
    hasher.update(match action {
        SystemActionKind::Reboot => b"reboot".as_slice(),
        SystemActionKind::Shutdown => b"shutdown".as_slice(),
    });
    let digest = hasher.finalize();
    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&digest);
    binding
}

#[cfg(feature = "restricted-control-surface")]
async fn request_action(
    state: &AppState,
    session: &crate::auth::session::Session,
    action: SystemActionKind,
    confirm: bool,
    operation_id: &str,
) -> AppResult<SystemActionOutcome> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "system actions are not configured".into(),
        ));
    };
    if !state.options.restricted_controls_enabled || !state.options.system_actions_enabled {
        return Err(AppError::ServiceUnavailable(
            "system actions are not enabled".into(),
        ));
    }
    if !confirm {
        return Err(AppError::BadRequest(
            "the system action needs an explicit confirmation".into(),
        ));
    }
    let request = SystemActionWireRequest {
        action,
        session_binding: session_identity(&session.token),
        confirmation_binding: confirmation_binding(&session.token, action),
        confirmed_at_unix_ms: unix_millis(),
    };
    let response = control
        .execute_v2(
            operation_id,
            PlatformControlRequestV2::SystemAction { request },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("system action control is unavailable".into()))?;
    Ok(match response {
        ControlResponse::Applied => SystemActionOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => SystemActionOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => SystemActionOutcome {
            applied: false,
            rejection: Some("invalid_state"),
        },
    })
}

pub(crate) async fn reboot(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SystemActionBody>,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    dispatch(state, session, SystemActionKind::Reboot, body).await
}

/// Reboots for an operation the operator already confirmed (installing or
/// rolling back a system update), through the same owner profile.
pub(crate) async fn confirmed_reboot(
    state: AppState,
    session: crate::auth::session::Session,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    dispatch(
        state,
        session,
        SystemActionKind::Reboot,
        SystemActionBody { confirm: true },
    )
    .await
}

pub(crate) async fn shutdown(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SystemActionBody>,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    dispatch(state, session, SystemActionKind::Shutdown, body).await
}

#[cfg(feature = "restricted-control-surface")]
async fn dispatch(
    state: AppState,
    session: crate::auth::session::Session,
    action: SystemActionKind,
    body: SystemActionBody,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    let operation_id = operation_id(&session.token, b"hardened-blikvm-system-action-op-v1");
    Ok(Json(ApiResponse::ok(
        request_action(&state, &session, action, body.confirm, &operation_id).await?,
    )))
}

#[cfg(not(feature = "restricted-control-surface"))]
async fn dispatch(
    state: AppState,
    session: crate::auth::session::Session,
    action: SystemActionKind,
    body: SystemActionBody,
) -> AppResult<Json<ApiResponse<SystemActionOutcome>>> {
    let _ = (&state, &session, &action, &body);
    Err(AppError::ServiceUnavailable(
        "system actions are not available".into(),
    ))
}
