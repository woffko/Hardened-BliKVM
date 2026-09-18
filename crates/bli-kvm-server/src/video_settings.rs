//! Video-engine settings forwarded to the board owner.
//!
//! The interface sets the desired frame rate and the JPEG quality. The server
//! never touches the streamer itself: it validates the authenticated session,
//! forwards one bounded change to the owner, and reports the settings the owner
//! will run the engine with.

use axum::{Extension, Json, extract::State};
use serde::{Deserialize, Serialize};
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
use bli_kvm_core::{PlatformControlRequestV2, VideoSettingsChange};

#[cfg(feature = "restricted-control-surface")]
use crate::hid::{rejection_name, session_identity};

#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) const VIDEO_SETTINGS_OP_DOMAIN: &[u8] = b"hardened-blikvm-video-settings-op-v1";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct ScreenBody {
    /// The reference names the request field `type`.
    #[serde(rename = "type")]
    pub kind: String,
    pub value: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ScreenOutcome {
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
    pub fps: u16,
    pub quality: u8,
}

#[cfg(feature = "restricted-control-surface")]
fn operation_id(session_token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(VIDEO_SETTINGS_OP_DOMAIN);
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

/// Maps the reference request onto the bounded change: only the desired frame
/// rate and the JPEG quality are start-up options of the MJPEG engine, so every
/// other menu entry is answered as unsupported instead of being ignored.
#[cfg(feature = "restricted-control-surface")]
fn change_from_body(body: &ScreenBody) -> Result<VideoSettingsChange, &'static str> {
    match body.kind.as_str() {
        "fps" => {
            let value = u16::try_from(body.value).map_err(|_| "value_out_of_range")?;
            let change = VideoSettingsChange::new(Some(value), None);
            change.validate().map_err(|_| "value_out_of_range")?;
            Ok(change)
        }
        "quality" => {
            let value = u8::try_from(body.value).map_err(|_| "value_out_of_range")?;
            let change = VideoSettingsChange::new(None, Some(value));
            change.validate().map_err(|_| "value_out_of_range")?;
            Ok(change)
        }
        _ => Err("unsupported_setting"),
    }
}

#[cfg(feature = "restricted-control-surface")]
fn outcome(response: ControlResponse) -> ScreenOutcome {
    match response {
        ControlResponse::Applied => ScreenOutcome {
            applied: true,
            rejection: None,
            fps: 0,
            quality: 0,
        },
        ControlResponse::VideoState { fps, quality } => ScreenOutcome {
            applied: true,
            rejection: None,
            fps,
            quality,
        },
        ControlResponse::Rejected { code } => ScreenOutcome {
            applied: false,
            rejection: Some(rejection_name(code)),
            fps: 0,
            quality: 0,
        },
        _ => ScreenOutcome {
            applied: false,
            rejection: Some("unexpected_response"),
            fps: 0,
            quality: 0,
        },
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn update_screen(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ScreenBody>,
) -> AppResult<Json<ApiResponse<ScreenOutcome>>> {
    let change = match change_from_body(&body) {
        Ok(change) => change,
        Err(rejection) => {
            return Ok(Json(ApiResponse::ok(ScreenOutcome {
                applied: false,
                rejection: Some(rejection),
                fps: 0,
                quality: 0,
            })));
        }
    };
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "video settings are not configured".into(),
        ));
    };
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::VideoSettings { change },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("video settings are unavailable".into()))?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

/// Read back the settings the owner will run the engine with.
#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn screen_state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ScreenOutcome>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "video settings are not configured".into(),
        ));
    };
    let _ = session_identity(&session.token);
    let response = control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::VideoSettingsState {},
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("video settings are unavailable".into()))?;
    Ok(Json(ApiResponse::ok(outcome(response))))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn update_screen(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<ScreenBody>,
) -> AppResult<Json<ApiResponse<ScreenOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "video settings are not configured".into(),
    ))
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn screen_state(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<ScreenOutcome>>> {
    Err(AppError::ServiceUnavailable(
        "video settings are not configured".into(),
    ))
}

#[cfg(all(test, feature = "restricted-control-surface"))]
mod tests {
    use super::*;

    #[test]
    fn maps_the_reference_request_to_a_bounded_change() {
        let change = change_from_body(&ScreenBody {
            kind: "fps".into(),
            value: 30,
        })
        .unwrap();
        assert_eq!(change.fps, Some(30));
        assert_eq!(change.quality, None);

        let change = change_from_body(&ScreenBody {
            kind: "quality".into(),
            value: 100,
        })
        .unwrap();
        assert_eq!(change.fps, None);
        assert_eq!(change.quality, Some(100));
    }

    #[test]
    fn refuses_out_of_range_and_unsupported_settings() {
        for (kind, value, rejection) in [
            ("fps", 0, "value_out_of_range"),
            ("fps", 61, "value_out_of_range"),
            ("fps", 1000, "value_out_of_range"),
            ("quality", 0, "value_out_of_range"),
            ("quality", 101, "value_out_of_range"),
            ("quality", -1, "value_out_of_range"),
            ("resolution", 720, "unsupported_setting"),
            ("gop", 30, "unsupported_setting"),
            ("type", 1, "unsupported_setting"),
        ] {
            assert_eq!(
                change_from_body(&ScreenBody {
                    kind: kind.into(),
                    value,
                }),
                Err(rejection),
                "{kind}={value}"
            );
        }
    }

    #[test]
    fn renders_the_owner_state_and_rejection() {
        let applied = outcome(ControlResponse::VideoState {
            fps: 24,
            quality: 80,
        });
        assert!(applied.applied);
        assert_eq!((applied.fps, applied.quality), (24, 80));
        let rejected = outcome(ControlResponse::Rejected {
            code: bli_kvm_control_protocol::ControlRejectionCode::CapabilityDisabled,
        });
        assert!(!rejected.applied);
        assert_eq!(rejected.rejection, Some("capability_disabled"));
    }
}
