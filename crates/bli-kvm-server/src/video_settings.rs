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
    /// Configured capture mode; independent from observed frame dimensions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_resolution: Option<String>,
    pub applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<&'static str>,
    pub fps: u16,
    pub quality: u8,
    /// The rate the bridge actually delivers, read from the engine. Absent when
    /// the engine cannot be reached; never made up from the requested value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_fps: Option<f64>,
    /// The current capture frame size and queued rate, read from the same
    /// engine state so the interface can show what the stream really carries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued_fps: Option<f64>,
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
        // The interface sends the height; the reviewed capture modes are the
        // ones the bridge and the vendor configuration list.
        "resolution" => {
            let height = u16::try_from(body.value).map_err(|_| "value_out_of_range")?;
            let mode =
                bli_kvm_core::resolution_for_height(height).ok_or("unsupported_resolution")?;
            Ok(VideoSettingsChange::new(None, None).with_resolution(mode))
        }
        _ => Err("unsupported_setting"),
    }
}

#[cfg(feature = "restricted-control-surface")]
fn outcome(response: ControlResponse) -> ScreenOutcome {
    match response {
        ControlResponse::Applied => ScreenOutcome {
            requested_resolution: None,
            applied: true,
            rejection: None,
            fps: 0,
            quality: 0,
            captured_fps: None,
            width: None,
            height: None,
            queued_fps: None,
        },
        ControlResponse::VideoState {
            fps,
            quality,
            resolution,
        } => ScreenOutcome {
            requested_resolution: resolution,
            applied: true,
            rejection: None,
            fps,
            quality,
            captured_fps: None,
            width: None,
            height: None,
            queued_fps: None,
        },
        ControlResponse::Rejected { code } => ScreenOutcome {
            requested_resolution: None,
            applied: false,
            rejection: Some(rejection_name(code)),
            fps: 0,
            quality: 0,
            captured_fps: None,
            width: None,
            height: None,
            queued_fps: None,
        },
        _ => ScreenOutcome {
            requested_resolution: None,
            applied: false,
            rejection: Some("unexpected_response"),
            fps: 0,
            quality: 0,
            captured_fps: None,
            width: None,
            height: None,
            queued_fps: None,
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
                requested_resolution: None,
                applied: false,
                rejection: Some(rejection),
                fps: 0,
                quality: 0,
                captured_fps: None,
                width: None,
                height: None,
                queued_fps: None,
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
    let mut outcome = outcome(response);
    // Read the measured state only for an applied state and only when the engine
    // socket is configured; an unreachable engine simply omits the fields.
    if outcome.applied {
        if let Some(socket) = state.options.engine_socket.clone() {
            let engine =
                tokio::task::spawn_blocking(move || crate::video_engine::read_state(&socket))
                    .await
                    .ok()
                    .flatten();
            if let Some(engine) = engine {
                outcome.captured_fps = engine.captured_fps;
                outcome.width = engine.width;
                outcome.height = engine.height;
                outcome.queued_fps = engine.queued_fps;
            }
        }
    }
    Ok(Json(ApiResponse::ok(outcome)))
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
            ("resolution", 0, "unsupported_resolution"),
            ("resolution", 1234, "unsupported_resolution"),
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
            resolution: Some("1280x720".into()),
            fps: 24,
            quality: 80,
        });
        assert!(applied.applied);
        assert_eq!((applied.fps, applied.quality), (24, 80));
        assert_eq!(applied.captured_fps, None);
        assert_eq!(applied.requested_resolution.as_deref(), Some("1280x720"));
        assert_eq!((applied.width, applied.height), (None, None));
        let rejected = outcome(ControlResponse::Rejected {
            code: bli_kvm_control_protocol::ControlRejectionCode::CapabilityDisabled,
        });
        assert!(!rejected.applied);
        assert_eq!(rejected.rejection, Some("capability_disabled"));
    }
}
