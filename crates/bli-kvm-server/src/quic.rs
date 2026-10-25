//! The server's side of the QUIC transport
//! (`docs/design/quic-transport-v1.md`).
//!
//! The gateway is the QUIC and TLS front; the authority stays here. A page
//! asks for a session over HTTPS and receives the gateway's port, the hash of
//! its certificate and a token; the gateway redeems that token over its link
//! and gets a reference that stands for the page's web session. The session
//! lives as long as the web session and the link do.

use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{Extension, Json, extract::State, http::StatusCode};
use bli_kvm_quic_protocol::{
    PROTOCOL,
    link::{self, EndReason, GatewayToServer, LEAST_VIDEO_SHARE, RedeemRefusal, ServerToGateway},
};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::{mpsc, watch},
};

use crate::{
    AppState,
    auth::{middleware::CurrentSession, session::Session},
    error::{ApiResponse, AppError, AppResult},
};

/// How long a token may wait for its page to open the session.
const TOKEN_SECONDS: u32 = 30;
/// Tokens that may wait at once; a page asks for one and uses it at once.
const MAX_TOKENS: usize = 64;
/// QUIC sessions one web session may have open.
const MAX_SESSIONS_PER_WEB_SESSION: usize = 2;
/// How often the open sessions are checked against their web sessions.
const SWEEP: Duration = Duration::from_secs(1);
/// A certificate this close to its end is not handed to a page.
const CERTIFICATE_MARGIN_MS: u64 = 60_000;
/// Messages waiting to be written to the gateway.
const OUTBOX: usize = 256;
/// Lease requests of all sessions waiting for the owner. The gateway sends
/// one per session and waits for its answer, so this is never reached by a
/// gateway that keeps to the protocol.
const LEASE_QUEUE: usize = 64;
/// The whole chosen bitrate, in percent.
pub(crate) const FULL_SHARE: u8 = 100;

struct Gateway {
    /// Which connection this is: a later one replaces an earlier one.
    generation: u64,
    certificate: String,
    not_before: u64,
    not_after: u64,
    port: u16,
    outbox: mpsc::Sender<Vec<u8>>,
}

struct Token {
    session: Session,
    expires: Instant,
}

/// A QUIC session that is open.
struct Open {
    /// The web session it stands for.
    web: Session,
    /// The HID lease it acquired over its input stream and has not released.
    lease: Option<String>,
    /// While it watches: the H.264 encoder's demand, held for it.
    video: Option<crate::webrtc::DemandGuard>,
    /// The share of the chosen bitrate, in percent, that its path carries,
    /// as the gateway last said.
    share: u8,
}

/// A lease that a session which ended had taken: the web session's token and
/// the lease's id.
type HeldLease = (String, String);

#[derive(Default)]
struct Inner {
    generations: u64,
    gateway: Option<Gateway>,
    tokens: HashMap<String, Token>,
    /// By session reference.
    sessions: HashMap<String, Open>,
}

impl Inner {
    /// Forgets every session and says which leases they held.
    fn drop_sessions(&mut self) -> Vec<HeldLease> {
        self.sessions
            .drain()
            .filter_map(|(_, open)| Some((open.web.token, open.lease?)))
            .collect()
    }
}

pub(crate) struct QuicTransport {
    inner: Mutex<Inner>,
    /// The least share of the chosen bitrate, in percent, that the sessions
    /// which watch ask for (`docs/design/quic-transport-v1.md`, "The
    /// bitrate"); whole while none asks for less. The H.264 encoder runs at
    /// the chosen bitrate times it.
    share: watch::Sender<u8>,
}

impl Default for QuicTransport {
    fn default() -> Self {
        Self {
            inner: Mutex::default(),
            share: watch::Sender::new(FULL_SHARE),
        }
    }
}

/// Why a page gets no token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoToken {
    /// No gateway is on the link: the switch is off, or the gateway is not
    /// running or speaks another protocol.
    Unavailable,
    /// The certificate is not valid by the page's clock.
    Clock,
}

struct Granted {
    port: u16,
    certificate: String,
    token: String,
}

