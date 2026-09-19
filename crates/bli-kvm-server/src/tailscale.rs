//! Tailscale surface forwarded to the board owner.
//!
//! The board ships no package, so the owner reports what it can observe and
//! refuses every action; the server only authenticates, forwards and reports.

use axum::{Extension, Json, extract::State};
use serde::Serialize;
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::PlatformControlRequestV2;

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity};

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) const TAILSCALE_OP_DOMAIN: &[u8] = b"hardened-blikvm-tailscale-op-v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TailscaleView {
    pub installed: bool,
    pub running: bool,
    pub enabled: bool,
    pub login_name: String,
    pub device_ip: String,
    pub device_name: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TailscaleOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(TAILSCALE_OP_DOMAIN);
    hasher.update(session_token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let digest = hasher.finalize();
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(feature = "restricted-control-surface")]
async fn request(
    state: &AppState,
    session: &crate::auth::session::Session,
    payload: PlatformControlRequestV2,
) -> AppResult<ControlResponse> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "Tailscale control is not configured".into(),
        ));
    };
    let _ = session_identity(&session.token);
    control
        .execute_v2(operation_id(&session.token), payload)
        .await
        .map_err(|_| AppError::ServiceUnavailable("Tailscale control is unavailable".into()))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn status(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TailscaleView>>> {
    let response = request(
        &state,
        &session,
        PlatformControlRequestV2::TailscaleStatus {},
    )
    .await?;
    match response {
        ControlResponse::TailscaleState {
            installed,
            running,
            enabled,
            login_name,
            device_ip,
            device_name,
            message,
        } => Ok(Json(ApiResponse::ok(TailscaleView {
            installed,
            running,
            enabled,
            login_name,
            device_ip,
            device_name,
            message,
        }))),
        _ => Err(AppError::ServiceUnavailable(
            "Tailscale state is unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn action(
    state: &AppState,
    session: &crate::auth::session::Session,
    name: &str,
) -> AppResult<Json<ApiResponse<TailscaleOutcome>>> {
    let response = request(
        state,
        session,
        PlatformControlRequestV2::TailscaleAction {
            action: name.to_string(),
        },
    )
    .await?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied => TailscaleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => TailscaleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => TailscaleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn status(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TailscaleView>>> {
    Err(AppError::ServiceUnavailable(
        "Tailscale control is not configured".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
macro_rules! tailscale_action {
    ($name:ident, $action:literal) => {
        pub(crate) async fn $name(
            State(state): State<AppState>,
            Extension(CurrentSession(session)): Extension<CurrentSession>,
        ) -> AppResult<Json<ApiResponse<TailscaleOutcome>>> {
            action(&state, &session, $action).await
        }
    };
}

#[cfg(not(feature = "restricted-control-surface"))]
macro_rules! tailscale_action {
    ($name:ident, $action:literal) => {
        pub(crate) async fn $name(
            State(_state): State<AppState>,
            Extension(CurrentSession(_session)): Extension<CurrentSession>,
        ) -> AppResult<Json<ApiResponse<TailscaleOutcome>>> {
            Err(AppError::ServiceUnavailable(
                "Tailscale control is not configured".into(),
            ))
        }
    };
}

tailscale_action!(install, "install");
tailscale_action!(uninstall, "uninstall");
tailscale_action!(start, "start");
tailscale_action!(restart, "restart");
tailscale_action!(stop, "stop");
tailscale_action!(up, "up");
tailscale_action!(down, "down");
tailscale_action!(login, "login");
tailscale_action!(logout, "logout");
