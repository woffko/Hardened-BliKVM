//! ATX action submission for the restricted power path.
//!
//! The endpoint forwards one typed action to the denial-by-default control
//! broker using only the session/target bindings remembered from a successful
//! safe-target/focus confirmation. The capability stays unavailable until the
//! broker's gates pass, so a shipped build answers `capability_disabled`.

use axum::{Extension, Json, extract::State};
use bli_kvm_control_protocol::ControlResponse;
use bli_kvm_core::{AtxConfirmation, AtxControlAction, PlatformControlRequestV2};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
    hid::{rejection_name, valid_operation_id},
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AtxRequestBody {
    pub operation_id: String,
    pub action: AtxActionWire,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AtxActionWire {
    PowerShort,
    PowerLong,
    Reset,
}

impl AtxActionWire {
    fn action(self) -> AtxControlAction {
        match self {
            Self::PowerShort => AtxControlAction::PowerShort,
            Self::PowerLong => AtxControlAction::PowerLong,
            Self::Reset => AtxControlAction::Reset,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AtxResponse {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

pub(crate) async fn application_power(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<AtxRequestBody>,
) -> AppResult<Json<ApiResponse<AtxResponse>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "ATX control channel is not configured".into(),
        ));
    };
    if !valid_operation_id(&request.operation_id) {
        return Err(AppError::BadRequest("ATX operation ID is invalid".into()));
    }
    let now = std::time::Instant::now();
    let bindings = state
        .focus
        .lock()
        .await
        .bindings(&session.token, now)
        .ok_or_else(|| AppError::BadRequest("a current focus confirmation is required".into()))?;
    let confirmed_at_unix_ms = unix_millis();
    if confirmed_at_unix_ms == 0 {
        return Err(AppError::Internal("ATX clock is invalid".into()));
    }
    let response = control
        .execute_v2(
            request.operation_id,
            PlatformControlRequestV2::Atx {
                action: request.action.action(),
                confirmation: AtxConfirmation {
                    session_binding: bindings.session_binding,
                    confirmation_binding: bindings.target_binding,
                    confirmed_at_unix_ms,
                },
            },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("ATX control channel is unavailable".into()))?;
    Ok(Json(ApiResponse::ok(match response {
        ControlResponse::Applied
        | ControlResponse::LeaseGranted { .. }
        | ControlResponse::MediaState { .. } => AtxResponse {
            applied: true,
            rejection: None,
        },
        ControlResponse::Rejected { code } => AtxResponse {
            applied: false,
            rejection: Some(rejection_name(code)),
        },
        ControlResponse::BoardStatus { .. }
        | ControlResponse::HidReconciliationState { .. }
        | ControlResponse::VideoState { .. }
        | ControlResponse::MdnsState { .. }
        | ControlResponse::MouseJigglerState { .. }
        | ControlResponse::HidOnlyState { .. }
        | ControlResponse::WifiState { .. }
        | ControlResponse::TlsState { .. }
        | ControlResponse::FirewallModeState { .. }
        | ControlResponse::FirewallState { .. }
        | ControlResponse::TailscaleState { .. } => AtxResponse {
            applied: false,
            rejection: Some("internal"),
        },
    })))
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}