fn random_hex(bytes: usize) -> String {
    let mut raw = vec![0u8; bytes];
    OsRng.fill_bytes(&mut raw);
    raw.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl QuicTransport {
    /// The least share of the chosen bitrate that the sessions which watch
    /// ask for, as it changes.
    pub(crate) fn share(&self) -> watch::Receiver<u8> {
        self.share.subscribe()
    }

    /// Takes the least share afresh after the sessions changed.
    fn reshare(&self, inner: &Inner) {
        let least = inner
            .sessions
            .values()
            .filter(|open| open.video.is_some())
            .map(|open| open.share)
            .min()
            .unwrap_or(FULL_SHARE);
        self.share
            .send_if_modified(|share| std::mem::replace(share, least) != least);
    }

    /// The gateway's port while one is on the link.
    fn port(&self) -> Option<u16> {
        self.inner
            .lock()
            .unwrap()
            .gateway
            .as_ref()
            .map(|gateway| gateway.port)
    }

    fn grant(&self, session: &Session, page_unix_ms: u64) -> Result<Granted, NoToken> {
        let mut inner = self.inner.lock().unwrap();
        let now = Instant::now();
        inner.tokens.retain(|_, token| token.expires > now);
        let gateway = inner.gateway.as_ref().ok_or(NoToken::Unavailable)?;
        let valid_from = gateway.not_before.saturating_mul(1000);
        let valid_until = gateway
            .not_after
            .saturating_mul(1000)
            .saturating_sub(CERTIFICATE_MARGIN_MS);
        if page_unix_ms < valid_from || page_unix_ms > valid_until {
            return Err(NoToken::Clock);
        }
        let granted = Granted {
            port: gateway.port,
            certificate: gateway.certificate.clone(),
            token: random_hex(32),
        };
        if inner.tokens.len() >= MAX_TOKENS {
            // The oldest waiting token makes room.
            if let Some(oldest) = inner
                .tokens
                .iter()
                .min_by_key(|(_, token)| token.expires)
                .map(|(key, _)| key.clone())
            {
                inner.tokens.remove(&oldest);
            }
        }
        inner.tokens.insert(
            granted.token.clone(),
            Token {
                session: session.clone(),
                expires: now + Duration::from_secs(u64::from(TOKEN_SECONDS)),
            },
        );
        Ok(granted)
    }

    /// Redeems a token once: the reference of the new session, or why not.
    fn redeem(&self, token: &str) -> Result<String, RedeemRefusal> {
        let mut inner = self.inner.lock().unwrap();
        let entry = inner.tokens.remove(token).ok_or(RedeemRefusal::Token)?;
        if entry.expires <= Instant::now() || !entry.session.is_active() {
            return Err(RedeemRefusal::Token);
        }
        let open = inner
            .sessions
            .values()
            .filter(|open| open.web.token == entry.session.token)
            .count();
        if open >= MAX_SESSIONS_PER_WEB_SESSION {
            return Err(RedeemRefusal::Sessions);
        }
        let reference = random_hex(16);
        inner.sessions.insert(
            reference.clone(),
            Open {
                web: entry.session,
                lease: None,
                video: None,
                share: FULL_SHARE,
            },
        );
        Ok(reference)
    }

    /// The session's connection is gone; the answer is the lease it held.
    fn closed(&self, reference: &str) -> Option<HeldLease> {
        let mut inner = self.inner.lock().unwrap();
        let open = inner.sessions.remove(reference)?;
        self.reshare(&inner);
        Some((open.web.token, open.lease?))
    }

    /// The web session a QUIC session stands for.
    fn web_session(&self, reference: &str) -> Option<Session> {
        self.inner
            .lock()
            .unwrap()
            .sessions
            .get(reference)
            .map(|open| open.web.clone())
    }

    /// Notes the lease a session acquired, or (`None`) that it gave it back.
    /// `false` if the session is no longer there.
    fn note_lease(&self, reference: &str, lease: Option<String>) -> bool {
        match self.inner.lock().unwrap().sessions.get_mut(reference) {
            Some(open) => {
                open.lease = lease;
                true
            }
            None => false,
        }
    }

    /// Notes that a session renewed or reported through a lease: after a
    /// reconnect the page goes on with the lease it took over the earlier
    /// session, so that session must not give it back when it closes. The
    /// lease is this session's now, and no other session of the web session
    /// names it. `false` if the session is no longer there.
    fn adopt_lease(&self, reference: &str, lease: &str) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(token) = inner
            .sessions
            .get(reference)
            .map(|open| open.web.token.clone())
        else {
            return false;
        };
        for (other, open) in inner.sessions.iter_mut() {
            if other == reference {
                open.lease = Some(lease.to_string());
            } else if open.web.token == token && open.lease.as_deref() == Some(lease) {
                open.lease = None;
            }
        }
        true
    }

    /// Holds the encoder's demand for a session that watches, or (`None`)
    /// lets it go. `false` if the session is no longer there; the demand is
    /// then let go with the guard.
    fn set_video(&self, reference: &str, video: Option<crate::webrtc::DemandGuard>) -> bool {
        let mut inner = self.inner.lock().unwrap();
        let Some(open) = inner.sessions.get_mut(reference) else {
            return false;
        };
        open.video = video;
        self.reshare(&inner);
        true
    }

    /// Notes the share of the chosen bitrate that a session's path carries,
    /// within the least a session may ask for and the whole.
    fn set_share(&self, reference: &str, percent: u8) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(open) = inner.sessions.get_mut(reference) {
            open.share = percent.clamp(LEAST_VIDEO_SHARE, FULL_SHARE);
            self.reshare(&inner);
        }
    }

    /// The sessions whose web session has ended, taken out of the table,
    /// each with the lease it held.
    fn take_ended(&self) -> Vec<(String, Option<HeldLease>)> {
        let mut inner = self.inner.lock().unwrap();
        let ended: Vec<String> = inner
            .sessions
            .iter()
            .filter(|(_, open)| !open.web.is_active())
            .map(|(reference, _)| reference.clone())
            .collect();
        let taken = ended
            .into_iter()
            .filter_map(|reference| {
                let open = inner.sessions.remove(&reference)?;
                let held = open.lease.map(|lease| (open.web.token, lease));
                Some((reference, held))
            })
            .collect();
        self.reshare(&inner);
        taken
    }

    /// A gateway said hello on a new connection; an earlier one is replaced
    /// and its sessions are forgotten. The answer is the connection's
    /// generation and the leases those sessions held.
    fn connected(&self, hello: Hello, outbox: mpsc::Sender<Vec<u8>>) -> (u64, Vec<HeldLease>) {
        let mut inner = self.inner.lock().unwrap();
        inner.generations += 1;
        let generation = inner.generations;
        let held = inner.drop_sessions();
        self.reshare(&inner);
        inner.tokens.clear();
        inner.gateway = Some(Gateway {
            generation,
            certificate: hello.certificate,
            not_before: hello.not_before,
            not_after: hello.not_after,
            port: hello.port,
            outbox,
        });
        (generation, held)
    }

    /// The gateway presents a new certificate from now on.
    fn certificate_replaced(&self, generation: u64, hello: Hello) {
        let mut inner = self.inner.lock().unwrap();
        if let Some(gateway) = inner.gateway.as_mut()
            && gateway.generation == generation
        {
            gateway.certificate = hello.certificate;
            gateway.not_before = hello.not_before;
            gateway.not_after = hello.not_after;
            gateway.port = hello.port;
            // A token issued with the old hash would only fail at the page.
            inner.tokens.clear();
        }
    }

    /// The connection of this generation is gone; the answer is the leases
    /// its sessions held.
    fn disconnected(&self, generation: u64) -> Vec<HeldLease> {
        let mut inner = self.inner.lock().unwrap();
        if inner
            .gateway
            .as_ref()
            .is_some_and(|gateway| gateway.generation == generation)
        {
            inner.gateway = None;
            inner.tokens.clear();
            let held = inner.drop_sessions();
            self.reshare(&inner);
            return held;
        }
        Vec::new()
    }

    fn send(&self, generation: u64, message: &ServerToGateway) -> bool {
        let Ok(bytes) = link::encode(message) else {
            return false;
        };
        let inner = self.inner.lock().unwrap();
        match &inner.gateway {
            Some(gateway) if gateway.generation == generation => {
                gateway.outbox.try_send(bytes).is_ok()
            }
            _ => false,
        }
    }
}

