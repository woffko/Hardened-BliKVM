//! Chat relay: the browser's `/api/picoclaw/gateway/ws` to the runtime's
//! local gateway, one assistant session at a time.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use axum::{
    Extension,
    extract::{
        Query, State,
        ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bli_kvm_admin_helper::PICOCLAW_GATEWAY_PORT;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, watch};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, client::IntoClientRequest, http::HeaderValue},
};

use super::{Active, LOCK_HELD, Picoclaw, PicoclawError, RUNTIME_UNAVAILABLE, enabled};
use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PING_INTERVAL: Duration = Duration::from_secs(30);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const CLOSE_RUNTIME_UNAVAILABLE: u16 = 4002;
const CLOSE_TAKEN_OVER: u16 = 4004;
const CLOSE_UPSTREAM_CLOSED: u16 = 4005;

type Upstream = WebSocketStream<TcpStream>;

#[derive(Debug, Deserialize)]
pub(crate) struct GatewayQuery {
    #[serde(default)]
    session_id: String,
}

fn valid_session_id(id: &str) -> bool {
    bli_kvm_admin_helper::valid_picoclaw_session_id(id)
}

async fn connect_upstream(session_id: &str, token: &str) -> Result<Upstream, &'static str> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, PICOCLAW_GATEWAY_PORT));
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(address))
        .await
        .map_err(|_| "gateway is unavailable")?
        .map_err(|_| "gateway is unavailable")?;
    let mut request = format!("ws://{address}/pico/ws?session_id={session_id}")
        .into_client_request()
        .map_err(|_| "gateway is unavailable")?;
    if !token.is_empty() {
        request.headers_mut().insert(
            "Authorization",
            HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "gateway config is invalid")?,
        );
    }
    let config = tungstenite::protocol::WebSocketConfig::default()
        .max_message_size(Some(MAX_MESSAGE_BYTES))
        .max_frame_size(Some(MAX_MESSAGE_BYTES));
    let (socket, _) = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio_tungstenite::client_async_with_config(request, stream, Some(config)),
    )
    .await
    .map_err(|_| "gateway is unavailable")?
    .map_err(|_| "gateway is unavailable")?;
    Ok(socket)
}

/// `GET /api/picoclaw/gateway/ws?session_id=...`
pub(crate) async fn connect_gateway(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Query(query): Query<GatewayQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let picoclaw = match enabled(&state) {
        Ok(picoclaw) => picoclaw,
        Err(response) => return *response,
    };
    if !crate::ws::origin_allowed(&headers, state.options.allow_missing_origin) {
        return (StatusCode::FORBIDDEN, "websocket origin is not allowed").into_response();
    }
    let session_id = query.session_id.trim().to_string();
    if !valid_session_id(&session_id) {
        return PicoclawError::new(super::SESSION_ID_MISSING, "invalid PicoClaw session")
            .into_response();
    }
    if picoclaw
        .active
        .lock()
        .await
        .as_ref()
        .is_some_and(|active| active.session_id == session_id)
    {
        return lock_held(session_id, "session is already connected");
    }
    let connect = async {
        let facts = super::facts(&state)
            .await
            .map_err(IntoResponse::into_response)?;
        connect_upstream(&session_id, &facts.gateway_token)
            .await
            .map_err(|reason| {
                super::set_error(picoclaw, reason);
                PicoclawError::new(RUNTIME_UNAVAILABLE, reason).into_response()
            })
    };
    let upstream = match open_session(picoclaw, &session_id, connect).await {
        Ok(upstream) => upstream,
        Err(response) => return response,
    };
    let Some(guard) = state.options.shutdown.register_upgrade() else {
        picoclaw.release(&session_id);
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    };
    let release = super::ReleaseOnDrop {
        picoclaw: state.picoclaw.clone().expect("checked by enabled()"),
        session_id: session_id.clone(),
    };
    upgrade
        .max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| async move {
            let _guard = guard;
            let _release = release;
            relay(state, socket, upstream, session_id, session).await;
        })
        .into_response()
}

