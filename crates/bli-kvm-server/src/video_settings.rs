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

#[derive(Debug, Default, Serialize)]
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
    /// The WebRTC encoders' target bitrate in kbit/s, when WebRTC is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webrtc_kbps: Option<u32>,
    /// What encodes H.264: `hardware` (the board's video engine) or `x264`.
    /// Absent while the encoder has not said, which it does with its first
    /// frame for a viewer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h264_encoder: Option<&'static str>,
    /// The board's choice of the H.264 engine, `hardware` or `software`,
    /// where the choice is offered.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h264_engine: Option<&'static str>,
    /// Whether the board's video engine can encode H.264, from the
    /// encoder's report, and why not; absent while it has not reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h264_hardware_available: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub h264_hardware_reason: Option<String>,
}

/// The H.264 encoder's own word for what encodes, from the file it keeps.
/// Anything but the two words it has is no answer.
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) fn read_h264_encoder(path: &std::path::Path) -> Option<&'static str> {
    use std::io::Read;
    let mut word = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(16)
        .read_to_string(&mut word)
        .ok()?;
    match word.trim_end_matches('\n') {
        "hardware" => Some("hardware"),
        "x264" => Some("x264"),
        _ => None,
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) fn operation_id(session_token: &str) -> String {
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
            applied: true,
            ..ScreenOutcome::default()
        },
        ControlResponse::VideoState {
            fps,
            quality,
            resolution,
        } => ScreenOutcome {
            requested_resolution: resolution,
            applied: true,
            fps,
            quality,
            ..ScreenOutcome::default()
        },
        ControlResponse::Rejected { code } => ScreenOutcome {
            rejection: Some(rejection_name(code)),
            ..ScreenOutcome::default()
        },
        _ => ScreenOutcome {
            rejection: Some("unexpected_response"),
            ..ScreenOutcome::default()
        },
    }
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn update_screen(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<ScreenBody>,
) -> AppResult<Json<ApiResponse<ScreenOutcome>>> {
    // The WebRTC quality and the H.264 engine belong to the server and its
    // encoders, not to the owner's capture settings.
    if body.kind == "webrtc_kbps" {
        return set_webrtc_bitrate(&state, body.value)
            .map(|outcome| Json(ApiResponse::ok(outcome)));
    }
    if body.kind == "h264_engine" {
        return set_h264_engine(&state, body.value).map(|outcome| Json(ApiResponse::ok(outcome)));
    }
    let change = match change_from_body(&body) {
        Ok(change) => change,
        Err(rejection) => {
            return Ok(Json(ApiResponse::ok(ScreenOutcome {
                rejection: Some(rejection),
                ..ScreenOutcome::default()
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

/// Stores the chosen WebRTC bitrate and sends it to the encoders of every
/// viewer; nothing restarts. Only the offered levels are accepted.
#[cfg(feature = "restricted-control-surface")]
fn set_webrtc_bitrate(state: &AppState, value: i64) -> AppResult<ScreenOutcome> {
    let Some(gateway) = state
        .webrtc
        .as_ref()
        .filter(|_| state.options.offers_webrtc_quality())
    else {
        return Ok(ScreenOutcome {
            rejection: Some("unsupported_setting"),
            ..ScreenOutcome::default()
        });
    };
    let Some(kbps) = u32::try_from(value)
        .ok()
        .filter(|kbps| crate::webrtc::BITRATE_LEVELS_KBPS.contains(kbps))
    else {
        return Ok(ScreenOutcome {
            rejection: Some("value_out_of_range"),
            ..ScreenOutcome::default()
        });
    };
    if let Err(error) = gateway.choose_bitrate(&state.options.state_dir, kbps) {
        eprintln!("webrtc: the bitrate could not be saved: {error}");
        return Err(AppError::ServiceUnavailable(
            "the WebRTC quality could not be saved".into(),
        ));
    }
    Ok(ScreenOutcome {
        applied: true,
        webrtc_kbps: Some(kbps),
        ..ScreenOutcome::default()
    })
}

/// Stores the board's choice of the H.264 engine and sends it to the H.264
/// encoder over every viewer's demand connection; the encoder switches at
/// its next frame. `1` is the video engine, `0` x264. The choice is sent
/// even where it is the one already made: the encoder then tries an engine
/// again that it could not have before.
#[cfg(feature = "restricted-control-surface")]
fn set_h264_engine(state: &AppState, value: i64) -> AppResult<ScreenOutcome> {
    use crate::webrtc::H264Engine;
    let Some(gateway) = state
        .webrtc
        .as_ref()
        .filter(|gateway| gateway.h264_demand.is_some() && state.options.offers_engine_choice())
    else {
        return Ok(ScreenOutcome {
            rejection: Some("unsupported_setting"),
            ..ScreenOutcome::default()
        });
    };
    let engine = match value {
        1 => H264Engine::Hardware,
        0 => H264Engine::Software,
        _ => {
            return Ok(ScreenOutcome {
                rejection: Some("value_out_of_range"),
                ..ScreenOutcome::default()
            });
        }
    };
    if let Err(error) = gateway.choose_engine(&state.options.state_dir, engine) {
        eprintln!("webrtc: the H.264 engine could not be saved: {error}");
        return Err(AppError::ServiceUnavailable(
            "the H.264 engine could not be saved".into(),
        ));
    }
    Ok(ScreenOutcome {
        applied: true,
        h264_engine: Some(engine.name()),
        ..ScreenOutcome::default()
    })
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
    outcome.webrtc_kbps = state
        .webrtc
        .as_ref()
        .map(|gateway| *gateway.bitrate.borrow());
    outcome.h264_encoder = state
        .options
        .h264_encoder_status
        .as_deref()
        .and_then(read_h264_encoder);
    outcome.h264_engine = state
        .webrtc
        .as_ref()
        .filter(|gateway| gateway.h264_demand.is_some() && state.options.offers_engine_choice())
        .map(|gateway| gateway.engine.borrow().name());
    if let Some(availability) = state
        .options
        .h264_encoder_report
        .as_deref()
        .and_then(crate::encoder_report::read_engine_availability)
    {
        outcome.h264_hardware_available = Some(availability.available);
        outcome.h264_hardware_reason = availability.reason;
    }
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
    fn the_h264_encoder_is_named_by_its_own_word_and_no_other() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("encoder");
        assert_eq!(read_h264_encoder(&path), None);
        for (written, read) in [
            ("hardware\n", Some("hardware")),
            ("x264\n", Some("x264")),
            ("x264", Some("x264")),
            ("", None),
            ("vp8\n", None),
            ("hardware and more\n", None),
            ("hardware\nx264\n", None),
        ] {
            std::fs::write(&path, written).unwrap();
            assert_eq!(read_h264_encoder(&path), read, "{written:?}");
        }
        // A file that is not the encoder's small word is not read to its end.
        std::fs::write(&path, vec![b'x'; 1 << 20]).unwrap();
        assert_eq!(read_h264_encoder(&path), None);
    }

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

/// The reference's frame detect switch (`POST /api/stream/mjpeg/detect`).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(feature = "restricted-control-surface"), allow(dead_code))]
pub(crate) struct FrameDetectBody {
    #[serde(default)]
    pub enabled: bool,
}

/// The reference pauses frame detect around a resolution change.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrameDetectStopBody {
    #[serde(default)]
    #[allow(dead_code)]
    pub duration: i64,
}

#[cfg(feature = "restricted-control-surface")]
pub(crate) async fn update_frame_detect(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<FrameDetectBody>,
) -> AppResult<Json<ApiResponse<()>>> {
    let Some(control) = state.control.as_ref() else {
        return Err(AppError::ServiceUnavailable(
            "video settings are not configured".into(),
        ));
    };
    let change = VideoSettingsChange::new(None, None).with_frame_detect(body.enabled);
    match control
        .execute_v2(
            operation_id(&session.token),
            PlatformControlRequestV2::VideoSettings { change },
        )
        .await
        .map_err(|_| AppError::ServiceUnavailable("video settings are unavailable".into()))?
    {
        ControlResponse::Applied | ControlResponse::VideoState { .. } => {
            Ok(Json(ApiResponse::ok(())))
        }
        ControlResponse::Rejected { code } => Err(AppError::BadRequest(format!(
            "frame detect was refused: {}",
            rejection_name(code)
        ))),
        _ => Err(AppError::ServiceUnavailable(
            "video settings are unavailable".into(),
        )),
    }
}

#[cfg(not(feature = "restricted-control-surface"))]
pub(crate) async fn update_frame_detect(
    State(_state): State<AppState>,
    Extension(CurrentSession(_session)): Extension<CurrentSession>,
    Json(_body): Json<FrameDetectBody>,
) -> AppResult<Json<ApiResponse<()>>> {
    Err(AppError::ServiceUnavailable(
        "video settings are not configured".into(),
    ))
}

/// The streamer only drops frames identical to the previous one, so a new
/// resolution is sent at once; pausing the detection is not needed and would
/// cost two streamer restarts. Answered as done, as the reference does.
pub(crate) async fn stop_frame_detect(
    Json(_body): Json<FrameDetectStopBody>,
) -> Json<ApiResponse<()>> {
    Json(ApiResponse::ok(()))
}
