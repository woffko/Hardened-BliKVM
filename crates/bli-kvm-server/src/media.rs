//! Virtual-media control endpoint for the restricted control surface.
//!
//! The endpoint forwards a typed media command to the control broker as a v2
//! `VirtualMedia` request. The command carries an opaque image ID, never a path
//! or filename. The server derives the session binding from the authenticated
//! session and a stable domain-separated target binding, because virtual media
//! is not focus-bound like HID.
//!
//! No media command is accepted unless the restricted control surface is
//! compiled in, explicitly enabled and backed by a configured control socket.

use axum::{Extension, Json, extract::State};
use bli_kvm_core::VirtualMediaCommand;
use serde::{Deserialize, Serialize};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_control_protocol::ControlResponse;
#[cfg(feature = "restricted-control-surface")]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(feature = "restricted-control-surface")]
use bli_kvm_core::{MediaRequest, PlatformControlRequestV2};
#[cfg(feature = "restricted-control-surface")]
use sha2::{Digest, Sha256};

use crate::{
    AppState,
    auth::middleware::CurrentSession,
    error::{ApiResponse, AppError, AppResult},
};

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity, valid_operation_id};

#[cfg(feature = "restricted-control-surface")]
const MEDIA_TARGET_DOMAIN: &[u8] = b"hardened-blikvm-media-target-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct VirtualMediaBody {
    pub operation_id: String,
    pub command: VirtualMediaCommand,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VirtualMediaOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
}

#[cfg(feature = "restricted-control-surface")]
fn media_target_binding() -> [u8; 32] {
    let digest = Sha256::digest(MEDIA_TARGET_DOMAIN);
    let mut binding = [0_u8; 32];
    binding.copy_from_slice(&digest);
    binding
}

#[cfg(feature = "restricted-control-surface")]
fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) async fn virtual_media(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(request): Json<VirtualMediaBody>,
) -> AppResult<Json<ApiResponse<VirtualMediaOutcome>>> {
    #[cfg(not(feature = "restricted-control-surface"))]
    {
        let _ = (&state, &session, &request);
        Err(AppError::ServiceUnavailable(
            "virtual media control is not available".into(),
        ))
    }
    #[cfg(feature = "restricted-control-surface")]
    {
        let Some(control) = state.control.as_ref() else {
            return Err(AppError::ServiceUnavailable(
                "virtual media control is not configured".into(),
            ));
        };
        if !state.options.restricted_controls_enabled {
            return Err(AppError::ServiceUnavailable(
                "virtual media control is not enabled".into(),
            ));
        }
        if !valid_operation_id(&request.operation_id) {
            return Err(AppError::BadRequest("media operation ID is invalid".into()));
        }
        let media = MediaRequest {
            command: request.command,
            session_binding: session_identity(&session.token),
            target_binding: media_target_binding(),
            issued_at_unix_ms: unix_millis(),
        };
        let response = control
            .execute_v2(
                &request.operation_id,
                PlatformControlRequestV2::VirtualMedia { request: media },
            )
            .await
            .map_err(|_| {
                AppError::ServiceUnavailable("virtual media control is unavailable".into())
            })?;
        Ok(Json(ApiResponse::ok(match response {
            ControlResponse::Applied | ControlResponse::LeaseGranted { .. } => {
                VirtualMediaOutcome {
                    applied: true,
                    rejection: None,
                }
            }
            ControlResponse::Rejected { code } => VirtualMediaOutcome {
                applied: false,
                rejection: Some(rejection_name(code)),
            },
        })))
    }
}