/// Reaches the runtime with `connect`, then takes the assistant's lock for
/// `session_id`: the lock blocks manual input, so it waits for a runtime that
/// answered (up to two connect timeouts otherwise). Another session that got
/// the lock meanwhile wins, and this upstream is dropped.
async fn open_session<T>(
    picoclaw: &Picoclaw,
    session_id: &str,
    connect: impl std::future::Future<Output = Result<T, Response>>,
) -> Result<T, Response> {
    let upstream = connect.await?;
    if let Err(owner) = picoclaw.acquire(session_id) {
        return Err(lock_held(owner, "another PicoClaw session is running"));
    }
    Ok(upstream)
}

fn lock_held(session_id: String, message: &str) -> Response {
    let mut response = PicoclawError::new(LOCK_HELD, message)
        .with_session(session_id)
        .into_response();
    *response.status_mut() = StatusCode::CONFLICT;
    response
}

fn close_message(code: u16, reason: &str) -> Message {
    Message::Close(Some(CloseFrame {
        code,
        reason: reason.chars().take(100).collect::<String>().into(),
    }))
}

async fn relay(
    state: AppState,
    browser: WebSocket,
    upstream: Upstream,
    session_id: String,
    operator: Session,
) {
    let Some(picoclaw) = state.picoclaw.clone() else {
        return;
    };
    let (mut browser_tx, mut browser_rx) = browser.split();
    let (mut upstream_tx, mut upstream_rx) = upstream.split();
    let (to_upstream, mut for_upstream) = mpsc::channel::<String>(32);
    let (to_browser, mut for_browser) = mpsc::channel::<String>(32);
    let (close_tx, mut close_rx) = watch::channel::<Option<(u16, String)>>(None);
    *picoclaw.active.lock().await = Some(Active {
        session_id: session_id.clone(),
        operator: operator.clone(),
        upstream: to_upstream.clone(),
        downstream: to_browser.clone(),
        close: close_tx,
    });
    eprintln!("picoclaw: assistant session opened");
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.tick().await;
    let mut watchdog = tokio::time::interval(Duration::from_secs(1));
    let (code, reason) = loop {
        tokio::select! {
            () = state.options.shutdown.cancelled() => break (1001, "server is stopping".to_string()),
            _ = close_rx.changed() => {
                let requested = close_rx.borrow().clone();
                if let Some((code, reason)) = requested {
                    break (code, reason);
                }
            }
            _ = watchdog.tick() => {
                if !operator.is_active() {
                    break (1008, "session expired".to_string());
                }
                if !crate::admin_features::switches().picoclaw {
                    break (CLOSE_RUNTIME_UNAVAILABLE, "picoclaw is switched off".to_string());
                }
                if picoclaw.owner().as_deref() != Some(session_id.as_str()) {
                    break (CLOSE_TAKEN_OVER, "session lock lost".to_string());
                }
            }
            _ = ping.tick() => {
                if browser_tx.send(Message::Ping(Vec::new().into())).await.is_err()
                    || upstream_tx.send(tungstenite::Message::Ping(Vec::new().into())).await.is_err()
                {
                    break (1000, "relay closed".to_string());
                }
            }
            Some(text) = for_upstream.recv() => {
                if upstream_tx.send(tungstenite::Message::Text(text.into())).await.is_err() {
                    break (CLOSE_UPSTREAM_CLOSED, "gateway closed".to_string());
                }
            }
            Some(text) = for_browser.recv() => {
                if browser_tx.send(Message::Text(text.into())).await.is_err() {
                    break (1000, "browser closed".to_string());
                }
            }
            message = browser_rx.next() => match message {
                Some(Ok(Message::Text(text))) => {
                    if !picoclaw.renew(&session_id) {
                        break (CLOSE_TAKEN_OVER, "session lock lost".to_string());
                    }
                    if upstream_tx
                        .send(tungstenite::Message::Text(text.as_str().into()))
                        .await
                        .is_err()
                    {
                        break (CLOSE_UPSTREAM_CLOSED, "gateway closed".to_string());
                    }
                }
                Some(Ok(Message::Binary(_))) => break (1003, "unsupported binary message".to_string()),
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break (1000, "closed".to_string()),
                Some(Ok(_)) => {}
            },
            message = upstream_rx.next() => match message {
                Some(Ok(tungstenite::Message::Text(text))) => {
                    if !picoclaw.renew(&session_id) {
                        break (CLOSE_TAKEN_OVER, "session lock lost".to_string());
                    }
                    if browser_tx.send(Message::Text(text.as_str().into())).await.is_err() {
                        break (1000, "browser closed".to_string());
                    }
                }
                Some(Ok(tungstenite::Message::Binary(_))) => {
                    break (CLOSE_UPSTREAM_CLOSED, "upstream sent unsupported binary message".to_string());
                }
                Some(Ok(tungstenite::Message::Close(_))) | None | Some(Err(_)) => {
                    break (CLOSE_UPSTREAM_CLOSED, "gateway closed".to_string());
                }
                Some(Ok(_)) => {}
            },
        }
    };
    let _ = upstream_tx
        .send(tungstenite::Message::Close(Some(
            tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Normal,
                reason: "relay closing".into(),
            },
        )))
        .await;
    let _ = browser_tx.send(close_message(code, &reason)).await;
    {
        let mut active = picoclaw.active.lock().await;
        if active
            .as_ref()
            .is_some_and(|active| active.session_id == session_id)
        {
            *active = None;
        }
    }
    picoclaw.release(&session_id);
    super::kvm::release_all(&state, &operator).await;
    if code == CLOSE_UPSTREAM_CLOSED {
        super::set_error(&picoclaw, &reason);
    }
    eprintln!("picoclaw: assistant session closed ({code})");
    drop(to_upstream);
    drop(to_browser);
}

