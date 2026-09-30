//! Authenticated WebRTC signaling for the VP8 and H.264 streams.
//!
//! The browser speaks a closed protocol with this server; the server alone
//! talks to Janus over its loopback WebSocket, creates one session and
//! streaming handle per viewer, watches the fixed mountpoint, relays the
//! offer/answer and trickle candidates, keeps the session alive and destroys
//! it when either side leaves. No Janus request is ever forwarded verbatim.

use std::{net::SocketAddr, path::PathBuf, sync::Arc, time::Duration};

use axum::{
    extract::{
        Extension, Query, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpStream, UnixStream},
    sync::Semaphore,
};
use tokio_tungstenite::{
    WebSocketStream, client_async,
    tungstenite::{self, client::IntoClientRequest, http::HeaderValue},
};

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
    ws::origin_allowed,
};

pub(crate) const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_SDP_BYTES: usize = 32 * 1024;
const MAX_CANDIDATE_BYTES: usize = 1024;
const MAX_CANDIDATES: usize = 64;
/// Janus mountpoints (`packaging/janus/janus.plugin.streaming.jcfg`).
const VP8_MOUNTPOINT: u64 = 1;
const H264_MOUNTPOINT: u64 = 2;
const KEEPALIVE: Duration = Duration::from_secs(20);
const JANUS_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

/// Configured when the Janus gateway is provisioned; absent keeps the
/// endpoint unavailable.
#[derive(Debug)]
pub(crate) struct WebRtcGateway {
    pub janus: SocketAddr,
    pub viewers: Arc<Semaphore>,
    /// The VP8 encoder's demand socket; see `hold_demand`.
    pub demand: Option<PathBuf>,
    /// The H.264 encoder's; set, H.264 is offered.
    pub h264_demand: Option<PathBuf>,
}

/// The codec a viewer asked for (`?codec=`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StreamCodec {
    Vp8,
    H264,
}

