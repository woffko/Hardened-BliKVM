//! The server's side of the QUIC transport against a gateway that speaks the
//! link's messages over the Unix socket.

use super::*;
use bli_kvm_quic_protocol::link::{
    self, EndReason, GatewayToServer, RedeemRefusal, ServerToGateway,
};
use tokio::net::UnixStream;
use tokio::time::{Duration, timeout};

const PATIENCE: Duration = Duration::from_secs(5);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// The gateway's end of the link.
struct Gateway(UnixStream);

impl Gateway {
    async fn connect(path: &std::path::Path) -> Self {
        // The listener is bound when the router is built.
        Self(UnixStream::connect(path).await.unwrap())
    }

    async fn say(&mut self, message: &GatewayToServer) {
        self.0
            .write_all(&link::encode(message).unwrap())
            .await
            .unwrap();
    }

    /// The next message, or `None` once the server closed the link.
    async fn hear(&mut self) -> Option<ServerToGateway> {
        let mut prefix = [0u8; 4];
        timeout(PATIENCE, self.0.read_exact(&mut prefix))
            .await
            .expect("the server said nothing")
            .ok()?;
        let mut body = vec![0u8; link::body_length(prefix).unwrap()];
        self.0.read_exact(&mut body).await.ok()?;
        Some(link::decode(&body).unwrap())
    }

    async fn hello(&mut self, protocol: u16, certificate: &str, port: u16) {
        let now = now_ms() / 1000;
        self.say(&GatewayToServer::Hello {
            protocol,
            certificate_sha256: certificate.into(),
            not_before: now - 3600,
            not_after: now - 3600 + 13 * 24 * 3600,
            port,
        })
        .await;
    }

    /// Says hello as the current protocol and reads the server's answer.
    async fn greeted(path: &std::path::Path, certificate: &str) -> Self {
        let mut gateway = Self::connect(path).await;
        gateway
            .hello(bli_kvm_quic_protocol::PROTOCOL, certificate, 443)
            .await;
        assert_eq!(
            gateway.hear().await,
            Some(ServerToGateway::Hello {
                protocol: bli_kvm_quic_protocol::PROTOCOL
            })
        );
        gateway
    }

    async fn redeem(&mut self, id: u64, token: &str) -> ServerToGateway {
        self.say(&GatewayToServer::Redeem {
            id,
            token: token.into(),
            peer: "192.0.2.10:50000".into(),
        })
        .await;
        self.hear().await.unwrap()
    }
}

struct Page {
    app: Router,
    cookie: String,
    csrf: String,
}

impl Page {
    async fn call(&self, method: Method, uri: &str, body: Value) -> Value {
        let mut request = json_request(method, uri, body);
        request
            .headers_mut()
            .insert(header::COOKIE, self.cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", self.csrf.parse().unwrap());
        json_body(self.app.clone().oneshot(request).await.unwrap()).await
    }

    async fn session(&self, page_time: u64) -> Value {
        self.call(
            Method::POST,
            "/api/transport/quic/session",
            serde_json::json!({ "pageTime": page_time }),
        )
        .await["data"]
            .clone()
    }

    async fn token(&self) -> String {
        let granted = self.session(now_ms()).await;
        assert_eq!(granted["granted"], true, "{granted}");
        granted["token"].as_str().unwrap().to_owned()
    }
}

async fn board(change: impl FnOnce(&mut ServerOptions)) -> (tempfile::TempDir, PathBuf, Page) {
    let root = tempdir().unwrap();
    let socket = root.path().join("link.sock");
    let mut options = ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(socket.clone()),
        ..ServerOptions::default()
    };
    change(&mut options);
    let app = app_with_options(options);
    let (cookie, csrf) = setup_and_login(&app).await;
    (root, socket, Page { app, cookie, csrf })
}

fn session_of(answer: ServerToGateway) -> String {
    match answer {
        ServerToGateway::Redeemed {
            session: Some(session),
            refused: None,
            ..
        } => session,
        other => panic!("not redeemed: {other:?}"),
    }
}

#[tokio::test]
async fn without_a_link_the_transport_is_not_supported() {
    let root = tempdir().unwrap();
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };
    let state = page.call(Method::GET, "/api/transport", Value::Null).await;
    assert_eq!(
        state["data"]["quic"],
        serde_json::json!({
            "supported": false,
            "enabled": false,
            "active": false,
            "connected": false,
            "blocked": false
        })
    );
    let refused = page.session(now_ms()).await;
    assert_eq!(
        refused,
        serde_json::json!({ "granted": false, "reason": "unavailable" })
    );
    let switched = page
        .call(Method::POST, "/api/transport/quic/enable", Value::Null)
        .await;
    assert_ne!(switched["code"], 0);
}

