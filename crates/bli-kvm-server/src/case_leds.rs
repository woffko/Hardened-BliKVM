//! The target's case LEDs as a socket, as KVMD sends them to its page (plan
//! of 2026-11-11, H6).
//!
//! One task reads the board owner every `POLL_INTERVAL` while a socket is
//! open, and none while none is; each socket is sent the LEDs when it opens
//! and at each change. boardd holds each change for 100 ms, so a change
//! stands long enough for the read to see it. Frames from the page are not
//! read: the socket only tells.

use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Extension,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use tokio::sync::watch;

use crate::{
    AppState, CaseLeds,
    auth::{middleware::CurrentSession, session::Session},
};

/// How often the owner is read while a socket is open: half boardd's hold.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The sockets open at once; one more is refused.
pub(crate) const MAX_OPEN_SOCKETS: usize = 16;

/// How often an idle socket checks its session.
const SESSION_CHECK: Duration = Duration::from_millis(250);

/// How long a send may wait for a page that does not read.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

/// A frame from the page is at most this long; a longer one ends the socket.
const MAX_FRAME_BYTES: usize = 64;

/// The shared read of the case LEDs: `None` until the first read of a run of
/// open sockets.
pub(crate) struct CaseLedsFeed {
    sender: watch::Sender<Option<CaseLeds>>,
    polling: Mutex<bool>,
    open: AtomicUsize,
}

impl Default for CaseLedsFeed {
    fn default() -> Self {
        Self {
            sender: watch::channel(None).0,
            polling: Mutex::new(false),
            open: AtomicUsize::new(0),
        }
    }
}

impl CaseLedsFeed {
    /// A receiver of the LEDs; the reads start when none ran.
    fn subscribe(self: &Arc<Self>, state: &AppState) -> watch::Receiver<Option<CaseLeds>> {
        let mut polling = self.polling.lock().unwrap_or_else(PoisonError::into_inner);
        let receiver = self.sender.subscribe();
        if !*polling {
            *polling = true;
            self.sender.send_replace(None);
            tokio::spawn(poll(state.clone(), Arc::clone(self)));
        }
        receiver
    }

    /// Takes a place among the open sockets, if one is left.
    fn admit(self: &Arc<Self>) -> Option<OpenSocket> {
        self.open
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |open| {
                (open < MAX_OPEN_SOCKETS).then_some(open + 1)
            })
            .ok()
            .map(|_| OpenSocket(Arc::clone(self)))
    }
}

/// One open socket's place, given back when it ends.
struct OpenSocket(Arc<CaseLedsFeed>);

impl Drop for OpenSocket {
    fn drop(&mut self) {
        self.0.open.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reads the owner while a socket listens, then stops.
async fn poll(state: AppState, feed: Arc<CaseLedsFeed>) {
    loop {
        {
            let mut polling = feed.polling.lock().unwrap_or_else(PoisonError::into_inner);
            if feed.sender.receiver_count() == 0 || state.options.shutdown.is_stopping() {
                *polling = false;
                feed.sender.send_replace(None);
                return;
            }
        }
        let (_, power_led, hdd_led) = crate::case_leds(&state).await;
        let read = Some(CaseLeds { power_led, hdd_led });
        feed.sender.send_if_modified(|current| {
            let changed = *current != read;
            *current = read;
            changed
        });
        tokio::select! {
            () = state.options.shutdown.cancelled() => {}
            () = tokio::time::sleep(POLL_INTERVAL) => {}
        }
    }
}

/// `GET /api/atx/leds/socket`: the case LEDs at each change.
pub(crate) async fn socket(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if state.control.is_none() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "the board owner is not configured",
        )
            .into_response();
    }
    if !crate::ws::origin_allowed(&headers, state.options.allow_missing_origin) {
        return (StatusCode::FORBIDDEN, "websocket origin is not allowed").into_response();
    }
    let Some(place) = state.case_leds_feed.admit() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "too many LED sockets").into_response();
    };
    let Some(guard) = state.options.shutdown.register_upgrade() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    };
    ws.max_message_size(MAX_FRAME_BYTES)
        .max_frame_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _place = place;
            serve(state, session, socket).await;
        })
}

async fn serve(state: AppState, session: Session, mut socket: WebSocket) {
    let mut receiver = state.case_leds_feed.subscribe(&state);
    let mut sent = None;
    loop {
        let current = *receiver.borrow_and_update();
        if let Some(leds) = current.filter(|leds| sent != Some(*leds)) {
            let Ok(text) = serde_json::to_string(&leds) else {
                break;
            };
            match tokio::time::timeout(SEND_TIMEOUT, socket.send(Message::Text(text.into()))).await
            {
                Ok(Ok(())) => sent = Some(leds),
                _ => break,
            }
        }
        tokio::select! {
            biased;
            () = state.options.shutdown.cancelled() => break,
            changed = receiver.changed() => {
                if changed.is_err() {
                    break;
                }
            }
            message = socket.recv() => match message {
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            () = tokio::time::sleep(SESSION_CHECK) => {}
        }
        if state.options.shutdown.is_stopping() || !session.is_active() {
            break;
        }
    }
    let _ = socket.send(Message::Close(None)).await;
}
