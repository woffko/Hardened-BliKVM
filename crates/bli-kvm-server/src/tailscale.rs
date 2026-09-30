//! Tailscale as in the reference (`/api/extensions/tailscale/*`).
//!
//! The server authenticates and forwards; the root administration helper
//! installs Tailscale from its signed apt repository, drives the daemon and
//! reads its status back in the reference's shape.

use std::time::Duration;

use axum::{Json, extract::State};
use bli_kvm_admin_helper::{AdminReply, AdminRequest, TailscaleStatus};
use serde::Serialize;

use crate::{
    AppState,
    error::{ApiResponse, AppError, AppResult},
};

/// An apt install or removal may take minutes on this board.
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const ACTION_TIMEOUT: Duration = Duration::from_secs(75);

#[derive(Debug, Serialize)]
pub(crate) struct LoginView {
    url: String,
}

async fn helper(state: &AppState, action: &str) -> AppResult<AdminReply> {
    let socket = state.options.admin_socket.as_deref().ok_or_else(|| {
        AppError::ServiceUnavailable("Tailscale control is not configured".into())
    })?;
    let timeout = if matches!(action, "install" | "uninstall") {
        PACKAGE_TIMEOUT
    } else {
        ACTION_TIMEOUT
    };
    let request = AdminRequest::Tailscale {
        action: action.to_string(),
    };
    match crate::scripts::call_with_timeout(socket, &request, None, timeout).await? {
        AdminReply::Refused { reason } => Err(AppError::BadRequest(reason)),
        reply => Ok(reply),
    }
}

pub(crate) async fn status(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<TailscaleStatus>>> {
    match helper(&state, "status").await? {
        AdminReply::Tailscale { tailscale } => Ok(Json(ApiResponse::ok(tailscale))),
        _ => Err(AppError::ServiceUnavailable("unknown status".into())),
    }
}

pub(crate) async fn login(
    State(state): State<AppState>,
) -> AppResult<Json<ApiResponse<LoginView>>> {
    match helper(&state, "login").await? {
        AdminReply::TailscaleLogin { url } => Ok(Json(ApiResponse::ok(LoginView { url }))),
        _ => Err(AppError::ServiceUnavailable("login failed".into())),
    }
}

macro_rules! tailscale_action {
    ($name:ident, $action:literal) => {
        pub(crate) async fn $name(
            State(state): State<AppState>,
        ) -> AppResult<Json<ApiResponse<()>>> {
            helper(&state, $action).await?;
            Ok(Json(ApiResponse::ok(())))
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
tailscale_action!(logout, "logout");