impl StreamCodec {
    fn name(self) -> &'static str {
        match self {
            Self::Vp8 => "vp8",
            Self::H264 => "h264",
        }
    }

    fn mountpoint(self) -> u64 {
        match self {
            Self::Vp8 => VP8_MOUNTPOINT,
            Self::H264 => H264_MOUNTPOINT,
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct StreamQuery {
    codec: Option<String>,
}

impl WebRtcGateway {
    pub(crate) fn new(
        janus: SocketAddr,
        demand: Option<PathBuf>,
        h264_demand: Option<PathBuf>,
    ) -> Option<Arc<Self>> {
        janus.ip().is_loopback().then(|| {
            Arc::new(Self {
                janus,
                viewers: Arc::new(Semaphore::new(2)),
                demand,
                h264_demand,
            })
        })
    }

    /// Asks the codec's encoder to run while the returned connection is
    /// held: it otherwise decodes and encodes every changed frame for nobody.
    /// The first line names the codec. An encoder without the socket runs
    /// anyway, so a failure only means no demand is signalled.
    pub(crate) async fn hold_demand(&self, codec: StreamCodec) -> Option<UnixStream> {
        let socket = match codec {
            StreamCodec::Vp8 => self.demand.as_ref(),
            StreamCodec::H264 => self.h264_demand.as_ref(),
        }?;
        let codec = codec.name();
        let mut connection = UnixStream::connect(socket).await.ok()?;
        connection
            .write_all(format!("{codec}\n").as_bytes())
            .await
            .ok()?;
        Some(connection)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ClientMessage {
    Answer { sdp: String },
    Candidate { candidate: Option<ClientCandidate> },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ClientCandidate {
    candidate: String,
    sdp_mid: Option<String>,
    sdp_m_line_index: Option<u32>,
}

fn event(kind: &str) -> Message {
    Message::Text(json!({ "type": kind }).to_string().into())
}

fn error(code: &str) -> Message {
    Message::Text(json!({ "type": "error", "code": code }).to_string().into())
}

pub(crate) async fn webrtc_socket(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Query(query): Query<StreamQuery>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(gateway) = state.webrtc.clone() else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            "WebRTC video is not configured",
        )
            .into_response();
    };
    let codec = match query.codec.as_deref() {
        None | Some("vp8") => StreamCodec::Vp8,
        Some("h264") if gateway.h264_demand.is_some() => StreamCodec::H264,
        Some("h264") => {
            return (StatusCode::SERVICE_UNAVAILABLE, "H.264 is not configured").into_response();
        }
        Some(_) => return (StatusCode::BAD_REQUEST, "unknown codec").into_response(),
    };
    if !origin_allowed(&headers, state.options.allow_missing_origin) {
        return (StatusCode::FORBIDDEN, "websocket origin is not allowed").into_response();
    }
    let Ok(permit) = Arc::clone(&gateway.viewers)
        .try_acquire_owned()
        .map_err(|_| ())
    else {
        return (StatusCode::TOO_MANY_REQUESTS, "too many WebRTC viewers").into_response();
    };
    let Some(guard) = state.options.shutdown.register_upgrade() else {
        return (StatusCode::SERVICE_UNAVAILABLE, "server is stopping").into_response();
    };
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |mut socket| async move {
            let _guard = guard;
            let _permit = permit;
            let _demand = gateway.hold_demand(codec).await;
            if let Err(code) = relay(&state, &gateway, &session, codec, &mut socket).await {
                let _ = socket.send(error(code)).await;
            }
            let _ = socket.close().await;
        })
}

type Janus = WebSocketStream<TcpStream>;

struct JanusSession {
    socket: Janus,
    session: u64,
    handle: u64,
    next: u64,
}

impl JanusSession {
    async fn connect(address: SocketAddr) -> Result<Self, &'static str> {
        let stream = tokio::time::timeout(JANUS_REPLY_TIMEOUT, TcpStream::connect(address))
            .await
            .map_err(|_| "gateway_unavailable")?
            .map_err(|_| "gateway_unavailable")?;
        let mut request = format!("ws://{address}/")
            .into_client_request()
            .map_err(|_| "gateway_unavailable")?;
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_static("janus-protocol"),
        );
        let (socket, _) = tokio::time::timeout(JANUS_REPLY_TIMEOUT, client_async(request, stream))
            .await
            .map_err(|_| "gateway_unavailable")?
            .map_err(|_| "gateway_unavailable")?;
        let mut janus = Self {
            socket,
            session: 0,
            handle: 0,
            next: 1,
        };
        let created = janus.request(json!({ "janus": "create" })).await?;
        janus.session = created["data"]["id"].as_u64().ok_or("gateway_error")?;
        let attached = janus
            .request(json!({ "janus": "attach", "plugin": "janus.plugin.streaming" }))
            .await?;
        janus.handle = attached["data"]["id"].as_u64().ok_or("gateway_error")?;
        Ok(janus)
    }

    fn transaction(&mut self) -> String {
        self.next += 1;
        format!("bk{}", self.next)
    }

    async fn send(&mut self, mut body: Value) -> Result<String, &'static str> {
        let transaction = self.transaction();
        body["transaction"] = json!(transaction);
        if self.session != 0 {
            body["session_id"] = json!(self.session);
        }
        if self.handle != 0
            && body["janus"] != "create"
            && body["janus"] != "attach"
            && body["janus"] != "keepalive"
            && body["janus"] != "destroy"
        {
            body["handle_id"] = json!(self.handle);
        }
        self.socket
            .send(tungstenite::Message::Text(body.to_string().into()))
            .await
            .map_err(|_| "gateway_unavailable")?;
        Ok(transaction)
    }

    async fn receive(&mut self) -> Result<Value, &'static str> {
        loop {
            match self.socket.next().await {
                Some(Ok(tungstenite::Message::Text(text))) if text.len() <= MAX_MESSAGE_BYTES => {
                    return serde_json::from_str(&text).map_err(|_| "gateway_error");
                }
                Some(Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_))) => {}
                _ => return Err("gateway_unavailable"),
            }
        }
    }

    /// Send and wait for the synchronous `success` reply of that transaction.
    async fn request(&mut self, body: Value) -> Result<Value, &'static str> {
        let transaction = self.send(body).await?;
        tokio::time::timeout(JANUS_REPLY_TIMEOUT, async {
            loop {
                let reply = self.receive().await?;
                if reply["transaction"] != json!(transaction) {
                    continue;
                }
                return match reply["janus"].as_str() {
                    Some("success") => Ok(reply),
                    _ => Err("gateway_error"),
                };
            }
        })
        .await
        .map_err(|_| "gateway_timeout")?
    }
}