struct Hello {
    certificate: String,
    not_before: u64,
    not_after: u64,
    port: u16,
}

fn valid_hello(message: GatewayToServer) -> Option<(u16, Hello)> {
    match message {
        GatewayToServer::Hello {
            protocol,
            certificate_sha256,
            not_before,
            not_after,
            port,
        } if certificate_sha256.len() == 64
            && certificate_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && not_before < not_after
            && port != 0 =>
        {
            Some((
                protocol,
                Hello {
                    certificate: certificate_sha256,
                    not_before,
                    not_after,
                    port,
                },
            ))
        }
        _ => None,
    }
}

/// Starts the link's listener if one is configured. A link that cannot be
/// set up leaves the QUIC transport unavailable and everything else as it is.
pub(crate) fn listen(state: &AppState) {
    let (Some(path), Some(transport)) =
        (state.options.quic_link_socket.clone(), state.quic.clone())
    else {
        return;
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        eprintln!("quic link: no runtime; the QUIC transport stays unavailable");
        return;
    };
    let _in_runtime = runtime.enter();
    match bind(&path, state.options.quic_link_uid) {
        Ok((listener, peer)) => {
            runtime.spawn(accept(state.clone(), transport, listener, peer));
        }
        Err(reason) => {
            eprintln!("quic link: {reason}; the QUIC transport stays unavailable");
        }
    }
}

/// The listener and the only user whose process may connect to it: the
/// configured one, or this server's own.
fn bind(path: &Path, peer: Option<u32>) -> Result<(UnixListener, u32), String> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    if !path.is_absolute() {
        return Err("the link socket path must be absolute".into());
    }
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("the old link socket could not be removed: {error}")),
    }
    let listener = std::os::unix::net::UnixListener::bind(path)
        .map_err(|error| format!("the link socket could not be bound: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| error.to_string())?;
    // The gateway is another user: the socket is open to connect to, and the
    // peer's user is checked on every connection.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666))
        .map_err(|error| format!("the link socket's mode could not be set: {error}"))?;
    let own = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .uid();
    let listener = UnixListener::from_std(listener).map_err(|error| error.to_string())?;
    Ok((listener, peer.unwrap_or(own)))
}

async fn accept(state: AppState, transport: Arc<QuicTransport>, listener: UnixListener, peer: u32) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(200)).await;
            continue;
        };
        match stream.peer_cred() {
            Ok(credentials) if credentials.uid() == peer => {
                tokio::spawn(serve(state.clone(), Arc::clone(&transport), stream));
            }
            Ok(credentials) => {
                eprintln!(
                    "quic link: a connection of user {} was refused",
                    credentials.uid()
                );
            }
            Err(_) => {}
        }
    }
}

async fn read_message(reader: &mut tokio::net::unix::OwnedReadHalf) -> Option<GatewayToServer> {
    let mut prefix = [0u8; 4];
    reader.read_exact(&mut prefix).await.ok()?;
    let length = link::body_length(prefix).ok()?;
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).await.ok()?;
    link::decode(&body).ok()
}

/// What the sessions of one gateway connection ask of the server, in the
/// order it arrived.
enum Job {
    /// A request from a session's input stream.
    Lease {
        id: u64,
        session: String,
        request: serde_json::Value,
    },
    /// A session that held the lease is gone.
    Release(HeldLease),
    /// A session asks for the picture, or gives it up.
    Video {
        id: u64,
        session: String,
        subscribe: bool,
    },
}

