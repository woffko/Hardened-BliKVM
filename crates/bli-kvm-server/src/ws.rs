//! Authenticated websocket transport for the restricted HID surface.
//!
//! The reference GUI opens `/api/ws` and sends binary frames whose first byte
//! is a message tag (0 heartbeat, 1 keyboard, 2 mouse) followed by the raw HID
//! report. This module accepts that protocol only for an authenticated session
//! whose origin matches the request host, and forwards every report through the
//! lease the same session acquired over `POST /api/hid/lease`. The websocket
//! never mints or extends a lease and never reaches the backend without one.
//!
//! Text frames are rejected with a bounded error message. Nothing is buffered:
//! a report without a live lease is dropped with an error and no backend call.

use std::time::Instant;

use axum::{
    Extension,
    extract::{
        State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bli_kvm_control_protocol::ControlResponse;
use bli_kvm_core::{HidLeaseRequest, HidReport, PlatformControlRequestV2};
use rand_core::{OsRng, RngCore};

use crate::{AppState, auth::middleware::CurrentSession};

/// Bounded frame size: tag plus the largest HID report (8 bytes).
pub(crate) const MAX_FRAME_BYTES: usize = 16;

const HEARTBEAT_TAG: u8 = 0;
const KEYBOARD_TAG: u8 = 1;
const MOUSE_TAG: u8 = 2;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum WsFrameError {
    Empty,
    TooLong,
    Unsupported,
}

/// Translate one binary GUI frame into an optional HID report.
///
/// `Ok(None)` is a heartbeat. A frame that is not exactly one of the pinned
/// shapes is rejected instead of being coerced.
pub(crate) fn parse_frame(data: &[u8]) -> Result<Option<HidReport>, WsFrameError> {
    if data.len() > MAX_FRAME_BYTES {
        return Err(WsFrameError::TooLong);
    }
    let Some((&tag, payload)) = data.split_first() else {
        return Err(WsFrameError::Empty);
    };
    match tag {
        HEARTBEAT_TAG if payload.is_empty() => Ok(None),
        KEYBOARD_TAG if payload.len() == 8 => Ok(Some(HidReport::Keyboard {
            data: payload.try_into().expect("length was checked"),
        })),
        MOUSE_TAG if payload.len() == 4 => Ok(Some(HidReport::RelativeMouse {
            data: payload.try_into().expect("length was checked"),
        })),
        MOUSE_TAG if payload.len() == 6 => Ok(Some(HidReport::AbsoluteMouse {
            data: payload.try_into().expect("length was checked"),
        })),
        _ => Err(WsFrameError::Unsupported),
    }
}

/// A websocket upgrade is accepted only from the same origin as the request.
///
/// `Host` is the authority the browser used; `Origin` must match it exactly so
/// a page from another origin cannot drive HID through the session cookie.
pub(crate) fn origin_allowed(headers: &HeaderMap, allow_missing_origin: bool) -> bool {
    let Some(origin) = headers.get("origin").and_then(|value| value.to_str().ok()) else {
        return allow_missing_origin;
    };
    let Some(host) = headers.get("host").and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Some((scheme, authority)) = origin.split_once("://") else {
        return false;
    };
    if !matches!(
        scheme.to_ascii_lowercase().as_str(),
        "http" | "https" | "ws" | "wss"
    ) {
        return false;
    }
    let authority = authority.trim_end_matches('/');
    !authority.is_empty() && authority.eq_ignore_ascii_case(host)
}

fn error_message(code: &str) -> Message {
    Message::Text(format!("{{\"type\":\"error\",\"code\":\"{code}\"}}").into())
}

fn operation_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    let mut output = String::with_capacity(32);
    for byte in bytes {
        output.push_str(&format!("{byte:02x}"));
    }
    output
}

pub(crate) async fn hid_socket(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if state.control.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "HID control channel is not configured",
        )
            .into_response();
    }
    if !origin_allowed(&headers, state.options.allow_missing_origin) {
        return (StatusCode::FORBIDDEN, "websocket origin is not allowed").into_response();
    }
    ws.on_upgrade(move |socket| handle_socket(state, session.token, socket))
}

