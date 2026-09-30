//! The reference's SSH switch (`/api/vm/ssh`), applied by the board owner.

use axum::{Extension, Json, extract::State};
use serde::Serialize;

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[derive(Debug, Serialize)]
pub(crate) struct SshView {
    enabled: bool,
    active: bool,
}

/// Enabling or disabling a unit reloads the service manager, which takes a
/// few seconds on this board.
#[cfg(feature = "restricted-control-surface")]
const SWITCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

#[cfg(feature = "restricted-control-surface")]
async fn request(
    state: &AppState,
    session_token: &str,
    request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    use sha2::{Digest, Sha256};
    let Some(socket) = state.options.control_socket.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "the SSH switch is not configured".into(),
        ));
    };
    let control = bli_kvm_control_client::ControlClient::with_timeout(socket, SWITCH_TIMEOUT)
        .map_err(|_| AppError::ServiceUnavailable("the SSH switch is unavailable".into()))?;
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-ssh-v1");
    hasher.update(session_token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_nanos() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let operation: String = hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    control
        .execute_v2(operation, request)
        .await
        .map_err(|_| AppError::ServiceUnavailable("the SSH switch is unavailable".into()))
}

#[cfg(feature = "restricted-control-surface")]
async fn read(state: &AppState, session_token: &str) -> AppResult<SshView> {
    use bli_kvm_control_protocol::ControlResponse;
    match request(
        state,
        session_token,
        bli_kvm_core::PlatformControlRequestV2::SshState {},
    )
    .await?
    {
        ControlResponse::SshState { enabled, active } => Ok(SshView { enabled, active }),
        _ => Err(AppError::ServiceUnavailable(
            "the SSH state is unavailable".into(),
        )),
    }
}

#[cfg(feature = "restricted-control-surface")]
async fn set(state: &AppState, session_token: &str, enabled: bool) -> AppResult<SshView> {
    use bli_kvm_control_protocol::ControlResponse;
    match request(
        state,
        session_token,
        bli_kvm_core::PlatformControlRequestV2::Ssh { enabled },
    )
    .await?
    {
        ControlResponse::Applied => read(state, session_token).await,
        ControlResponse::Rejected { code } => Err(AppError::Conflict(format!(
            "the SSH switch was refused: {}",
            crate::hid::rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "the SSH switch is unavailable".into(),
        )),
    }
}

#[cfg(not(feature = "restricted-control-surface"))]
async fn read(_state: &AppState, _session_token: &str) -> AppResult<SshView> {
    Err(AppError::ServiceUnavailable(
        "the SSH switch is not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
async fn set(_state: &AppState, _session_token: &str, _enabled: bool) -> AppResult<SshView> {
    Err(AppError::ServiceUnavailable(
        "the SSH switch is not configured".into(),
    ))
}

pub(crate) async fn state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<SshView>>> {
    Ok(Json(ApiResponse::ok(read(&state, &session.token).await?)))
}

pub(crate) async fn enable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<SshView>>> {
    Ok(Json(ApiResponse::ok(
        set(&state, &session.token, true).await?,
    )))
}

pub(crate) async fn disable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<SshView>>> {
    Ok(Json(ApiResponse::ok(
        set(&state, &session.token, false).await?,
    )))
}