/// Holds the H.264 encoder's demand for a session that watches, as the
/// WebRTC signalling does for its viewer, with the quality the operator
/// chose; or lets it go. The reason there is no picture: the session is not
/// one of a live web session, this board has no H.264 encoder, or the
/// encoder could not be reached.
async fn serve_video(
    state: &AppState,
    transport: &QuicTransport,
    reference: &str,
    subscribe: bool,
) -> Result<(), &'static str> {
    let web = transport.web_session(reference).ok_or("session")?;
    state.sessions.validate(&web.token).await.ok_or("session")?;
    if !subscribe {
        transport.set_video(reference, None);
        return Ok(());
    }
    let gateway = state
        .webrtc
        .as_ref()
        .filter(|gateway| gateway.h264_demand.is_some())
        .ok_or("unavailable")?;
    let demand = gateway
        .hold_demand(crate::webrtc::StreamCodec::H264)
        .await
        .ok_or("encoder")?;
    if transport.set_video(reference, Some(demand)) {
        Ok(())
    } else {
        Err("session")
    }
}

/// An answer of the lease API as the link carries it: the status and the
/// body the HTTPS route would have given.
fn lease_answer(status: StatusCode, body: impl Serialize) -> serde_json::Value {
    serde_json::json!({
        "status": status.as_u16(),
        "body": serde_json::to_value(body).unwrap_or(serde_json::Value::Null),
    })
}

fn lease_refusal(error: AppError) -> serde_json::Value {
    let (status, body) = error.parts();
    lease_answer(status, body)
}

/// Serves one lease request of a QUIC session for the web session it stands
/// for, with the code behind `POST /api/hid/lease`.
async fn serve_lease(
    state: &AppState,
    transport: &QuicTransport,
    reference: &str,
    request: serde_json::Value,
) -> serde_json::Value {
    let Some(web) = transport.web_session(reference) else {
        return lease_refusal(AppError::Unauthorized);
    };
    // As for a request over HTTPS: it counts as the session's activity, and
    // a session that has ended is refused.
    let Some(session) = state.sessions.validate(&web.token).await else {
        return lease_refusal(AppError::Unauthorized);
    };
    let Ok(request) = serde_json::from_value::<crate::hid::HidLeaseBody>(request) else {
        return lease_answer(
            StatusCode::UNPROCESSABLE_ENTITY,
            ApiResponse::<()>::error(-422, "the request is not a lease request"),
        );
    };
    let releases = matches!(request, crate::hid::HidLeaseBody::Release { .. });
    let used = match &request {
        crate::hid::HidLeaseBody::Renew { lease_id, .. }
        | crate::hid::HidLeaseBody::Report { lease_id, .. } => Some(lease_id.clone()),
        _ => None,
    };
    match crate::hid::serve_lease(state, &session, request).await {
        Ok(response) => {
            if let Some(lease) = response.lease_id.as_ref() {
                // A session that went away meanwhile cannot give it back.
                if !transport.note_lease(reference, Some(lease.clone())) {
                    crate::hid::release_lease_if_current(state, &session.token, lease).await;
                }
            } else if releases && response.applied {
                transport.note_lease(reference, None);
            } else if let (Some(lease), true) = (used, response.applied) {
                transport.adopt_lease(reference, &lease);
            }
            lease_answer(StatusCode::OK, ApiResponse::ok(response))
        }
        Err(error) => lease_refusal(error),
    }
}

/// Works through what the sessions of one gateway connection ask, one
/// request at a time. It is a task of its own: a lease request that has
/// reached the owner is finished and accounted for even if the link ends
/// meanwhile.
async fn session_worker(
    state: AppState,
    transport: Arc<QuicTransport>,
    generation: u64,
    mut jobs: mpsc::Receiver<Job>,
) {
    while let Some(job) = jobs.recv().await {
        match job {
            Job::Lease {
                id,
                session,
                request,
            } => {
                let response = serve_lease(&state, &transport, &session, request).await;
                transport.send(generation, &ServerToGateway::LeaseAnswer { id, response });
            }
            Job::Release((token, lease)) => {
                crate::hid::release_lease_if_current(&state, &token, &lease).await;
            }
            Job::Video {
                id,
                session,
                subscribe,
            } => {
                let served = serve_video(&state, &transport, &session, subscribe).await;
                transport.send(
                    generation,
                    &ServerToGateway::VideoAnswer {
                        id,
                        ok: served.is_ok(),
                        reason: served.err().map(str::to_owned),
                    },
                );
            }
        }
    }
}