/// Asks the relay of `session_id` to close.
pub(crate) async fn close_session(
    _state: &AppState,
    picoclaw: &Picoclaw,
    session_id: &str,
    code: u16,
    reason: &str,
) {
    if let Some(active) = picoclaw.active.lock().await.as_ref()
        && active.session_id == session_id
    {
        let _ = active.close.send(Some((code, reason.to_string())));
    }
}

/// Asks whichever relay is running to close.
pub(crate) async fn close_active(_state: &AppState, picoclaw: &Picoclaw, code: u16, reason: &str) {
    if let Some(active) = picoclaw.active.lock().await.as_ref() {
        let _ = active.close.send(Some((code, reason.to_string())));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn manual_input_is_not_blocked_while_the_runtime_is_reached() {
        let dir = tempfile::tempdir().unwrap();
        let picoclaw = Picoclaw::new(dir.path(), "http://127.0.0.1:18080/x".into());
        let meanwhile = std::cell::Cell::new(None);
        let opened = open_session(&picoclaw, "a", async {
            meanwhile.set(Some(picoclaw.blocks_manual_input()));
            Err::<(), _>(StatusCode::SERVICE_UNAVAILABLE.into_response())
        })
        .await;
        assert!(opened.is_err());
        assert_eq!(meanwhile.get(), Some(false), "manual input was blocked");
        assert!(!picoclaw.blocks_manual_input());
    }

    #[tokio::test]
    async fn a_reached_session_takes_the_lock_and_holds_off_another() {
        let dir = tempfile::tempdir().unwrap();
        let picoclaw = Picoclaw::new(dir.path(), "http://127.0.0.1:18080/x".into());
        assert!(open_session(&picoclaw, "a", async { Ok(()) }).await.is_ok());
        assert!(picoclaw.blocks_manual_input());
        let other = open_session(&picoclaw, "b", async { Ok(()) }).await;
        assert_eq!(other.unwrap_err().status(), StatusCode::CONFLICT);
        assert_eq!(picoclaw.owner().as_deref(), Some("a"));
    }
}
