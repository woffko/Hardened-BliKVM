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

use std::collections::HashMap;
use std::time::{Duration, Instant};

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

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
};

/// Bounded frame size: tag plus the largest HID report (8 bytes).
pub(crate) const MAX_FRAME_BYTES: usize = 16;

/// The largest WebSocket message the socket takes: a frame with room to say
/// it is malformed; anything larger ends the socket unread.
const MAX_SOCKET_MESSAGE_BYTES: usize = 64;

/// How long a web session's HID lease outlives the last of its sockets. A
/// page that opens its socket again (another mouse mode, a mounted image, a
/// short loss of the network) keeps remote control; one that is gone gives
/// it back after this (2026-10-24: control turned off at every reconnect).
const RECONNECT_GRACE: Duration = Duration::from_secs(10);

/// The open sockets of each web session.
#[derive(Debug, Default)]
pub(crate) struct OpenSockets(std::sync::Mutex<HashMap<String, usize>>);

impl OpenSockets {
    fn opened(&self, token: &str) {
        *self.0.lock().unwrap().entry(token.to_owned()).or_default() += 1;
    }

    /// One of the session's sockets closed; the answer is how many are left.
    fn closed(&self, token: &str) -> usize {
        let mut open = self.0.lock().unwrap();
        let Some(count) = open.get_mut(token) else {
            return 0;
        };
        *count = count.saturating_sub(1);
        let left = *count;
        if left == 0 {
            open.remove(token);
        }
        left
    }

    fn count(&self, token: &str) -> usize {
        self.0.lock().unwrap().get(token).copied().unwrap_or(0)
    }
}

/// When a socket that closed gives its web session's lease back.
#[derive(Debug, PartialEq, Eq)]
enum Release {
    /// The server stops or the session ended.
    Now,
    /// Unless a socket of the session opens again meanwhile.
    AfterGrace,
    /// Another socket of the session is open and goes on with it.
    No,
}

fn release_after_close(stopping: bool, active: bool, others_open: usize) -> Release {
    if stopping || !active {
        Release::Now
    } else if others_open > 0 {
        Release::No
    } else {
        Release::AfterGrace
    }
}

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
    let Some(guard) = state.options.shutdown.register_upgrade() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    };
    // A frame is at most 16 bytes; nothing larger is buffered.
    ws.max_message_size(MAX_SOCKET_MESSAGE_BYTES)
        .max_frame_size(MAX_SOCKET_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            handle_socket(state, session, socket).await;
        })
}

async fn handle_socket(state: AppState, session: Session, mut socket: WebSocket) {
    state.ws_sockets.opened(&session.token);
    serve_socket(&state, &session, &mut socket).await;
    // The socket ends now; only the lease's release may wait.
    drop(socket);
    let others = state.ws_sockets.closed(&session.token);
    match release_after_close(
        state.options.shutdown.is_stopping(),
        session.is_active(),
        others,
    ) {
        Release::No => return,
        Release::AfterGrace => {
            tokio::select! {
                () = state.options.shutdown.cancelled() => {}
                () = tokio::time::sleep(RECONNECT_GRACE) => {}
            }
            if !state.options.shutdown.is_stopping()
                && session.is_active()
                && state.ws_sockets.count(&session.token) > 0
            {
                return;
            }
        }
        Release::Now => {}
    }
    crate::hid::release_session_hid(&state, &session.token).await;
}

async fn serve_socket(state: &AppState, session: &Session, socket: &mut WebSocket) {
    loop {
        let message = tokio::select! {
            biased;
            () = state.options.shutdown.cancelled() => break,
            message = socket.recv() => message,
            () = tokio::time::sleep(Duration::from_millis(250)) => {
                if !session.is_active() {
                    break;
                }
                continue;
            }
        };
        let Some(Ok(message)) = message else {
            break;
        };
        if state.options.shutdown.is_stopping() || !session.is_active() {
            break;
        }
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
                // The assistant has the keyboard and mouse, as in the reference.
                if state
                    .picoclaw
                    .as_deref()
                    .is_some_and(crate::picoclaw::Picoclaw::blocks_manual_input)
                {
                    let _ = socket.send(error_message("assistant_active")).await;
                    continue;
                }
                let live = state
                    .hid_ws_leases
                    .lock()
                    .await
                    .live(&session.token, Instant::now());
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

    #[test]
    fn a_socket_that_closes_gives_the_lease_back_only_when_none_opens_again() {
        // A page that opens its socket again keeps remote control; one that
        // is gone gives it back after the grace, and at once when the
        // session ends or the server stops (2026-10-24).
        assert_eq!(release_after_close(true, true, 0), Release::Now);
        assert_eq!(release_after_close(false, false, 1), Release::Now);
        assert_eq!(release_after_close(false, true, 1), Release::No);
        assert_eq!(release_after_close(false, true, 0), Release::AfterGrace);
        let open = OpenSockets::default();
        open.opened("page");
        open.opened("page");
        open.opened("other");
        assert_eq!(open.closed("page"), 1);
        assert_eq!(open.count("page"), 1);
        assert_eq!(open.closed("page"), 0);
        assert_eq!(open.count("page"), 0);
        assert_eq!(open.closed("page"), 0, "no count below zero");
        assert_eq!(open.count("other"), 1);
    }

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
            &headers(Some("https://192.0.2.10:8443"), "192.0.2.10:8443"),
            false
        ));
        assert!(origin_allowed(
            &headers(Some("https://192.0.2.10"), "192.0.2.10"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("https://evil.example"), "192.0.2.10"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("file://192.0.2.10"), "192.0.2.10"),
            false
        ));
        assert!(!origin_allowed(
            &headers(Some("https://192.0.2.10"), "192.0.2.10:8443"),
            false
        ));
        assert!(!origin_allowed(&headers(None, "192.0.2.10"), false));
        assert!(origin_allowed(&headers(None, "192.0.2.10"), true));
        assert!(!origin_allowed(
            &headers(Some("not-a-url"), "192.0.2.10"),
            false
        ));
    }
}