#[tokio::test]
async fn the_transport_needs_a_login_and_the_csrf_token() {
    let (_root, _socket, page) = board(|_| {}).await;
    for (method, uri) in [
        (Method::GET, "/api/transport"),
        (Method::POST, "/api/transport/quic/session"),
        (Method::POST, "/api/transport/quic/enable"),
        (Method::POST, "/api/transport/quic/disable"),
    ] {
        let anonymous = page
            .app
            .clone()
            .oneshot(json_request(
                method,
                uri,
                serde_json::json!({ "pageTime": 1 }),
            ))
            .await
            .unwrap();
        assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED, "{uri}");
    }
    let mut request = json_request(
        Method::POST,
        "/api/transport/quic/session",
        serde_json::json!({ "pageTime": now_ms() }),
    );
    request
        .headers_mut()
        .insert(header::COOKIE, page.cookie.parse().unwrap());
    let without_csrf = page.app.clone().oneshot(request).await.unwrap();
    assert_eq!(without_csrf.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_token_is_granted_while_the_gateway_is_on_the_link_and_redeemed_once() {
    let (_root, socket, page) = board(|_| {}).await;
    // No gateway yet.
    assert_eq!(page.session(now_ms()).await["reason"], "unavailable");
    let state = page.call(Method::GET, "/api/transport", Value::Null).await;
    assert_eq!(state["data"]["quic"]["supported"], true);
    assert_eq!(state["data"]["quic"]["connected"], false);

    let certificate = "ab".repeat(32);
    let mut gateway = Gateway::greeted(&socket, &certificate).await;
    let state = page.call(Method::GET, "/api/transport", Value::Null).await;
    assert_eq!(state["data"]["quic"]["connected"], true);
    assert_eq!(state["data"]["quic"]["port"], 443);

    let granted = page.session(now_ms()).await;
    assert_eq!(granted["granted"], true);
    assert_eq!(granted["port"], 443);
    assert_eq!(granted["certificateSha256"], certificate);
    assert_eq!(granted["expiresInSeconds"], 30);
    let token = granted["token"].as_str().unwrap().to_owned();
    assert!(bli_kvm_quic_protocol::page::token_is_well_formed(&token));
    // Two requests give two tokens.
    assert_ne!(page.token().await, token);

    let session = session_of(gateway.redeem(1, &token).await);
    assert_eq!(session.len(), 32);
    // The reference is not the web session's cookie or the token.
    assert!(!page.cookie.contains(&session));
    assert_ne!(session, token);
    assert_eq!(
        gateway.redeem(2, &token).await,
        ServerToGateway::Redeemed {
            id: 2,
            session: None,
            refused: Some(RedeemRefusal::Token)
        }
    );
    assert_eq!(
        gateway.redeem(3, &"00".repeat(32)).await,
        ServerToGateway::Redeemed {
            id: 3,
            session: None,
            refused: Some(RedeemRefusal::Token)
        }
    );
}

#[tokio::test]
async fn a_page_whose_clock_is_outside_the_certificate_is_told_so() {
    let (_root, socket, page) = board(|_| {}).await;
    let _gateway = Gateway::greeted(&socket, &"cd".repeat(32)).await;
    let now = now_ms();
    let day = 24 * 3600 * 1000;
    assert_eq!(page.session(now - 2 * 3600 * 1000).await["reason"], "clock");
    assert_eq!(page.session(now + 13 * day).await["reason"], "clock");
    assert_eq!(page.session(now + 12 * day).await["granted"], true);
    assert_eq!(page.session(now - 1800 * 1000).await["granted"], true);
    // A body without the page's clock, or with more than it, is refused.
    for body in [
        serde_json::json!({}),
        serde_json::json!({ "pageTime": now, "more": 1 }),
        serde_json::json!({ "pageTime": "now" }),
    ] {
        let mut request = json_request(Method::POST, "/api/transport/quic/session", body);
        request
            .headers_mut()
            .insert(header::COOKIE, page.cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", page.csrf.parse().unwrap());
        let answer = page.app.clone().oneshot(request).await.unwrap();
        assert!(answer.status().is_client_error(), "{}", answer.status());
    }
}

#[tokio::test]
async fn a_web_session_has_at_most_two_quic_sessions() {
    let (_root, socket, page) = board(|_| {}).await;
    let mut gateway = Gateway::greeted(&socket, &"ef".repeat(32)).await;
    let first = session_of(gateway.redeem(1, &page.token().await).await);
    let _second = session_of(gateway.redeem(2, &page.token().await).await);
    assert_eq!(
        gateway.redeem(3, &page.token().await).await,
        ServerToGateway::Redeemed {
            id: 3,
            session: None,
            refused: Some(RedeemRefusal::Sessions)
        }
    );
    // One that closes makes room for another.
    gateway
        .say(&GatewayToServer::Closed { session: first })
        .await;
    let token = page.token().await;
    let third = gateway.redeem(4, &token).await;
    session_of(third);
}

#[tokio::test]
async fn a_logout_ends_the_web_sessions_quic_sessions() {
    let (_root, socket, page) = board(|_| {}).await;
    let mut gateway = Gateway::greeted(&socket, &"12".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    let waiting = page.token().await;

    let logout = page
        .call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(logout["code"], 0);
    assert_eq!(
        gateway.hear().await,
        Some(ServerToGateway::End {
            session,
            reason: EndReason::Session
        })
    );
    // A token issued before the logout is worth nothing after it.
    assert_eq!(
        gateway.redeem(2, &waiting).await,
        ServerToGateway::Redeemed {
            id: 2,
            session: None,
            refused: Some(RedeemRefusal::Token)
        }
    );
}

#[tokio::test]
async fn a_gateway_that_leaves_takes_its_sessions_and_tokens_with_it() {
    let (_root, socket, page) = board(|_| {}).await;
    let mut gateway = Gateway::greeted(&socket, &"34".repeat(32)).await;
    let waiting = page.token().await;
    drop(gateway.redeem(1, &page.token().await).await);
    drop(gateway);
    // The server notices the closed link.
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        if page.session(now_ms()).await["reason"] == "unavailable" {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let mut again = Gateway::greeted(&socket, &"56".repeat(32)).await;
    assert_eq!(
        page.session(now_ms()).await["certificateSha256"],
        "56".repeat(32)
    );
    assert_eq!(
        again.redeem(1, &waiting).await,
        ServerToGateway::Redeemed {
            id: 1,
            session: None,
            refused: Some(RedeemRefusal::Token)
        }
    );
}

#[tokio::test]
async fn a_replaced_certificate_reaches_the_next_page() {
    let (_root, socket, page) = board(|_| {}).await;
    let mut gateway = Gateway::greeted(&socket, &"78".repeat(32)).await;
    let old = page.token().await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    gateway
        .hello(bli_kvm_quic_protocol::PROTOCOL, &"9a".repeat(32), 443)
        .await;
    // The second hello is not answered; the next answer is to the redeem.
    assert_eq!(
        gateway.redeem(2, &old).await,
        ServerToGateway::Redeemed {
            id: 2,
            session: None,
            refused: Some(RedeemRefusal::Token)
        },
        "a token issued with the old hash is dropped"
    );
    assert_eq!(
        page.session(now_ms()).await["certificateSha256"],
        "9a".repeat(32)
    );
    // The open session goes on: its web session can still be ended.
    page.call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(
        gateway.hear().await,
        Some(ServerToGateway::End {
            session,
            reason: EndReason::Session
        })
    );
}

#[tokio::test]
async fn another_user_and_another_protocol_are_not_served() {
    // The link belongs to a user this test is not.
    let own = {
        use std::os::unix::fs::MetadataExt;
        std::fs::metadata(".").unwrap().uid()
    };
    let (_root, socket, page) = board(|options| options.quic_link_uid = Some(own + 1)).await;
    let mut stranger = Gateway::connect(&socket).await;
    stranger
        .hello(bli_kvm_quic_protocol::PROTOCOL, &"bc".repeat(32), 443)
        .await;
    assert_eq!(
        stranger.hear().await,
        None,
        "the connection is closed unanswered"
    );
    assert_eq!(page.session(now_ms()).await["reason"], "unavailable");

    let (_root, socket, page) = board(|_| {}).await;
    let mut old = Gateway::connect(&socket).await;
    old.hello(bli_kvm_quic_protocol::PROTOCOL + 1, &"bc".repeat(32), 443)
        .await;
    assert_eq!(
        old.hear().await,
        Some(ServerToGateway::Hello {
            protocol: bli_kvm_quic_protocol::PROTOCOL
        }),
        "the server names its own protocol"
    );
    assert_eq!(old.hear().await, None);
    assert_eq!(page.session(now_ms()).await["reason"], "unavailable");

    // What is not a hello gets no answer.
    let (_root, socket, page) = board(|_| {}).await;
    let mut rude = Gateway::connect(&socket).await;
    rude.say(&GatewayToServer::Closed {
        session: "x".into(),
    })
    .await;
    assert_eq!(rude.hear().await, None);
    let mut bad = Gateway::connect(&socket).await;
    bad.hello(bli_kvm_quic_protocol::PROTOCOL, "not a hash", 443)
        .await;
    assert_eq!(bad.hear().await, None);
    assert_eq!(page.session(now_ms()).await["reason"], "unavailable");
}

/// The board owner's control socket: answers each request with what
/// `answer` makes of it, and records the requests.
fn owner(
    socket: &std::path::Path,
    mut answer: impl FnMut(
        &bli_kvm_core::PlatformControlRequestV2,
    ) -> bli_kvm_control_protocol::ControlResponse
    + Send
    + 'static,
) -> std::sync::Arc<std::sync::Mutex<Vec<bli_kvm_core::PlatformControlRequestV2>>> {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let requests = std::sync::Arc::clone(&seen);
    let listener = UnixListener::bind(socket).unwrap();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut prefix = [0; 4];
            stream.read_exact(&mut prefix).await.unwrap();
            let mut bytes = vec![0; u32::from_be_bytes(prefix) as usize];
            stream.read_exact(&mut bytes).await.unwrap();
            let envelope = bli_kvm_control_protocol::decode_request_v2(&bytes).unwrap();
            let response = bli_kvm_control_protocol::ControlResponseEnvelope {
                schema_version: 2,
                request_id: envelope.request_id,
                operation_id: envelope.operation_id,
                response: answer(&envelope.request),
            };
            requests.lock().unwrap().push(envelope.request);
            let bytes = bli_kvm_control_protocol::encode_response_v2(&response).unwrap();
            stream
                .write_all(&bli_kvm_control_protocol::encode_frame(&bytes).unwrap())
                .await
                .unwrap();
        }
    });
    seen
}

#[cfg(feature = "restricted-control-surface")]
#[tokio::test]
async fn the_switch_is_the_owners_and_the_manifest_names_the_transport() {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::PlatformControlRequestV2 as Request;
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    let mut on = false;
    let seen = owner(&control, move |request| match request {
        Request::Quic { enabled } => {
            on = *enabled;
            ControlResponse::Applied
        }
        Request::QuicState {} => ControlResponse::QuicState {
            enabled: on,
            active: on,
        },
        _ => panic!("unexpected request"),
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(root.path().join("link.sock")),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };

    let manifest = page
        .call(Method::GET, "/api/capabilities", Value::Null)
        .await;
    assert_eq!(
        manifest["data"]["board"]["features"]["network_quic"]["availability"],
        "read_write"
    );

    let state = page.call(Method::GET, "/api/transport", Value::Null).await;
    assert_eq!(state["data"]["quic"]["enabled"], false);
    let switched = page
        .call(Method::POST, "/api/transport/quic/enable", Value::Null)
        .await;
    assert_eq!(switched["code"], 0, "{switched}");
    assert_eq!(switched["data"]["quic"]["enabled"], true);
    assert_eq!(switched["data"]["quic"]["active"], true);
    // Running, but not yet on the link.
    assert_eq!(switched["data"]["quic"]["connected"], false);
    let switched = page
        .call(Method::POST, "/api/transport/quic/disable", Value::Null)
        .await;
    assert_eq!(switched["data"]["quic"]["enabled"], false);
    assert_eq!(
        *seen.lock().unwrap(),
        vec![
            Request::QuicState {},
            Request::Quic { enabled: true },
            Request::QuicState {},
            Request::Quic { enabled: false },
            Request::QuicState {},
        ]
    );
}

#[cfg(feature = "restricted-control-surface")]
#[tokio::test]
async fn a_switch_the_owner_refuses_is_reported_and_no_link_means_no_feature() {
    use bli_kvm_control_protocol::{ControlRejectionCode, ControlResponse};
    let root = tempdir().unwrap();
    let control = root.path().join("control.sock");
    owner(&control, |_| ControlResponse::Rejected {
        code: ControlRejectionCode::BackendRejected,
    });
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(root.path().join("link.sock")),
        control_socket: Some(control.clone()),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };
    let refused = page
        .call(Method::POST, "/api/transport/quic/enable", Value::Null)
        .await;
    assert_ne!(refused["code"], 0);
    let state = page.call(Method::GET, "/api/transport", Value::Null).await;
    assert_eq!(state["data"]["quic"]["enabled"], false);

    let bare = tempdir().unwrap();
    let app = app_with_options(ServerOptions {
        state_dir: bare.path().join("account"),
        control_socket: Some(control),
        restricted_controls_enabled: true,
        ..ServerOptions::default()
    });
    let manifest = json_body(
        app.oneshot(json_request(Method::GET, "/api/capabilities", Value::Null))
            .await
            .unwrap(),
    )
    .await;
    assert!(manifest["data"]["board"]["features"]["network_quic"].is_null());
}

/// The owner as the lease path meets it: a target attestation and every
/// lease request are applied, an acquire grants the lease `[0x55; 16]`.
fn hid_owner(
    socket: &std::path::Path,
) -> std::sync::Arc<std::sync::Mutex<Vec<bli_kvm_core::PlatformControlRequestV2>>> {
    use bli_kvm_control_protocol::ControlResponse;
    use bli_kvm_core::{HidLeaseRequest, PlatformControlRequestV2 as Request};
    owner(socket, |request| match request {
        Request::HidLease {
            lease: HidLeaseRequest::Acquire { .. },
        } => ControlResponse::LeaseGranted {
            lease_id: [0x55; 16],
        },
        _ => ControlResponse::Applied,
    })
}

const LEASE: &str = "55555555555555555555555555555555";

/// The lease requests the owner has seen, without the attestations.
fn lease_requests(
    seen: &std::sync::Mutex<Vec<bli_kvm_core::PlatformControlRequestV2>>,
) -> Vec<bli_kvm_core::HidLeaseRequest> {
    seen.lock()
        .unwrap()
        .iter()
        .filter_map(|request| match request {
            bli_kvm_core::PlatformControlRequestV2::HidLease { lease } => Some(lease.clone()),
            _ => None,
        })
        .collect()
}

async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + PATIENCE;
    while !check() {
        assert!(std::time::Instant::now() < deadline, "{what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A board with an owner for the HID lease, a logged-in page and a gateway
/// on the link with one session of that page.
async fn board_with_input() -> (
    tempfile::TempDir,
    PathBuf,
    Page,
    Gateway,
    String,
    std::sync::Arc<std::sync::Mutex<Vec<bli_kvm_core::PlatformControlRequestV2>>>,
) {
    let root = tempdir().unwrap();
    let socket = root.path().join("link.sock");
    let control = root.path().join("control.sock");
    let seen = hid_owner(&control);
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(socket.clone()),
        control_socket: Some(control),
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };
    let mut gateway = Gateway::greeted(&socket, &"12".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    (root, socket, page, gateway, session, seen)
}

impl Page {
    /// The operator confirms the target and the focus, as the page does
    /// before it acquires a lease.
    async fn confirm_focus(&self) {
        let challenge =
            self.call(Method::GET, "/api/hid/focus", Value::Null).await["data"]["challenge"]
                .as_str()
                .unwrap()
                .to_owned();
        let confirmed = self
            .call(
                Method::POST,
                "/api/hid/focus",
                serde_json::json!({
                    "operationId": "0123456789abcdef0123456789abcdef",
                    "challenge": challenge,
                    "targetId": "11".repeat(16),
                    "operatorConfirmed": true,
                }),
            )
            .await;
        assert_eq!(confirmed["data"]["attested"], true, "{confirmed}");
    }

    /// A request over HTTPS with its status, the way the page sees both.
    async fn post(&self, uri: &str, body: Value) -> (StatusCode, Value) {
        let mut request = json_request(Method::POST, uri, body);
        request
            .headers_mut()
            .insert(header::COOKIE, self.cookie.parse().unwrap());
        request
            .headers_mut()
            .insert("x-csrf-token", self.csrf.parse().unwrap());
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }
}

impl Gateway {
    /// A lease request from a session's input stream, and its answer.
    async fn lease(&mut self, id: u64, session: &str, request: Value) -> Value {
        self.say(&GatewayToServer::Lease {
            id,
            session: session.into(),
            request,
        })
        .await;
        match self.hear().await {
            Some(ServerToGateway::LeaseAnswer {
                id: answered,
                response,
            }) => {
                assert_eq!(answered, id);
                response
            }
            other => panic!("no lease answer: {other:?}"),
        }
    }
}

fn operation(number: u32) -> String {
    format!("{number:032x}")
}

fn key_report(number: u32, code: u8) -> Value {
    serde_json::json!({
        "action": "report",
        "operationId": operation(number),
        "leaseId": LEASE,
        "report": { "mode": "keyboard", "data": [0, 0, code, 0, 0, 0, 0, 0] },
    })
}

/// A lease request from a QUIC session is served by the code behind
/// `/api/hid/lease`, for the web session the token belonged to: the answers
/// are that API's, the owner sees the same requests, and their order is kept.
#[tokio::test]
async fn a_lease_request_over_the_link_is_served_like_the_apis() {
    use bli_kvm_core::{HidLeaseRequest, HidReport};
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    page.confirm_focus().await;

    let acquired = gateway
        .lease(
            2,
            &session,
            serde_json::json!({ "action": "acquire", "operationId": operation(1), "ttlSeconds": 30 }),
        )
        .await;
    assert_eq!(
        acquired,
        serde_json::json!({
            "status": 200,
            "body": {
                "code": 0,
                "msg": "success",
                "data": { "applied": true, "leaseId": LEASE, "ttlSeconds": 30 },
            },
        })
    );
    // Forty key reports, one after the other: every one reaches the owner,
    // in the order the session sent them.
    let codes: Vec<u8> = (4..44).collect();
    for (index, code) in codes.iter().enumerate() {
        let answer = gateway
            .lease(
                10 + index as u64,
                &session,
                key_report(100 + index as u32, *code),
            )
            .await;
        assert_eq!(answer["status"], 200, "{answer}");
        assert_eq!(answer["body"]["data"]["applied"], true, "{answer}");
    }
    let released = gateway
        .lease(
            90,
            &session,
            serde_json::json!({ "action": "release", "operationId": operation(2), "leaseId": LEASE }),
        )
        .await;
    assert_eq!(released["body"]["data"]["applied"], true, "{released}");

    let requests = lease_requests(&seen);
    assert_eq!(requests.len(), codes.len() + 2);
    let HidLeaseRequest::Acquire {
        session_binding,
        target_binding,
        ttl_seconds: 30,
    } = requests[0]
    else {
        panic!("not an acquire: {:?}", requests[0]);
    };
    let reported: Vec<u8> = requests[1..=codes.len()]
        .iter()
        .map(|request| match request {
            HidLeaseRequest::Report {
                lease_id,
                session_binding: session,
                target_binding: target,
                report: HidReport::Keyboard { data },
            } => {
                assert_eq!(*lease_id, [0x55; 16]);
                // The bindings of the page's own confirmation.
                assert_eq!((*session, *target), (session_binding, target_binding));
                data[2]
            }
            other => panic!("not a key report: {other:?}"),
        })
        .collect();
    assert_eq!(reported, codes);
    assert!(matches!(
        requests.last(),
        Some(HidLeaseRequest::Release { lease_id, .. }) if *lease_id == [0x55; 16]
    ));

    // The lease is the web session's, whichever transport took it: the
    // HTTPS route renews and reports under the one acquired over the link.
    let again = gateway
        .lease(
            91,
            &session,
            serde_json::json!({ "action": "acquire", "operationId": operation(3), "ttlSeconds": 30 }),
        )
        .await;
    assert_eq!(again["body"]["data"]["leaseId"], LEASE);
    let (status, over_https) = page.post("/api/hid/lease", key_report(4, 9)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(over_https["data"]["applied"], true, "{over_https}");
}

/// Without a confirmation of the target the link gets what HTTPS gives, word
/// for word, and nothing reaches the owner.
#[tokio::test]
async fn without_a_confirmation_the_link_refuses_as_https_does() {
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    let requests = [
        serde_json::json!({ "action": "acquire", "operationId": operation(1), "ttlSeconds": 30 }),
        key_report(2, 4),
        serde_json::json!({ "action": "renew", "operationId": operation(3), "leaseId": LEASE, "ttlSeconds": 30 }),
        serde_json::json!({ "action": "release", "operationId": operation(4), "leaseId": LEASE }),
        // An operation id that is none.
        serde_json::json!({ "action": "acquire", "operationId": "short", "ttlSeconds": 30 }),
    ];
    for (index, request) in requests.into_iter().enumerate() {
        let (status, body) = page.post("/api/hid/lease", request.clone()).await;
        let over_the_link = gateway.lease(10 + index as u64, &session, request).await;
        assert_eq!(
            over_the_link,
            serde_json::json!({ "status": status.as_u16(), "body": body }),
        );
        assert_eq!(body["code"], -1, "{body}");
    }
    assert_eq!(
        gateway.lease(20, &session, key_report(5, 4)).await["body"]["msg"],
        "a current focus confirmation is required"
    );
    // What is no lease request is refused with the status HTTPS answers.
    for request in [
        serde_json::json!({ "action": "reboot" }),
        serde_json::json!({ "action": "acquire", "operationId": operation(6), "ttlSeconds": 30, "more": 1 }),
        serde_json::json!([1, 2]),
    ] {
        let (status, _) = page.post("/api/hid/lease", request.clone()).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(gateway.lease(30, &session, request).await["status"], 422);
    }
    // A session the gateway only claims to have is nobody's.
    let unknown = gateway.lease(40, &"ab".repeat(16), key_report(7, 4)).await;
    assert_eq!(unknown["status"], 401);
    assert!(lease_requests(&seen).is_empty(), "the owner was asked");
}

/// A QUIC session that ends gives back the lease it took, like a client that
/// vanished: when its connection closes, and when the link goes. It gives
/// back nothing it did not take, and nothing twice.
#[tokio::test]
async fn a_quic_session_that_ends_gives_its_lease_back() {
    use bli_kvm_core::HidLeaseRequest;
    let acquire = |number: u32| serde_json::json!({ "action": "acquire", "operationId": operation(number), "ttlSeconds": 30 });
    let releases = |seen: &std::sync::Mutex<Vec<bli_kvm_core::PlatformControlRequestV2>>| {
        lease_requests(seen)
            .iter()
            .filter(|request| matches!(request, HidLeaseRequest::Release { lease_id, .. } if *lease_id == [0x55; 16]))
            .count()
    };

    // The session's connection closes.
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    page.confirm_focus().await;
    assert_eq!(gateway.lease(2, &session, acquire(1)).await["status"], 200);
    assert_eq!(releases(&seen), 0);
    gateway
        .say(&GatewayToServer::Closed {
            session: session.clone(),
        })
        .await;
    eventually("the lease was not given back", || releases(&seen) == 1).await;
    // The session is gone: its next request is nobody's.
    assert_eq!(
        gateway.lease(3, &session, key_report(2, 4)).await["status"],
        401
    );

    // The link goes.
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    page.confirm_focus().await;
    assert_eq!(gateway.lease(2, &session, acquire(1)).await["status"], 200);
    drop(gateway);
    eventually("the lease was not given back", || releases(&seen) == 1).await;

    // A lease the session gave back itself is not given back again.
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    page.confirm_focus().await;
    assert_eq!(gateway.lease(2, &session, acquire(1)).await["status"], 200);
    let released = gateway
        .lease(
            3,
            &session,
            serde_json::json!({ "action": "release", "operationId": operation(2), "leaseId": LEASE }),
        )
        .await;
    assert_eq!(released["body"]["data"]["applied"], true);
    gateway
        .say(&GatewayToServer::Closed {
            session: session.clone(),
        })
        .await;
    // The next answer shows the server has worked through the close.
    assert_eq!(
        gateway.lease(4, &session, key_report(3, 4)).await["status"],
        401
    );
    assert_eq!(releases(&seen), 1);

    // A lease taken over HTTPS is not the QUIC session's to give back.
    let (_root, _socket, page, mut gateway, session, seen) = board_with_input().await;
    page.confirm_focus().await;
    let (_, over_https) = page.post("/api/hid/lease", acquire(1)).await;
    assert_eq!(over_https["data"]["leaseId"], LEASE);
    gateway
        .say(&GatewayToServer::Closed {
            session: session.clone(),
        })
        .await;
    assert_eq!(
        gateway.lease(5, &session, key_report(4, 4)).await["status"],
        401
    );
    assert_eq!(releases(&seen), 0);
}

/// Another login of the same operator, as a page of its own.
async fn another_page(app: &Router) -> Page {
    let login = app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/auth/login",
            serde_json::json!({ "username": "operator", "password": "initial password" }),
        ))
        .await
        .unwrap();
    let cookie = login.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let csrf = json_body(login).await["data"]["csrfToken"]
        .as_str()
        .unwrap()
        .to_owned();
    Page {
        app: app.clone(),
        cookie,
        csrf,
    }
}

/// A lease request over the link is the web session's activity, as a request
/// over HTTPS is: it keeps the idle lock away, and a session that has ended
/// is refused.
#[tokio::test]
async fn a_lease_request_over_the_link_counts_for_the_web_session() {
    let (_root, _socket, page, mut gateway, session, _seen) = board_with_input().await;
    // The lock's time is also the lifetime of the session that sets it. Two
    // other logins keep their lifetime and are judged by their activity
    // alone: the page with the QUIC session, and one that does nothing.
    let idle = another_page(&page.app).await;
    let setter = another_page(&page.app).await;
    let (status, _) =
        set_session_lock(&page.app, &setter.cookie, Some(&setter.csrf), 1, false).await;
    assert_eq!(status, StatusCode::OK);
    // Longer than the lock, with a request over the link every 300 ms and
    // none over HTTPS.
    for index in 0..6 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let answer = gateway
            .lease(10 + index, &session, key_report(index as u32, 4))
            .await;
        assert_eq!(answer["status"], 200, "{answer}");
    }
    let (status, _) = page.post("/api/hid/lease", key_report(50, 4)).await;
    assert_eq!(status, StatusCode::OK, "the session was locked meanwhile");
    let (status, _) = idle.post("/api/hid/lease", key_report(51, 4)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "the idle session lives");

    page.call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(
        gateway.lease(30, &session, key_report(52, 4)).await["status"],
        401
    );
}

impl Gateway {
    /// A session asks for the picture, or gives it up: the server's answer.
    async fn video(&mut self, id: u64, session: &str, subscribe: bool) -> ServerToGateway {
        self.say(&GatewayToServer::Video {
            id,
            session: session.into(),
            subscribe,
        })
        .await;
        self.hear().await.unwrap()
    }
}

/// The H.264 encoder's demand socket: each connection's lines, and its end.
struct Demand(tokio::net::UnixListener);

impl Demand {
    async fn held(&self) -> tokio::io::Lines<tokio::io::BufReader<UnixStream>> {
        use tokio::io::AsyncBufReadExt;
        let (connection, _) = timeout(PATIENCE, self.0.accept())
            .await
            .expect("no demand was signalled")
            .unwrap();
        tokio::io::BufReader::new(connection).lines()
    }

    async fn idle(&self) {
        assert!(
            timeout(Duration::from_millis(200), self.0.accept())
                .await
                .is_err(),
            "demand was signalled"
        );
    }
}

async fn line(lines: &mut tokio::io::Lines<tokio::io::BufReader<UnixStream>>) -> Option<String> {
    timeout(PATIENCE, lines.next_line())
        .await
        .expect("the demand connection said nothing")
        .unwrap()
}

fn video_answer(id: u64, reason: Option<&str>) -> ServerToGateway {
    ServerToGateway::VideoAnswer {
        id,
        ok: reason.is_none(),
        reason: reason.map(str::to_owned),
    }
}

/// A session that asks for the picture holds the H.264 encoder's demand, as
/// a WebRTC viewer's signalling does, for as long as it watches: until it
/// gives the picture up, its connection closes, its web session ends, or the
/// link goes.
#[tokio::test]
async fn a_video_subscription_holds_the_encoders_demand() {
    let root = tempdir().unwrap();
    let socket = root.path().join("link.sock");
    let demand_path = root.path().join("h264.sock");
    let demand = Demand(tokio::net::UnixListener::bind(&demand_path).unwrap());
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(socket.clone()),
        webrtc_janus: Some("127.0.0.1:8188".parse().unwrap()),
        webrtc_h264_demand_socket: Some(demand_path),
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };
    let mut gateway = Gateway::greeted(&socket, &"9a".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    demand.idle().await;

    // It watches: the codec and the quality, as for a WebRTC viewer.
    gateway
        .say(&GatewayToServer::Video {
            id: 2,
            session: session.clone(),
            subscribe: true,
        })
        .await;
    let mut held = demand.held().await;
    assert_eq!(line(&mut held).await.as_deref(), Some("h264"));
    assert_eq!(line(&mut held).await.as_deref(), Some("kbps 2500"));
    assert_eq!(gateway.hear().await.unwrap(), video_answer(2, None));
    // It gives the picture up: the demand ends.
    assert_eq!(
        gateway.video(3, &session, false).await,
        video_answer(3, None)
    );
    assert_eq!(line(&mut held).await, None);

    // Asked twice, it holds one demand: the earlier one is let go.
    assert_eq!(
        gateway.video(4, &session, true).await,
        video_answer(4, None)
    );
    let mut first = demand.held().await;
    assert_eq!(
        gateway.video(5, &session, true).await,
        video_answer(5, None)
    );
    let mut second = demand.held().await;
    assert_eq!(line(&mut first).await.as_deref(), Some("h264"));
    assert_eq!(line(&mut first).await.as_deref(), Some("kbps 2500"));
    assert_eq!(line(&mut first).await, None);
    assert_eq!(line(&mut second).await.as_deref(), Some("h264"));
    assert_eq!(line(&mut second).await.as_deref(), Some("kbps 2500"));

    // Its connection closes.
    gateway
        .say(&GatewayToServer::Closed {
            session: session.clone(),
        })
        .await;
    assert_eq!(line(&mut second).await, None);
    // The session is gone: it is nobody's to watch for.
    assert_eq!(
        gateway.video(6, &session, true).await,
        video_answer(6, Some("session"))
    );
    demand.idle().await;

    // The web session ends.
    let session = session_of(gateway.redeem(7, &page.token().await).await);
    assert_eq!(
        gateway.video(8, &session, true).await,
        video_answer(8, None)
    );
    let mut held = demand.held().await;
    assert_eq!(line(&mut held).await.as_deref(), Some("h264"));
    page.call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(line(&mut held).await.as_deref(), Some("kbps 2500"));
    assert_eq!(line(&mut held).await, None);

    // The link goes.
    let (_root, socket, page) = board(|options| {
        options.webrtc_janus = Some("127.0.0.1:8188".parse().unwrap());
        options.webrtc_h264_demand_socket = Some(root.path().join("h264.sock"));
    })
    .await;
    let mut gateway = Gateway::greeted(&socket, &"9b".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    assert_eq!(
        gateway.video(2, &session, true).await,
        video_answer(2, None)
    );
    let mut held = demand.held().await;
    assert_eq!(line(&mut held).await.as_deref(), Some("h264"));
    assert_eq!(line(&mut held).await.as_deref(), Some("kbps 2500"));
    drop(gateway);
    assert_eq!(line(&mut held).await, None);
}

/// Without an H.264 encoder, or with one that cannot be reached, the session
/// is told so and nothing else changes.
#[tokio::test]
async fn without_an_encoder_there_is_no_picture() {
    // No encoder is provisioned.
    let (_root, socket, page) = board(|_| {}).await;
    let mut gateway = Gateway::greeted(&socket, &"9c".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    assert_eq!(
        gateway.video(2, &session, true).await,
        video_answer(2, Some("unavailable"))
    );
    // Giving up what one does not have is no error.
    assert_eq!(
        gateway.video(3, &session, false).await,
        video_answer(3, None)
    );
    assert_eq!(
        gateway.video(4, &"ab".repeat(16), true).await,
        video_answer(4, Some("session"))
    );

    // One is provisioned, and its socket is not there.
    let root = tempdir().unwrap();
    let (_other, socket, page) = board(|options| {
        options.webrtc_janus = Some("127.0.0.1:8188".parse().unwrap());
        options.webrtc_h264_demand_socket = Some(root.path().join("missing.sock"));
    })
    .await;
    let mut gateway = Gateway::greeted(&socket, &"9d".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    assert_eq!(
        gateway.video(2, &session, true).await,
        video_answer(2, Some("encoder"))
    );
    // The session is as it was: its lease requests are still served.
    assert_eq!(
        gateway.lease(3, &session, key_report(1, 4)).await["status"],
        503
    );
}

/// The quality the operator chooses reaches the encoder over the demand a
/// QUIC session holds, as it does over a WebRTC viewer's.
#[cfg(feature = "restricted-control-surface")]
#[tokio::test]
async fn the_quality_reaches_the_encoder_over_a_quic_sessions_demand() {
    let root = tempdir().unwrap();
    let socket = root.path().join("link.sock");
    let demand_path = root.path().join("h264.sock");
    let demand = Demand(tokio::net::UnixListener::bind(&demand_path).unwrap());
    let app = app_with_options(ServerOptions {
        state_dir: root.path().join("account"),
        quic_link_socket: Some(socket.clone()),
        restricted_controls_enabled: true,
        video_controls_enabled: true,
        control_socket: Some(root.path().join("control.sock")),
        webrtc_janus: Some("127.0.0.1:8188".parse().unwrap()),
        webrtc_h264_demand_socket: Some(demand_path),
        ..ServerOptions::default()
    });
    let (cookie, csrf) = setup_and_login(&app).await;
    let page = Page { app, cookie, csrf };
    let mut gateway = Gateway::greeted(&socket, &"9e".repeat(32)).await;
    let session = session_of(gateway.redeem(1, &page.token().await).await);
    assert_eq!(
        gateway.video(2, &session, true).await,
        video_answer(2, None)
    );
    let mut held = demand.held().await;
    assert_eq!(line(&mut held).await.as_deref(), Some("h264"));
    assert_eq!(line(&mut held).await.as_deref(), Some("kbps 2500"));
    let (_, changed) = page
        .post(
            "/api/vm/screen",
            serde_json::json!({ "type": "webrtc_kbps", "value": 8000 }),
        )
        .await;
    assert_eq!(changed["data"]["applied"], true, "{changed}");
    assert_eq!(line(&mut held).await.as_deref(), Some("kbps 8000"));
}

/// The gateway itself, on the board's link, the way its unit runs it.
struct RealGateway {
    port: u16,
    stop: tokio::sync::oneshot::Sender<()>,
    served: tokio::task::JoinHandle<()>,
}

impl RealGateway {
    fn start(socket: PathBuf) -> Self {
        let gateway = bli_kvm_quic_gateway::Gateway::bind(bli_kvm_quic_gateway::config::Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            link_socket: socket,
            counters_interval: Duration::from_millis(200),
            ..Default::default()
        })
        .unwrap();
        let port = gateway.local_addr().unwrap().port();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let served = tokio::spawn(gateway.serve(async {
            let _ = stopped.await;
        }));
        Self { port, stop, served }
    }

    /// The unit is stopped.
    async fn stop(self) {
        let _ = self.stop.send(());
        timeout(PATIENCE, self.served).await.unwrap().unwrap();
    }
}

/// What the session endpoint grants once the gateway has reached the server
/// by itself.
async fn granted(page: &Page) -> Value {
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let answer = page.session(now_ms()).await;
        if answer["granted"] == true {
            return answer;
        }
        assert!(std::time::Instant::now() < deadline, "{answer}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A page's connection to the gateway, made with the certificate's hash the
/// way a browser makes it.
async fn connect(port: u16, certificate: &str) -> Result<wtransport::Connection, String> {
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&certificate[index * 2..index * 2 + 2], 16).unwrap();
    }
    let client = wtransport::Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([wtransport::tls::Sha256Digest::new(digest)])
            .build(),
    )
    .map_err(|error| error.to_string())?;
    client
        .connect(
            wtransport::endpoint::ConnectOptions::builder(format!("https://127.0.0.1:{port}/kvm"))
                .add_header("origin", format!("https://127.0.0.1:{port}"))
                .build(),
        )
        .await
        .map_err(|error| error.to_string())
}

/// A page's control stream.
struct Control {
    send: wtransport::SendStream,
    receive: wtransport::RecvStream,
    reader: bli_kvm_quic_protocol::page::MessageReader,
}

impl Control {
    async fn open(connection: &wtransport::Connection) -> Self {
        let (send, receive) = connection.open_bi().await.unwrap().await.unwrap();
        Self {
            send,
            receive,
            reader: bli_kvm_quic_protocol::page::MessageReader::new(),
        }
    }

    async fn say(&mut self, message: &bli_kvm_quic_protocol::page::PageControl) {
        self.send
            .write_all(&bli_kvm_quic_protocol::page::encode(message).unwrap())
            .await
            .unwrap();
    }

    /// The next message that is not the periodic counters.
    async fn hear(&mut self) -> bli_kvm_quic_protocol::page::GatewayControl {
        use bli_kvm_quic_protocol::page::GatewayControl;
        let mut buffer = [0u8; 2048];
        loop {
            match self.reader.take::<GatewayControl>().unwrap() {
                Some(GatewayControl::Counters { .. }) => {}
                Some(message) => return message,
                None => {
                    let count = timeout(PATIENCE, self.receive.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    self.reader.push(&buffer[..count]);
                }
            }
        }
    }
}

/// The real gateway on the real link: a page gets a token over the API, opens
/// the WebTransport session with the certificate's hash, is welcomed, has a
/// ping answered, and is told when its web session ends.
#[tokio::test]
async fn a_page_opens_a_session_through_the_real_gateway_and_a_logout_ends_it() {
    use bli_kvm_quic_protocol::{
        PROTOCOL,
        page::{Closing, GatewayControl, PageControl},
    };
    let (_root, socket, page) = board(|_| {}).await;
    let gateway = RealGateway::start(socket);
    let granted = granted(&page).await;
    assert_eq!(granted["port"], gateway.port);
    let connection = timeout(
        PATIENCE,
        connect(gateway.port, granted["certificateSha256"].as_str().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let mut control = Control::open(&connection).await;
    control
        .say(&PageControl::Hello {
            protocol: PROTOCOL,
            token: granted["token"].as_str().unwrap().to_owned(),
        })
        .await;
    assert!(matches!(
        control.hear().await,
        GatewayControl::Welcome {
            protocol: PROTOCOL,
            ..
        }
    ));
    control
        .say(&PageControl::Ping {
            id: 5,
            page_time: 2.5,
        })
        .await;
    assert!(matches!(
        control.hear().await,
        GatewayControl::Pong { id: 5, .. }
    ));

    page.call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(
        control.hear().await,
        GatewayControl::Closing {
            reason: Closing::Session
        }
    );
    timeout(PATIENCE, connection.closed()).await.unwrap();
    gateway.stop().await;
}

/// The switch is enforced twice. Off, the gateway's unit does not run, so
/// nothing answers on its port, whatever the caller holds. And the server,
/// which has lost the link, issues no token and honours none it issued
/// before, even when the gateway is switched on again.
#[tokio::test]
async fn the_switch_is_enforced_twice() {
    use bli_kvm_quic_protocol::{
        PROTOCOL,
        page::{GatewayControl, PageControl, Refusal},
    };
    let (_root, socket, page) = board(|_| {}).await;
    let gateway = RealGateway::start(socket.clone());
    let before = granted(&page).await;
    let port = gateway.port;
    let certificate = before["certificateSha256"].as_str().unwrap().to_owned();
    let waiting = before["token"].as_str().unwrap().to_owned();
    // On, the port answers a page that has the certificate's hash.
    timeout(PATIENCE, connect(port, &certificate))
        .await
        .unwrap()
        .unwrap();

    // Off: the owner stops the unit.
    gateway.stop().await;

    // Once: the same page, with the same hash, gets no answer.
    let answered = timeout(Duration::from_millis(1500), connect(port, &certificate)).await;
    assert!(!matches!(answered, Ok(Ok(_))), "the port still answers");
    // Twice: the server issues no token.
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        let answer = page.session(now_ms()).await;
        if answer["granted"] == false {
            assert_eq!(answer["reason"], "unavailable");
            assert!(answer.get("token").is_none(), "{answer}");
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{answer}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // On again: a token from before the switch went off opens nothing.
    let again = RealGateway::start(socket);
    let after = granted(&page).await;
    assert_ne!(after["certificateSha256"], certificate.as_str());
    assert_ne!(after["token"], waiting.as_str());
    let connection = timeout(
        PATIENCE,
        connect(again.port, after["certificateSha256"].as_str().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    let mut control = Control::open(&connection).await;
    control
        .say(&PageControl::Hello {
            protocol: PROTOCOL,
            token: waiting,
        })
        .await;
    assert_eq!(
        control.hear().await,
        GatewayControl::Refused {
            reason: Refusal::Token
        }
    );
    again.stop().await;
}