/// One connection of the gateway, from its hello to its end.
async fn serve(state: AppState, transport: Arc<QuicTransport>, stream: UnixStream) {
    let (mut reader, mut writer) = stream.into_split();
    let Some((protocol, hello)) = read_message(&mut reader).await.and_then(valid_hello) else {
        return;
    };
    let Ok(greeting) = link::encode(&ServerToGateway::Hello { protocol: PROTOCOL }) else {
        return;
    };
    if writer.write_all(&greeting).await.is_err() {
        return;
    }
    if protocol != PROTOCOL {
        eprintln!("quic link: the gateway speaks link protocol {protocol}, not {PROTOCOL}");
        return;
    }
    let (outbox, mut queued) = mpsc::channel::<Vec<u8>>(OUTBOX);
    let (generation, replaced) = transport.connected(hello, outbox);
    eprintln!("quic link: the gateway is connected");
    let (jobs, waiting) = mpsc::channel::<Job>(LEASE_QUEUE);
    tokio::spawn(session_worker(
        state.clone(),
        Arc::clone(&transport),
        generation,
        waiting,
    ));
    // A session that ends gives its lease back behind the requests it made.
    let release = |held: HeldLease| {
        if let Err(mpsc::error::TrySendError::Full(Job::Release((token, lease)))) =
            jobs.try_send(Job::Release(held))
        {
            let state = state.clone();
            tokio::spawn(async move {
                crate::hid::release_lease_if_current(&state, &token, &lease).await;
            });
        }
    };
    replaced.into_iter().for_each(&release);

    let write = async {
        while let Some(bytes) = queued.recv().await {
            if writer.write_all(&bytes).await.is_err() {
                break;
            }
        }
    };
    let read = async {
        while let Some(message) = read_message(&mut reader).await {
            match message {
                GatewayToServer::Hello { .. } => match valid_hello(message) {
                    Some((_, hello)) => transport.certificate_replaced(generation, hello),
                    None => break,
                },
                GatewayToServer::Redeem { id, token, peer } => {
                    let answer = match transport.redeem(&token) {
                        Ok(session) => {
                            eprintln!("quic link: a session was opened for {peer}");
                            ServerToGateway::Redeemed {
                                id,
                                session: Some(session),
                                refused: None,
                            }
                        }
                        Err(refused) => ServerToGateway::Redeemed {
                            id,
                            session: None,
                            refused: Some(refused),
                        },
                    };
                    transport.send(generation, &answer);
                }
                GatewayToServer::Closed { session } => {
                    if let Some(held) = transport.closed(&session) {
                        release(held);
                    }
                }
                GatewayToServer::VideoShare { session, percent } => {
                    transport.set_share(&session, percent);
                }
                GatewayToServer::Lease {
                    id,
                    session,
                    request,
                } => {
                    let job = Job::Lease {
                        id,
                        session,
                        request,
                    };
                    if jobs.try_send(job).is_err() {
                        transport.send(
                            generation,
                            &ServerToGateway::LeaseAnswer {
                                id,
                                response: lease_refusal(AppError::ServiceUnavailable(
                                    "too many lease requests are waiting".into(),
                                )),
                            },
                        );
                    }
                }
                GatewayToServer::Video {
                    id,
                    session,
                    subscribe,
                } => {
                    let job = Job::Video {
                        id,
                        session,
                        subscribe,
                    };
                    if jobs.try_send(job).is_err() {
                        transport.send(
                            generation,
                            &ServerToGateway::VideoAnswer {
                                id,
                                ok: false,
                                reason: Some("busy".into()),
                            },
                        );
                    }
                }
            }
        }
    };
    // A web session that ends takes its QUIC sessions with it.
    let sweep = async {
        let mut tick = tokio::time::interval(SWEEP);
        loop {
            tick.tick().await;
            for (session, held) in transport.take_ended() {
                // Nothing else gives its lease back: the gateway's word on
                // the session finds it gone.
                if let Some(held) = held {
                    release(held);
                }
                transport.send(
                    generation,
                    &ServerToGateway::End {
                        session,
                        reason: EndReason::Session,
                    },
                );
            }
        }
    };
    tokio::select! {
        () = write => {}
        () = read => {}
        () = sweep => {}
    }
    transport
        .disconnected(generation)
        .into_iter()
        .for_each(&release);
    eprintln!("quic link: the gateway is disconnected");
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct QuicView {
    /// This installation has the link to a gateway configured.
    supported: bool,
    /// The board's switch: the gateway's unit is enabled, and running.
    enabled: bool,
    active: bool,
    /// The gateway is on the link: a session can be asked for.
    connected: bool,
    /// The firewall mode keeps the gateway's port closed.
    blocked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
}

#[derive(Debug, Serialize)]
pub(crate) struct TransportView {
    quic: QuicView,
}

#[cfg(feature = "restricted-control-surface")]
const SWITCH_TIMEOUT: Duration = Duration::from_secs(20);

/// Asks the owner about, or for, the gateway's unit.
#[cfg(feature = "restricted-control-surface")]
async fn owner(
    state: &AppState,
    session_token: &str,
    request: bli_kvm_core::PlatformControlRequestV2,
) -> AppResult<bli_kvm_control_protocol::ControlResponse> {
    use sha2::{Digest, Sha256};
    let unavailable = || AppError::ServiceUnavailable("the QUIC switch is unavailable".into());
    let socket = state
        .options
        .control_socket
        .as_ref()
        .ok_or_else(unavailable)?;
    let control = bli_kvm_control_client::ControlClient::with_timeout(socket, SWITCH_TIMEOUT)
        .map_err(|_| unavailable())?;
    let mut hasher = Sha256::new();
    hasher.update(b"hardened-blikvm-quic-v1");
    hasher.update(session_token.as_bytes());
    hasher.update(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|now| now.as_nanos() as u64)
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let operation: String = hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    control
        .execute_v2(operation, request)
        .await
        .map_err(|_| unavailable())
}

/// The switch as the owner reads it from systemd: enabled, and running.
async fn switch(state: &AppState, session_token: &str) -> (bool, bool) {
    #[cfg(feature = "restricted-control-surface")]
    if state.options.restricted_controls_enabled
        && let Ok(bli_kvm_control_protocol::ControlResponse::QuicState { enabled, active }) = owner(
            state,
            session_token,
            bli_kvm_core::PlatformControlRequestV2::QuicState {},
        )
        .await
    {
        return (enabled, active);
    }
    let _ = (state, session_token);
    (false, false)
}

async fn view(state: &AppState, session_token: &str) -> TransportView {
    let port = state.quic.as_ref().and_then(|transport| transport.port());
    let (enabled, active) = match state.quic {
        Some(_) => switch(state, session_token).await,
        None => (false, false),
    };
    TransportView {
        quic: QuicView {
            supported: state.quic.is_some(),
            enabled,
            active,
            connected: port.is_some(),
            blocked: crate::firewall_modes::quic_blocked_now(state).await,
            port,
        },
    }
}

/// `GET /api/transport`: what the QUIC transport can do right now.
pub(crate) async fn state(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> Json<ApiResponse<TransportView>> {
    Json(ApiResponse::ok(view(&state, &session.token).await))
}

async fn set(state: &AppState, session_token: &str, enabled: bool) -> AppResult<TransportView> {
    if state.quic.is_none() {
        return Err(AppError::ServiceUnavailable(
            "the QUIC transport is not installed".into(),
        ));
    }
    #[cfg(feature = "restricted-control-surface")]
    if state.options.restricted_controls_enabled {
        return match owner(
            state,
            session_token,
            bli_kvm_core::PlatformControlRequestV2::Quic { enabled },
        )
        .await?
        {
            bli_kvm_control_protocol::ControlResponse::Applied => {
                Ok(view(state, session_token).await)
            }
            _ => Err(AppError::ServiceUnavailable(
                "the QUIC gateway could not be switched".into(),
            )),
        };
    }
    let _ = (session_token, enabled);
    Err(AppError::ServiceUnavailable(
        "the QUIC switch is unavailable".into(),
    ))
}

/// `POST /api/transport/quic/enable`.
pub(crate) async fn enable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TransportView>>> {
    Ok(Json(ApiResponse::ok(
        set(&state, &session.token, true).await?,
    )))
}

/// `POST /api/transport/quic/disable`.
pub(crate) async fn disable(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
) -> AppResult<Json<ApiResponse<TransportView>>> {
    Ok(Json(ApiResponse::ok(
        set(&state, &session.token, false).await?,
    )))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SessionBody {
    /// The page's clock, milliseconds since 1970.
    page_time: u64,
    /// How the page's session before this one ended, from its side.
    #[serde(default)]
    previous: Option<PreviousSession>,
}

/// How a page's last session ended, as the page says it: the gateway sees
/// only that the page closed its connection, or went silent. It is written in
/// the journal and nothing else; a later page may name what this one does not
/// know, or say more, and is not refused for it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreviousSession {
    ending: PageEnding,
    open_seconds: u64,
    since_pong_ms: u64,
    hidden: bool,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PageEnding {
    /// Its pings went unanswered.
    NoPong,
    /// The control stream ended.
    StreamEnded,
    ReadFailed,
    WriteFailed,
    /// The browser closed the connection, or found it closed.
    TransportClosed,
    /// The gateway said it closes the session.
    GatewayClosing,
    #[serde(other)]
    Other,
}

/// The longest part of what the browser said that the journal keeps.
const DETAIL_CHARS: usize = 120;

impl PreviousSession {
    fn line(&self) -> String {
        let ending = match self.ending {
            PageEnding::NoPong => "its pings went unanswered",
            PageEnding::StreamEnded => "its control stream ended",
            PageEnding::ReadFailed => "a read failed",
            PageEnding::WriteFailed => "a write failed",
            PageEnding::TransportClosed => "its connection closed",
            PageEnding::GatewayClosing => "the gateway closed it",
            PageEnding::Other => "something this server does not name",
        };
        let mut line = format!(
            "{ending} after {} s, {} ms after the last pong",
            self.open_seconds, self.since_pong_ms
        );
        if self.hidden {
            line.push_str(", the page hidden");
        }
        if let Some(detail) = &self.detail {
            // What the browser said, kept to one short line of plain text.
            let detail: String = detail
                .chars()
                .take(DETAIL_CHARS)
                .map(|c| {
                    if c.is_ascii_graphic() || c == ' ' {
                        c
                    } else {
                        '?'
                    }
                })
                .collect();
            line.push_str(": ");
            line.push_str(&detail);
        }
        line
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SessionView {
    granted: bool,
    /// Why not: `unavailable`, `blocked` or `clock`.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    certificate_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in_seconds: Option<u32>,
}

impl SessionView {
    fn refused(reason: &'static str) -> Self {
        Self {
            granted: false,
            reason: Some(reason),
            port: None,
            certificate_sha256: None,
            token: None,
            expires_in_seconds: None,
        }
    }
}

/// `POST /api/transport/quic/session`: the gateway's port, the hash of its
/// certificate and a token for one session of this web session.
pub(crate) async fn session(
    State(state): State<AppState>,
    Extension(CurrentSession(session)): Extension<CurrentSession>,
    Json(body): Json<SessionBody>,
) -> AppResult<Json<ApiResponse<SessionView>>> {
    if let Some(previous) = &body.previous {
        eprintln!(
            "quic link: a page's last session ended: {}",
            previous.line()
        );
    }
    let Some(transport) = state.quic.as_ref() else {
        return Ok(Json(ApiResponse::ok(SessionView::refused("unavailable"))));
    };
    if crate::firewall_modes::quic_blocked_now(&state).await {
        return Ok(Json(ApiResponse::ok(SessionView::refused("blocked"))));
    }
    let view = match transport.grant(&session, body.page_time) {
        Ok(granted) => SessionView {
            granted: true,
            reason: None,
            port: Some(granted.port),
            certificate_sha256: Some(granted.certificate),
            token: Some(granted.token),
            expires_in_seconds: Some(TOKEN_SECONDS),
        },
        Err(NoToken::Unavailable) => SessionView::refused("unavailable"),
        Err(NoToken::Clock) => SessionView::refused("clock"),
    };
    Ok(Json(ApiResponse::ok(view)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::session::SessionStore;

    async fn transport_with_gateway() -> (QuicTransport, Session, mpsc::Receiver<Vec<u8>>) {
        let session = SessionStore::new().issue("operator", 900).await;
        let transport = QuicTransport::default();
        let (outbox, queued) = mpsc::channel(8);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        transport.connected(
            Hello {
                certificate: "ab".repeat(32),
                not_before: now - 3600,
                not_after: now + 12 * 24 * 3600,
                port: 443,
            },
            outbox,
        );
        (transport, session, queued)
    }

    fn page_time() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    #[tokio::test]
    async fn a_token_that_waited_too_long_is_worth_nothing() {
        let (transport, session, _queued) = transport_with_gateway().await;
        let granted = transport.grant(&session, page_time()).ok().unwrap();
        transport
            .inner
            .lock()
            .unwrap()
            .tokens
            .get_mut(&granted.token)
            .unwrap()
            .expires = Instant::now() - Duration::from_millis(1);
        assert_eq!(transport.redeem(&granted.token), Err(RedeemRefusal::Token));
        // It is gone, not merely refused: the next grant cleans up as well.
        let fresh = transport.grant(&session, page_time()).ok().unwrap();
        assert_eq!(transport.inner.lock().unwrap().tokens.len(), 1);
        assert!(transport.redeem(&fresh.token).is_ok());
    }

    #[tokio::test]
    async fn waiting_tokens_are_bounded_and_the_oldest_gives_way() {
        let (transport, session, _queued) = transport_with_gateway().await;
        let first = transport.grant(&session, page_time()).ok().unwrap().token;
        for _ in 0..MAX_TOKENS {
            assert!(transport.grant(&session, page_time()).is_ok());
        }
        assert_eq!(transport.inner.lock().unwrap().tokens.len(), MAX_TOKENS);
        assert_eq!(transport.redeem(&first), Err(RedeemRefusal::Token));
    }

    #[tokio::test]
    async fn a_token_of_a_session_that_ended_is_refused() {
        let store = SessionStore::new();
        let session = store.issue("operator", 900).await;
        let (transport, _, _queued) = transport_with_gateway().await;
        let granted = transport.grant(&session, page_time()).ok().unwrap();
        let open = transport.grant(&session, page_time()).ok().unwrap();
        let reference = transport.redeem(&open.token).unwrap();
        store.revoke(&session.token).await;
        assert_eq!(transport.redeem(&granted.token), Err(RedeemRefusal::Token));
        assert_eq!(transport.take_ended(), vec![(reference, None)]);
        assert!(transport.take_ended().is_empty());
    }

    #[tokio::test]
    async fn a_session_that_ended_hands_back_its_lease() {
        // The sweep that ends it must release the lease; the gateway's
        // later word finds nothing (code audit of 2026-10-20, AUD-16.4).
        let store = SessionStore::new();
        let session = store.issue("operator", 900).await;
        let (transport, _, _queued) = transport_with_gateway().await;
        let open = transport.grant(&session, page_time()).ok().unwrap();
        let reference = transport.redeem(&open.token).unwrap();
        assert!(transport.note_lease(&reference, Some("lease-1".into())));
        store.revoke(&session.token).await;
        assert_eq!(
            transport.take_ended(),
            vec![(
                reference.clone(),
                Some((session.token.clone(), "lease-1".into()))
            )]
        );
        assert_eq!(transport.closed(&reference), None);
    }

    #[tokio::test]
    async fn messages_go_only_to_the_connection_that_is_current() {
        let (transport, _session, mut first) = transport_with_gateway().await;
        let generation = transport.inner.lock().unwrap().generations;
        let message = ServerToGateway::Hello { protocol: PROTOCOL };
        assert!(transport.send(generation, &message));
        assert!(first.recv().await.is_some());
        // A later connection replaces it; the earlier one hears nothing more
        // and cannot take the later one's place away by leaving.
        let (outbox, mut second) = mpsc::channel(8);
        let (later, _) = transport.connected(
            Hello {
                certificate: "cd".repeat(32),
                not_before: 1,
                not_after: 2,
                port: 443,
            },
            outbox,
        );
        assert!(!transport.send(generation, &message));
        transport.disconnected(generation);
        assert_eq!(transport.port(), Some(443));
        assert!(transport.send(later, &message));
        assert!(second.recv().await.is_some());
        transport.disconnected(later);
        assert_eq!(transport.port(), None);
    }

    /// After a reconnect the page renews and reports through the lease it
    /// took over the earlier session; that session's late close must not
    /// give the lease back (2026-10-24: remote control turned off a few
    /// seconds after each reconnect, with a renewal refused as LeaseConflict).
    #[tokio::test]
    async fn a_lease_used_over_a_later_session_stays_when_the_earlier_one_closes() {
        let (transport, session, _queued) = transport_with_gateway().await;
        let open = |transport: &QuicTransport| {
            let token = transport.grant(&session, page_time()).ok().unwrap().token;
            transport.redeem(&token).unwrap()
        };
        let lease = "5a".repeat(16);
        let earlier = open(&transport);
        assert!(transport.note_lease(&earlier, Some(lease.clone())));
        let later = open(&transport);
        assert!(transport.adopt_lease(&later, &lease));
        assert_eq!(transport.closed(&earlier), None);
        assert_eq!(
            transport.closed(&later),
            Some((session.token.clone(), lease.clone()))
        );
        assert!(
            !transport.adopt_lease(&later, &lease),
            "the session is gone"
        );
    }

    /// A session that ends says which lease it held, once, whatever ends
    /// it: its own connection, the link, or a gateway that takes the place.
    #[tokio::test]
    async fn a_session_that_ends_names_the_lease_it_held() {
        let (transport, session, _queued) = transport_with_gateway().await;
        let open = |transport: &QuicTransport| {
            let token = transport.grant(&session, page_time()).ok().unwrap().token;
            transport.redeem(&token).unwrap()
        };
        let lease = || Some("5a".repeat(16));
        let held = (session.token.clone(), "5a".repeat(16));

        // Its own connection ends: with a lease, after giving it back, and
        // without ever having had one.
        let with = open(&transport);
        assert!(transport.note_lease(&with, lease()));
        assert_eq!(transport.web_session(&with).unwrap().token, session.token);
        assert_eq!(transport.closed(&with), Some(held.clone()));
        assert_eq!(transport.closed(&with), None);
        assert!(!transport.note_lease(&with, lease()), "the session is gone");
        assert!(transport.web_session(&with).is_none());
        let returned = open(&transport);
        assert!(transport.note_lease(&returned, lease()));
        assert!(transport.note_lease(&returned, None));
        assert_eq!(transport.closed(&returned), None);
        let without = open(&transport);
        assert_eq!(transport.closed(&without), None);

        // The link ends.
        let generation = transport.inner.lock().unwrap().generations;
        let first = open(&transport);
        let _second = open(&transport);
        assert!(transport.note_lease(&first, lease()));
        assert_eq!(transport.disconnected(generation), vec![held.clone()]);
        assert!(transport.disconnected(generation).is_empty());

        // Another gateway takes the place.
        let (transport, session, _queued) = transport_with_gateway().await;
        let token = transport.grant(&session, page_time()).ok().unwrap().token;
        let reference = transport.redeem(&token).unwrap();
        assert!(transport.note_lease(&reference, lease()));
        let (outbox, _later) = mpsc::channel(8);
        let (_, replaced) = transport.connected(
            Hello {
                certificate: "ef".repeat(32),
                not_before: 1,
                not_after: 2,
                port: 443,
            },
            outbox,
        );
        assert_eq!(replaced, vec![(session.token.clone(), "5a".repeat(16))]);
    }

    #[test]
    fn the_journal_says_how_a_page_lost_its_last_session() {
        let body: SessionBody = serde_json::from_value(serde_json::json!({
            "pageTime": 1,
            "previous": {
                "ending": "no_pong",
                "openSeconds": 152,
                "sincePongMs": 6012,
                "hidden": false
            }
        }))
        .unwrap();
        assert_eq!(
            body.previous.unwrap().line(),
            "its pings went unanswered after 152 s, 6012 ms after the last pong"
        );

        // What the browser said is one short line of plain text.
        let previous: PreviousSession = serde_json::from_value(serde_json::json!({
            "ending": "transport_closed",
            "openSeconds": 3,
            "sincePongMs": 40,
            "hidden": true,
            "detail": format!("WebTransportError: lost\nnext line é {}", "x".repeat(200))
        }))
        .unwrap();
        let line = previous.line();
        assert!(
            line.starts_with(
                "its connection closed after 3 s, 40 ms after the last pong, the page hidden: \
                 WebTransportError: lost?next line ? xx"
            ),
            "{line}"
        );
        let (_, detail) = line.split_once("hidden: ").unwrap();
        assert_eq!(detail.chars().count(), DETAIL_CHARS);

        // A later page's names and fields are not refused; an older page
        // says nothing.
        let later: SessionBody = serde_json::from_value(serde_json::json!({
            "pageTime": 1,
            "previous": {
                "ending": "something_new",
                "openSeconds": 1,
                "sincePongMs": 1,
                "hidden": false,
                "more": 1
            }
        }))
        .unwrap();
        assert!(
            later
                .previous
                .unwrap()
                .line()
                .starts_with("something this server")
        );
        let older: SessionBody =
            serde_json::from_value(serde_json::json!({ "pageTime": 1 })).unwrap();
        assert!(older.previous.is_none());
    }
}