async fn relay(
    state: &AppState,
    gateway: &WebRtcGateway,
    session: &Session,
    codec: StreamCodec,
    socket: &mut WebSocket,
) -> Result<(), &'static str> {
    let mut janus = JanusSession::connect(gateway.janus).await?;
    let result = exchange(state, session, socket, codec.mountpoint(), &mut janus).await;
    // Always release the gateway session, even after a browser failure.
    let _ = janus.send(json!({ "janus": "destroy" })).await;
    result
}

async fn exchange(
    state: &AppState,
    session: &Session,
    socket: &mut WebSocket,
    mountpoint: u64,
    janus: &mut JanusSession,
) -> Result<(), &'static str> {
    janus
        .send(json!({ "janus": "message", "body": { "request": "watch", "id": mountpoint } }))
        .await?;
    let mut offered = false;
    let mut answered = false;
    let mut candidates = 0_usize;
    let mut keepalive = tokio::time::interval(KEEPALIVE);
    keepalive.tick().await;
    loop {
        tokio::select! {
            biased;
            () = state.options.shutdown.cancelled() => return Ok(()),
            _ = keepalive.tick() => {
                if !session.is_active() {
                    return Err("session_expired");
                }
                janus.send(json!({ "janus": "keepalive" })).await?;
            }
            message = janus.receive() => {
                let message = message?;
                match message["janus"].as_str() {
                    Some("event") if !offered => {
                        if let Some(jsep) = message.get("jsep") {
                            let sdp = jsep["sdp"].as_str().ok_or("gateway_error")?;
                            if jsep["type"] != "offer" || sdp.len() > MAX_SDP_BYTES {
                                return Err("gateway_error");
                            }
                            socket
                                .send(Message::Text(json!({ "type": "offer", "sdp": sdp }).to_string().into()))
                                .await
                                .map_err(|_| "client_gone")?;
                            offered = true;
                        } else if message["plugindata"]["data"]["error"].is_string() {
                            return Err("stream_unavailable");
                        }
                    }
                    Some("webrtcup") => {
                        socket.send(event("started")).await.map_err(|_| "client_gone")?;
                    }
                    Some("hangup") => return Err("stream_ended"),
                    _ => {}
                }
            }
            message = socket.recv() => {
                let Some(Ok(message)) = message else {
                    return Ok(());
                };
                if !session.is_active() {
                    return Err("session_expired");
                }
                let Message::Text(text) = message else {
                    continue;
                };
                let parsed: ClientMessage = serde_json::from_str(&text).map_err(|_| "invalid_message")?;
                match parsed {
                    ClientMessage::Answer { sdp } => {
                        if !offered || answered || sdp.len() > MAX_SDP_BYTES {
                            return Err("invalid_message");
                        }
                        answered = true;
                        janus
                            .send(json!({
                                "janus": "message",
                                "body": { "request": "start" },
                                "jsep": { "type": "answer", "sdp": sdp }
                            }))
                            .await?;
                    }
                    ClientMessage::Candidate { candidate } => {
                        candidates += 1;
                        if !answered || candidates > MAX_CANDIDATES {
                            return Err("invalid_message");
                        }
                        let candidate = match candidate {
                            None => json!({ "completed": true }),
                            Some(candidate) if candidate.candidate.len() <= MAX_CANDIDATE_BYTES => json!({
                                "candidate": candidate.candidate,
                                "sdpMid": candidate.sdp_mid,
                                "sdpMLineIndex": candidate.sdp_m_line_index,
                            }),
                            Some(_) => return Err("invalid_message"),
                        };
                        janus.send(json!({ "janus": "trickle", "candidate": candidate })).await?;
                    }
                }
            }
        }
    }
}
