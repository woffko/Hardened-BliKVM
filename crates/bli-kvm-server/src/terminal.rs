//! Web terminal (reference `/api/vm/terminal`), switched off by default.
//!
//! The operator re-enters the password to get a one-time ticket bound to the
//! session; the WebSocket then relays keystrokes and window sizes to the root
//! administration helper, which re-checks the switch on its own. The socket
//! closes when the web session expires.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use axum::{
    Extension, Json,
    extract::{
        ConnectInfo, Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use bli_kvm_admin_helper::{
    AdminReply, AdminRequest, FRAME_DATA, FRAME_RESIZE, MAX_FRAME_BYTES, encode_frame,
    encode_resize,
};
use bli_kvm_core::AdminFeature;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
    error::{ApiResponse, AppError, AppResult},
};

const TICKET_TTL: Duration = Duration::from_secs(60);
const MAX_TICKETS: usize = 32;

#[derive(Debug, Default)]
pub(crate) struct TicketStore {
    tickets: HashMap<String, (String, Instant)>,
}

impl TicketStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn issue(&mut self, session_token: &str) -> String {
        let now = Instant::now();
        self.tickets.retain(|_, (_, expires)| *expires > now);
        if self.tickets.len() >= MAX_TICKETS {
            self.tickets.clear();
        }
        let ticket = crate::auth::token::random_token(24);
        self.tickets.insert(
            ticket.clone(),
            (session_token.to_string(), now + TICKET_TTL),
        );
        ticket
    }

    /// One use only, and only by the session that unlocked it.
    fn consume(&mut self, ticket: &str, session_token: &str) -> bool {
        match self.tickets.remove(ticket) {
            Some((owner, expires)) => owner == session_token && Instant::now() < expires,
            None => false,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UnlockBody {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UnlockView {
    ticket: String,
    expires_at: u64,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ConnectQuery {
    ticket: String,
}

#[derive(Debug, Deserialize)]
struct WindowSize {
    rows: u16,
    cols: u16,
}

pub(crate) async fn unlock(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    ConnectInfo(address): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<UnlockBody>,
) -> AppResult<Json<ApiResponse<UnlockView>>> {
    crate::admin_features::require(AdminFeature::Terminal)?;
    if state.options.admin_socket.is_none() {
        return Err(AppError::ServiceUnavailable(
            "the terminal helper is not configured".into(),
        ));
    }
    if body.username != session.username {
        return Err(AppError::InvalidCredentials);
    }
    let source = crate::auth::rate_limit::client_address(address, &headers);
    verify_unlock(
        &state.accounts,
        &state.login_limiter,
        &session.username,
        &source,
        &body.password,
    )
    .await?;
    let ticket = state.terminal_tickets.lock().await.issue(&session.token);
    let expires_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|now| (now + TICKET_TTL).as_secs())
        .unwrap_or(0);
    Ok(Json(ApiResponse::ok(UnlockView { ticket, expires_at })))
}

/// Checks the session's password for an unlock through the login limiter, as
/// a login is checked: a password longer than a login's is refused before the
/// limiter or the hash see it, and the lockout is checked again once a
/// verification place is free, so parallel guesses from one session cannot
/// outrun it (code audit of 2026-10-20, AUD-16.1).
async fn verify_unlock(
    accounts: &std::sync::Arc<crate::auth::password::AccountStore>,
    limiter: &tokio::sync::Mutex<crate::auth::rate_limit::LoginRateLimiter>,
    username: &str,
    source: &str,
    password: &str,
) -> AppResult<()> {
    if password.len() > 256 {
        return Err(AppError::BadRequest("invalid username or password".into()));
    }
    let limiter_key = format!("terminal:{username}");
    let locked = || AppError::RateLimited("terminal unlock locked after repeated failures".into());
    if limiter.lock().await.is_locked(source, &limiter_key) {
        return Err(locked());
    }
    let permit = accounts.verification_permit().await;
    if limiter.lock().await.is_locked(source, &limiter_key) {
        return Err(locked());
    }
    let verified = crate::auth::password::verify_blocking(
        accounts.clone(),
        username.to_string(),
        password.to_string(),
    )
    .await?;
    drop(permit);
    let mut limiter = limiter.lock().await;
    if !verified {
        let _ = limiter.record_failure(source, &limiter_key);
        return Err(AppError::InvalidCredentials);
    }
    limiter.record_success(source, &limiter_key);
    Ok(())
}

pub(crate) async fn connect(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Query(query): Query<ConnectQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> AppResult<Response> {
    crate::admin_features::require(AdminFeature::Terminal)?;
    if !crate::ws::origin_allowed(&headers, state.options.allow_missing_origin) {
        return Err(AppError::Forbidden("invalid websocket origin".into()));
    }
    let Some(socket) = state.options.admin_socket.clone() else {
        return Err(AppError::ServiceUnavailable(
            "the terminal helper is not configured".into(),
        ));
    };
    if !state
        .terminal_tickets
        .lock()
        .await
        .consume(&query.ticket, &session.token)
    {
        return Err(AppError::Forbidden("terminal unlock is required".into()));
    }
    Ok(upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |websocket| relay(websocket, socket, session))
        .into_response())
}

/// Why an open terminal must end now, if it must: the switch is off or the
/// session is over. Checked every second, so switching the feature off closes
/// the shells that were open, not only the ones opened later.
fn close_reason(session_active: bool, switched_on: bool) -> Option<(u16, &'static str)> {
    if !switched_on {
        Some((1008, "the terminal is switched off"))
    } else if !session_active {
        Some((1008, "terminal session expired"))
    } else {
        None
    }
}

async fn relay(websocket: WebSocket, socket: std::path::PathBuf, session: Session) {
    let (mut ws_tx, mut ws_rx) = websocket.split();
    let close = |reason: &'static str| {
        Message::Close(Some(CloseFrame {
            code: 1011,
            reason: reason.into(),
        }))
    };
    let helper = match open_terminal(&socket).await {
        Ok(helper) => helper,
        Err(reason) => {
            let _ = ws_tx.send(close(reason)).await;
            return;
        }
    };
    let (mut helper_rx, mut helper_tx) = helper.into_split();
    // The helper's frames are read by their own task: a read of a half frame
    // must not be cancelled by the watchdog's tick.
    let (frames_tx, mut frames) = tokio::sync::mpsc::channel::<Vec<u8>>(8);
    let reader = tokio::spawn(async move {
        let mut header = [0_u8; 5];
        loop {
            if helper_rx.read_exact(&mut header).await.is_err() {
                return;
            }
            let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
            if header[0] != FRAME_DATA || length > MAX_FRAME_BYTES {
                return;
            }
            let mut payload = vec![0_u8; length];
            if helper_rx.read_exact(&mut payload).await.is_err()
                || frames_tx.send(payload).await.is_err()
            {
                return;
            }
        }
    });
    // The deadline can move (a session lock change), so it is asked again
    // every second rather than slept for.
    let mut watchdog = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = watchdog.tick() => {
                let Some((code, reason)) = close_reason(
                    session.is_active(),
                    crate::admin_features::switches().get(AdminFeature::Terminal),
                ) else {
                    continue;
                };
                let _ = ws_tx.send(Message::Close(Some(CloseFrame {
                    code,
                    reason: reason.into(),
                }))).await;
                break;
            }
            frame = frames.recv() => {
                let Some(payload) = frame else { break };
                if ws_tx.send(Message::Binary(payload.into())).await.is_err() {
                    break;
                }
            }
            message = ws_rx.next() => {
                let frame = match message {
                    Some(Ok(Message::Text(text))) => encode_frame(FRAME_DATA, text.as_bytes()),
                    // The reference page sends the window size as a JSON blob.
                    Some(Ok(Message::Binary(data))) => match serde_json::from_slice::<WindowSize>(&data) {
                        Ok(size) => encode_frame(FRAME_RESIZE, &encode_resize(size.rows, size.cols)),
                        Err(_) => continue,
                    },
                    Some(Ok(Message::Ping(data))) => {
                        if ws_tx.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                        continue;
                    }
                    Some(Ok(Message::Pong(_))) => continue,
                    _ => break,
                };
                let Ok(frame) = frame else { break };
                if helper_tx.write_all(&frame).await.is_err() {
                    break;
                }
            }
        }
    }
    reader.abort();
}

