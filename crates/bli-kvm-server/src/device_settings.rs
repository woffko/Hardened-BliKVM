//! Device toggles forwarded to the board owner.
//!
//! The interface publishes the device name over mDNS. The board has no avahi;
//! the owner flips the reviewed `systemd-resolved` key and restarts that unit,
//! and the server only validates the session and reports what the owner holds.

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
pub(crate) const MDNS_OP_DOMAIN: &[u8] = b"hardened-blikvm-mdns-op-v1";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToggleOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MdnsView {
    pub enabled: bool,
}

#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(MDNS_OP_DOMAIN);
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
async fn toggle(
    state: &AppState,
    session: &crate::auth::session::Session,
    enabled: bool,
) -> AppResult<ToggleOutcome> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let _ = session_identity(&session.token);
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::Mdns { enabled },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    Ok(match response {
        ControlResponse::Applied => ToggleOutcome {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => ToggleOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        _ => ToggleOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
        },
    })
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MdnsView>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "device settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::MdnsState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("device settings are unavailable".into()))?;
    match response {
        ControlResponse::MdnsState { enabled } => Ok(Json(ApiResponse::ok(MdnsView { enabled }))),
        _ => Err(AppError::ServiceUnavailable(
            "device settings are unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_enable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Ok(Json(ApiResponse::ok(toggle(&state, &session, true).await?)))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn mdns_disable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Ok(Json(ApiResponse::ok(
        toggle(&state, &session, false).await?,
    )))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<MdnsView>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_enable(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn mdns_disable(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ToggleOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "device settings are not configured".into(),
    ))
}