async fn handle_socket(state: AppState, session_token: String, mut socket: WebSocket) {
    while let Some(Ok(message)) = socket.recv().await {
        match message {
            Message::Binary(data) => {
                let report = match parse_frame(&data) {
                    Ok(None) => continue,
                    Ok(Some(report)) => report,
                    Err(_) => {
                        let _ = socket.send(error_message("unsupported_frame")).await;
                        continue;
                    }
                };
                let Some(control) = state.control.as_ref().cloned() else {
                    break;
                };
                let live = state
                    .hid_ws_leases
                    .lock()
                    .await
                    .live(&session_token, Instant::now());
                let Some((lease_id, bindings)) = live else {
                    let _ = socket.send(error_message("no_active_lease")).await;
                    continue;
                };
                let request = PlatformControlRequestV2::HidLease {
                    lease: HidLeaseRequest::Report {
                        lease_id,
                        session_binding: bindings.session_binding,
                        target_binding: bindings.target_binding,
                        report,
                    },
                };
                match control.execute_v2(&operation_id(), request).await {
                    Ok(ControlResponse::Applied) => {}
                    Ok(ControlResponse::Rejected { code }) => {
                        let _ = socket
                            .send(error_message(crate::hid::rejection_name(code)))
                            .await;
                    }
                    Ok(_) => {
                        let _ = socket.send(error_message("invalid_state")).await;
                    }
                    Err(_) => {
                        let _ = socket.send(error_message("backend_unavailable")).await;
                    }
                }
            }
            Message::Text(_) => {
                let _ = socket.send(error_message("unsupported_frame")).await;
            }
            Message::Close(_) => break,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("host", host.parse().unwrap());
        if let Some(origin) = origin {
            headers.insert("origin", origin.parse().unwrap());
        }
        headers
    }

    #[test]
    fn frames_map_to_the_pinned_hid_reports() {
        assert_eq!(parse_frame(&[0]), Ok(None));
        assert_eq!(
            parse_frame(&[1, 0, 0, 4, 0, 0, 0, 0, 0]),
            Ok(Some(HidReport::Keyboard {
                data: [0, 0, 4, 0, 0, 0, 0, 0]
            }))
        );
        assert_eq!(
            parse_frame(&[2, 0, 1, 2, 3]),
            Ok(Some(HidReport::RelativeMouse { data: [0, 1, 2, 3] }))
        );
        assert_eq!(
            parse_frame(&[2, 0, 1, 0, 2, 0, 0]),
            Ok(Some(HidReport::AbsoluteMouse {
                data: [0, 1, 0, 2, 0, 0]
            }))
        );
    }

    #[test]
    fn malformed_frames_are_rejected() {
        assert_eq!(parse_frame(&[]), Err(WsFrameError::Empty));
        assert_eq!(parse_frame(&[0, 1]), Err(WsFrameError::Unsupported));
        assert_eq!(parse_frame(&[1, 0, 0]), Err(WsFrameError::Unsupported));
        assert_eq!(parse_frame(&[2, 0, 1]), Err(WsFrameError::Unsupported));
        assert_eq!(parse_frame(&[3, 0]), Err(WsFrameError::Unsupported));
        assert_eq!(
            parse_frame(&[0; MAX_FRAME_BYTES + 1]),
            Err(WsFrameError::TooLong)
        );
    }

    #[test]
    fn origin_must_match_the_request_host() {
        assert!(origin_allowed(
            &headers(Some("https://10.0.77.118:8443"), "10.0.77.118:8443"),
            false
        ));
        assert!(origin_allowed(
            &headers(Some("https://10.0.77.118"), "10.0.77.118"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("https://evil.example"), "10.0.77.118"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("file://10.0.77.118"), "10.0.77.118"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("https://10.0.77.118"), "10.0.77.118:8443"),
            false
        ));
        assert!(!origin_allowed(&headers(None, "10.0.77.118"), false));
        assert!(origin_allowed(&headers(None, "10.0.77.118"), true));
        assert!(!origin_allowed(
            &headers(Some("not-a-url"), "10.0.77.118"),
            false
        ));
    }
}