async fn open_terminal(socket: &std::path::Path) -> Result<tokio::net::UnixStream, &'static str> {
    let mut stream = tokio::net::UnixStream::connect(socket)
        .await
        .map_err(|_| "terminal helper unavailable")?;
    let mut request = serde_json::to_vec(&AdminRequest::Terminal { rows: 24, cols: 80 })
        .map_err(|_| "terminal request failed")?;
    request.push(b'\n');
    stream
        .write_all(&request)
        .await
        .map_err(|_| "terminal helper unavailable")?;
    // Read the single reply line byte by byte so no frame is consumed.
    let mut line = Vec::new();
    let reply = tokio::time::timeout(Duration::from_secs(10), async {
        let mut byte = [0_u8; 1];
        loop {
            stream.read_exact(&mut byte).await?;
            if byte[0] == b'\n' {
                break;
            }
            line.push(byte[0]);
            if line.len() >= bli_kvm_admin_helper::MAX_LINE_BYTES {
                return Err(std::io::ErrorKind::InvalidData.into());
            }
        }
        Ok::<_, std::io::Error>(())
    })
    .await;
    if !matches!(reply, Ok(Ok(()))) {
        return Err("terminal helper did not answer");
    }
    match serde_json::from_slice::<AdminReply>(&line) {
        Ok(AdminReply::Ready) => Ok(stream),
        Ok(AdminReply::Refused { .. }) => Err("terminal refused by the helper"),
        Ok(_) => Err("terminal helper answer invalid"),
        Err(_) => Err("terminal helper answer invalid"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_and_limiter() -> (
        tempfile::TempDir,
        std::sync::Arc<crate::auth::password::AccountStore>,
        tokio::sync::Mutex<crate::auth::rate_limit::LoginRateLimiter>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::auth::password::AccountStore::new(dir.path().join("account.json"));
        store
            .create_initial("operator", "initial password")
            .unwrap();
        let limiter =
            tokio::sync::Mutex::new(crate::auth::rate_limit::LoginRateLimiter::new(1, 600));
        (dir, std::sync::Arc::new(store), limiter)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn parallel_unlocks_from_one_session_cannot_outrun_the_lockout() {
        let (_dir, store, limiter) = store_and_limiter();
        let limiter = std::sync::Arc::new(limiter);
        let guesses = (0..10).map(|index| {
            let store = std::sync::Arc::clone(&store);
            let limiter = std::sync::Arc::clone(&limiter);
            tokio::spawn(async move {
                verify_unlock(
                    &store,
                    &limiter,
                    "operator",
                    "192.0.2.10",
                    &format!("wrong guess {index}"),
                )
                .await
            })
        });
        let mut checked = 0;
        for guess in guesses.collect::<Vec<_>>() {
            match guess.await.unwrap() {
                Err(AppError::InvalidCredentials) => checked += 1,
                Err(AppError::RateLimited(_)) => {}
                other => panic!("unexpected {other:?}"),
            }
        }
        // Two verifications run at once; once one has failed the rest are
        // refused, not checked.
        assert!(checked <= 2, "{checked} guesses were checked");
    }

    #[tokio::test]
    async fn an_unlock_password_is_bounded_as_a_login_password() {
        let (_dir, store, limiter) = store_and_limiter();
        assert!(matches!(
            verify_unlock(&store, &limiter, "operator", "192.0.2.10", &"x".repeat(300)).await,
            Err(AppError::BadRequest(_))
        ));
        verify_unlock(
            &store,
            &limiter,
            "operator",
            "192.0.2.10",
            "initial password",
        )
        .await
        .unwrap();
    }

    #[test]
    fn tickets_are_single_use_and_bound_to_the_session() {
        let mut store = TicketStore::new();
        let ticket = store.issue("session-a");
        assert!(!store.consume(&ticket, "session-b"));
        let ticket = store.issue("session-a");
        assert!(store.consume(&ticket, "session-a"));
        assert!(!store.consume(&ticket, "session-a"));
        assert!(!store.consume("unknown", "session-a"));
    }
}

#[cfg(test)]
mod close_tests {
    use super::close_reason;

    #[test]
    fn an_open_terminal_ends_with_its_switch_or_its_session() {
        assert_eq!(close_reason(true, true), None);
        assert!(close_reason(true, false).is_some());
        assert!(close_reason(false, true).is_some());
    }
}
