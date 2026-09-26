//! Administration feature switches (terminal, scripts, remote ISO download,
//! PicoClaw). See `docs/design/admin-features-v1.md`.
//!
//! The board owner is the only writer of the switch file; the server reads it
//! to refuse a switched-off feature and forwards changes to the owner. The
//! helpers that do the privileged work re-check the same file.

use std::path::Path;

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};

use bli_kvm_core::{ADMIN_FEATURES_PATH, AdminFeature, AdminFeatureSwitches};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

/// The persisted switches; everything is off when the file is absent.
pub(crate) fn switches() -> AdminFeatureSwitches {
    AdminFeatureSwitches::load(Path::new(ADMIN_FEATURES_PATH))
}

/// Refuses a request for a feature that is switched off.
pub(crate) fn require(feature: AdminFeature) -> AppResult<()> {
    if switches().get(feature) {
        Ok(())
    } else {
        Err(AppError::Forbidden(format!(
            "{} is switched off in the settings",
            feature.key()
        )))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdminFeaturesView {
    terminal: bool,
    scripts: bool,
    remote_download: bool,
    picoclaw: bool,
    raw_system_update: bool,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnabledView {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EnabledBody {
    pub(crate) enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FeatureBody {
    feature: AdminFeature,
    enabled: bool,
}

pub(crate) async fn list() -> Json<ApiResponse<AdminFeaturesView>> {
    let current = switches();
    Json(ApiResponse::ok(AdminFeaturesView {
        terminal: current.terminal,
        scripts: current.scripts,
        remote_download: current.remote_download,
        picoclaw: current.picoclaw,
        raw_system_update: current.raw_system_update,
    }))
}

pub(crate) async fn update(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<FeatureBody>,
) -> AppResult<Json<ApiResponse<EnabledView>>> {
    set(&state, &session.token, body.feature, body.enabled).await
}

/// Reference: `GET /api/vm/terminal/enabled`.
pub(crate) async fn terminal_enabled() -> Json<ApiResponse<EnabledView>> {
    Json(ApiResponse::ok(EnabledView {
        enabled: switches().terminal,
    }))
}

/// Reference: `POST /api/vm/terminal/enabled`.
pub(crate) async fn set_terminal_enabled(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<EnabledBody>,
) -> AppResult<Json<ApiResponse<EnabledView>>> {
    set(&state, &session.token, AdminFeature::Terminal, body.enabled).await
}

/// Reference: `GET /api/download/image/remote/enabled`.
pub(crate) async fn remote_download_enabled() -> Json<ApiResponse<EnabledView>> {
    Json(ApiResponse::ok(EnabledView {
        enabled: switches().remote_download,
    }))
}

/// Reference: `POST /api/download/image/remote/enabled`.
pub(crate) async fn set_remote_download_enabled(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<EnabledBody>,
) -> AppResult<Json<ApiResponse<EnabledView>>> {
    set(
        &state,
        &session.token,
        AdminFeature::RemoteDownload,
        body.enabled,
    )
    .await
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn set(
    state: &AppState,
    session_token: &str,
    feature: AdminFeature,
    enabled: bool,
) -> AppResult<Json<ApiResponse<EnabledView>>> {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::PlatformControlRequestV2;

    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "administration features are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(session_token),
            PlatformControlRequestV2::AdminFeature { feature, enabled },
        )
        .await
        .map_err(|_| {
            AppError::ServiceUnavailable("administration features are unavailable".into())
        })?;
    match response {
        ControlResponse::Applied => {}
        ControlResponse::Rejected { code } => {
            return Err(AppError::Conflict(format!(
                "the switch was refused: {}",
                crate::hid::rejection_name(code)
            )));
        }
        _ => {
            return Err(AppError::ServiceUnavailable(
                "administration features are unavailable".into(),
            ));
        }
    }
    // Read back what the owner persisted.
    let enabled = switches().get(feature);
    if feature == AdminFeature::Picoclaw && !enabled {
        crate::picoclaw::switched_off(state).await;
    }
    Ok(Json(ApiResponse::ok(EnabledView { enabled })))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn set(
    _state: &AppState,
    _session_token: &str,
    _feature: AdminFeature,
    _enabled: bool,
) -> AppResult<Json<ApiResponse<EnabledView>>> {
    Err(AppError::ServiceUnavailable(
        "administration features are not configured".into(),
    ))
}

#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-admin-feature-v1");
    hasher.update(session_token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
