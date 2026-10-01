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
#[cfg(feature = "restricted-control-surface")]
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

/// The real gateway on the real link: a page gets a token over the API, opens
/// the WebTransport session with the certificate's hash, is welcomed, has a
/// ping answered, and is told when its web session ends.
#[tokio::test]
async fn a_page_opens_a_session_through_the_real_gateway_and_a_logout_ends_it() {
    use bli_kvm_quic_protocol::{
        PROTOCOL,
        page::{self, Closing, GatewayControl, MessageReader, PageControl},
    };
    let (_root, socket, page) = board(|_| {}).await;
    let gateway = bli_kvm_quic_gateway::Gateway::bind(bli_kvm_quic_gateway::config::Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        link_socket: socket,
        counters_interval: Duration::from_millis(200),
        ..Default::default()
    })
    .unwrap();
    let port = gateway.local_addr().unwrap().port();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    tokio::spawn(gateway.serve(async {
        let _ = stopped.await;
    }));

    // The gateway reaches the server by itself.
    let deadline = std::time::Instant::now() + PATIENCE;
    let granted = loop {
        let answer = page.session(now_ms()).await;
        if answer["granted"] == true {
            break answer;
        }
        assert!(std::time::Instant::now() < deadline, "{answer}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(granted["port"], port);
    let certificate = granted["certificateSha256"].as_str().unwrap();
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
    .unwrap();
    let connection = timeout(
        PATIENCE,
        client.connect(
            wtransport::endpoint::ConnectOptions::builder(format!("https://127.0.0.1:{port}/kvm"))
                .add_header("origin", format!("https://127.0.0.1:{port}"))
                .build(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let (mut send, mut receive) = connection.open_bi().await.unwrap().await.unwrap();
    let mut reader = MessageReader::new();
    let hear = async |receive: &mut wtransport::RecvStream, reader: &mut MessageReader| {
        let mut buffer = [0u8; 2048];
        loop {
            match reader.take::<GatewayControl>().unwrap() {
                Some(GatewayControl::Counters { .. }) => {}
                Some(message) => return message,
                None => {
                    let count = timeout(PATIENCE, receive.read(&mut buffer))
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap();
                    reader.push(&buffer[..count]);
                }
            }
        }
    };
    let hello = PageControl::Hello {
        protocol: PROTOCOL,
        token: granted["token"].as_str().unwrap().to_owned(),
    };
    send.write_all(&page::encode(&hello).unwrap())
        .await
        .unwrap();
    assert!(matches!(
        hear(&mut receive, &mut reader).await,
        GatewayControl::Welcome {
            protocol: PROTOCOL,
            ..
        }
    ));
    send.write_all(
        &page::encode(&PageControl::Ping {
            id: 5,
            page_time: 2.5,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    assert!(matches!(
        hear(&mut receive, &mut reader).await,
        GatewayControl::Pong { id: 5, .. }
    ));

    page.call(Method::POST, "/api/auth/logout", Value::Null)
        .await;
    assert_eq!(
        hear(&mut receive, &mut reader).await,
        GatewayControl::Closing {
            reason: Closing::Session
        }
    );
    timeout(PATIENCE, connection.closed()).await.unwrap();
    let _ = stop.send(());
}
