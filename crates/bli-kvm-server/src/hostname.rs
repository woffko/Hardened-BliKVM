//! The reference's hostname setting (`/api/vm/hostname`), applied by the
//! board owner.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Serialize)]
pub(crate) struct HostnameView {
    hostname: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct HostnameBody {
    hostname: String,
}

pub(crate) async fn get_hostname() -> AppResult<Json<ApiResponse<HostnameView>>> {
    Ok(Json(ApiResponse::ok(HostnameView {
        hostname: crate::system_hostname()?,
    })))
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn set_hostname(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<HostnameBody>,
) -> AppResult<Json<ApiResponse<HostnameView>>> {
    use bli_kvm_control_protocol::ControlResponse;
    let name = body.hostname.trim().to_string();
    if !bli_kvm_core::valid_hostname(&name) {
        return Err(AppError::BadRequest("invalid arguments".into()));
    }
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "the hostname setting is not configured".into(),
        ));
    };
    match control
        .execute_v2(
            crate::network_write::operation_id(&session.token),
            bli_kvm_core::PlatformControlRequestV2::Hostname { name },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("the hostname setting is unavailable".into()))?
    {
        ControlResponse::Applied => Ok(Json(ApiResponse::ok(HostnameView {
            hostname: crate::system_hostname()?,
        }))),
        ControlResponse::Rejected { code } => Err(AppError::BadRequest(format!(
            "the hostname change was refused: {}",
            crate::hid::rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "the hostname setting is unavailable".into(),
        )),
    }
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn set_hostname(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<HostnameBody>,
) -> AppResult<Json<ApiResponse<HostnameView>>> {
    Err(AppError::ServiceUnavailable(
        "the hostname setting is not configured".into(),
    ))
}
