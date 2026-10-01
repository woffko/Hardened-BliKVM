//! The gateway against a page (wtransport's client, which checks the
//! certificate's hash the way a browser does) and a web server (a Unix socket
//! that speaks the link's messages).

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use bli_kvm_quic_gateway::{Gateway, config::Config};
use bli_kvm_quic_protocol::{
    PROTOCOL,
    link::{self, EndReason, GatewayToServer, RedeemRefusal, ServerToGateway},
    page::{
        self, Closing, GatewayControl, InputRequest, InputResponse, MessageReader, PageControl,
        Refusal,
    },
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixListener,
    sync::{mpsc, oneshot},
    time::timeout,
};
use wtransport::{
    ClientConfig, Connection, Endpoint, RecvStream, SendStream, endpoint::ConnectOptions,
    tls::Sha256Digest,
};

const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq)]
struct Hello {
    certificate: String,
    port: u16,
    not_before: u64,
    not_after: u64,
}

enum Command {
    End(String, EndReason),
    /// Close the link's connection; the next one is accepted again.
    Drop,
}

#[derive(Default)]
struct Seen {
    hellos: Vec<Hello>,
    redeemed: Vec<String>,
    closed: Vec<String>,
    tokens: HashSet<String>,
    /// Tokens answered with "too many sessions".
    crowded: HashSet<String>,
    /// Connections of the link that the gateway closed.
    left: usize,
    /// The lease requests, with the session each came from.
    leases: Vec<(String, Value)>,
    /// How long the server takes over a lease request.
    lease_delay: Duration,
    /// The server does not answer lease requests at all.
    lease_silent: bool,
}

/// The web server's side of the link.
struct Server {
    seen: Arc<Mutex<Seen>>,
    commands: mpsc::UnboundedSender<Command>,
}

impl Server {
    fn start(path: PathBuf, protocol: u16) -> Self {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let (commands, mut receiver) = mpsc::unbounded_channel::<Command>();
        let listener = UnixListener::bind(&path).unwrap();
        let state = Arc::clone(&seen);
        tokio::spawn(async move {
            let mut sessions = 0u32;
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let (mut reader, mut writer) = stream.into_split();
                let reply = link::encode(&ServerToGateway::Hello { protocol }).unwrap();
                let mut greeted = false;
                loop {
                    let mut prefix = [0u8; 4];
                    tokio::select! {
                        read = reader.read_exact(&mut prefix) => {
                            if read.is_err() {
                                state.lock().unwrap().left += 1;
                                break;
                            }
                            let length = link::body_length(prefix).unwrap();
                            let mut body = vec![0u8; length];
                            if reader.read_exact(&mut body).await.is_err() {
                                break;
                            }
                            let answer = match link::decode::<GatewayToServer>(&body).unwrap() {
                                GatewayToServer::Hello { protocol: theirs, certificate_sha256, not_before, not_after, port } => {
                                    assert_eq!(theirs, PROTOCOL);
                                    state.lock().unwrap().hellos.push(Hello {
                                        certificate: certificate_sha256,
                                        port,
                                        not_before,
                                        not_after,
                                    });
                                    // Only the first hello of a connection is answered.
                                    (!std::mem::replace(&mut greeted, true)).then(|| reply.clone())
                                }
                                GatewayToServer::Redeem { id, token, .. } => {
                                    let mut state = state.lock().unwrap();
                                    state.redeemed.push(token.clone());
                                    let message = if state.crowded.contains(&token) {
                                        ServerToGateway::Redeemed { id, session: None, refused: Some(RedeemRefusal::Sessions) }
                                    } else if state.tokens.remove(&token) {
                                        sessions += 1;
                                        ServerToGateway::Redeemed { id, session: Some(format!("session-{sessions}")), refused: None }
                                    } else {
                                        ServerToGateway::Redeemed { id, session: None, refused: Some(RedeemRefusal::Token) }
                                    };
                                    Some(link::encode(&message).unwrap())
                                }
                                GatewayToServer::Closed { session } => {
                                    state.lock().unwrap().closed.push(session);
                                    None
                                }
                                GatewayToServer::Lease { id, session, request } => {
                                    // One at a time, like the server: its answer names the
                                    // request it is for.
                                    let (delay, silent) = {
                                        let mut state = state.lock().unwrap();
                                        state.leases.push((session, request.clone()));
                                        (state.lease_delay, state.lease_silent)
                                    };
                                    tokio::time::sleep(delay).await;
                                    (!silent).then(|| {
                                        let response = json!({
                                            "status": 200,
                                            "body": { "answerTo": request["operationId"] },
                                        });
                                        link::encode(&ServerToGateway::LeaseAnswer { id, response }).unwrap()
                                    })
                                }
                                GatewayToServer::Video { .. } => None,
                            };
                            if let Some(bytes) = answer
                                && writer.write_all(&bytes).await.is_err()
                            {
                                break;
                            }
                        }
                        command = receiver.recv() => match command {
                            Some(Command::End(session, reason)) => {
                                let bytes = link::encode(&ServerToGateway::End { session, reason }).unwrap();
                                if writer.write_all(&bytes).await.is_err() {
                                    break;
                                }
                            }
                            Some(Command::Drop) => break,
                            None => return,
                        },
                    }
                }
            }
        });
        Self { seen, commands }
    }

    fn token(&self, value: &str) -> String {
        let token = value.repeat(64 / value.len());
        self.seen.lock().unwrap().tokens.insert(token.clone());
        token
    }

    async fn hello(&self, index: usize) -> Hello {
        eventually(|| self.seen.lock().unwrap().hellos.get(index).cloned()).await
    }
}

/// Waits for `check` to give a value.
async fn eventually<T>(mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "the awaited state did not come");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

struct Board {
    server: Server,
    port: u16,
    stop: Option<oneshot::Sender<()>>,
    _directory: tempfile::TempDir,
}

impl Drop for Board {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Board {
    async fn start(change: impl FnOnce(&mut Config), link_protocol: u16) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("link.sock");
        let server = Server::start(path.clone(), link_protocol);
        let mut config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            link_socket: path,
            hello_timeout: Duration::from_millis(700),
            counters_interval: Duration::from_millis(100),
            ..Config::default()
        };
        change(&mut config);
        let gateway = Gateway::bind(config).unwrap();
        let port = gateway.local_addr().unwrap().port();
        let (stop, stopped) = oneshot::channel::<()>();
        tokio::spawn(gateway.serve(async {
            let _ = stopped.await;
        }));
        Self {
            server,
            port,
            stop: Some(stop),
            _directory: directory,
        }
    }

    async fn ready() -> Self {
        let board = Self::start(|_| {}, PROTOCOL).await;
        let hello = board.server.hello(0).await;
        assert_eq!(hello.port, board.port);
        board
    }

    fn origin(&self) -> String {
        format!("https://127.0.0.1:{}", self.port)
    }

    /// A page of `origin` asks for `path` with the certificate's hash.
    async fn request(
        &self,
        certificate: &str,
        origin: Option<&str>,
        path: &str,
    ) -> Result<Connection, String> {
        let mut digest = [0u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&certificate[index * 2..index * 2 + 2], 16).unwrap();
        }
        let config = ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([Sha256Digest::new(digest)])
            .build();
        let endpoint = Endpoint::client(config).unwrap();
        let mut options = ConnectOptions::builder(format!("https://127.0.0.1:{}{path}", self.port));
        if let Some(origin) = origin {
            options = options.add_header("origin", origin);
        }
        match timeout(PATIENCE, endpoint.connect(options.build())).await {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => Err(error.to_string()),
            Err(_) => Err("no answer".into()),
        }
    }

    /// The board's own page opens a session and says hello.
    async fn page(&self, certificate: &str, hello: &PageControl) -> Page {
        let connection = self
            .request(certificate, Some(&self.origin()), "/kvm")
            .await
            .unwrap();
        let (send, receive) = connection.open_bi().await.unwrap().await.unwrap();
        let mut page = Page {
            connection,
            send,
            receive,
            reader: MessageReader::new(),
        };
        page.say(hello).await;
        page
    }

    async fn welcomed(&self, token: &str) -> Page {
        let certificate = self.server.hello(0).await.certificate;
        let mut page = self
            .page(
                &certificate,
                &PageControl::Hello {
                    protocol: PROTOCOL,
                    token: token.to_string(),
                },
            )
            .await;
        match page.hear().await {
            Some(GatewayControl::Welcome {
                protocol,
                max_datagram,
            }) => {
                assert_eq!(protocol, PROTOCOL);
                assert!((1..=1200).contains(&max_datagram), "{max_datagram}");
            }
            other => panic!("no welcome: {other:?}"),
        }
        page
    }
}

struct Page {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    reader: MessageReader,
}

impl Page {
    async fn say(&mut self, message: &PageControl) {
        self.send
            .write_all(&page::encode(message).unwrap())
            .await
            .unwrap();
    }

    /// The next message, or `None` once the stream has ended.
    async fn hear(&mut self) -> Option<GatewayControl> {
        let mut buffer = [0u8; 2048];
        loop {
            if let Some(message) = self.reader.take::<GatewayControl>().unwrap() {
                return Some(message);
            }
            let count = timeout(PATIENCE, self.receive.read(&mut buffer))
                .await
                .expect("the gateway said nothing")
                .ok()??;
            self.reader.push(&buffer[..count]);
        }
    }

    /// The next message that is not the periodic counters.
    async fn hear_but_counters(&mut self) -> Option<GatewayControl> {
        loop {
            match self.hear().await {
                Some(GatewayControl::Counters { .. }) => {}
                other => return other,
            }
        }
    }

    async fn gone(&self) {
        timeout(PATIENCE, self.connection.closed())
            .await
            .expect("the connection stayed open");
    }
}

#[tokio::test]
async fn a_page_with_a_token_is_welcomed_and_its_pings_are_answered() {
    let board = Board::ready().await;
    let hello = board.server.hello(0).await;
    assert_eq!(hello.certificate.len(), 64);
    assert_eq!(hello.not_after - hello.not_before, 13 * 24 * 3600);

    let token = board.server.token("0a");
    let mut page = board.welcomed(&token).await;
    assert_eq!(board.server.seen.lock().unwrap().redeemed, vec![token]);

    let mut last = 0;
    for id in 1..=3u32 {
        page.say(&PageControl::Ping {
            id,
            page_time: 1000.5 + f64::from(id),
        })
        .await;
        match page.hear_but_counters().await {
            Some(GatewayControl::Pong {
                id: answered,
                page_time,
                board_time,
            }) => {
                assert_eq!(answered, id);
                assert_eq!(page_time, 1000.5 + f64::from(id));
                assert!(board_time >= last);
                last = board_time;
            }
            other => panic!("no pong: {other:?}"),
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(last > 0, "the board's clock moves");

    // The counters come by themselves.
    loop {
        if let Some(GatewayControl::Counters {
            round_trip_micros, ..
        }) = page.hear().await
        {
            assert!(round_trip_micros < 1_000_000);
            break;
        }
    }
    // The picture is not served in this phase.
    page.say(&PageControl::Video { subscribe: true }).await;
    assert!(matches!(
        page.hear_but_counters().await,
        Some(GatewayControl::Video {
            state: page::VideoState::Unavailable,
            ..
        })
    ));

    page.say(&PageControl::Bye {}).await;
    page.gone().await;
    let closed = eventually(|| {
        let seen = board.server.seen.lock().unwrap();
        (!seen.closed.is_empty()).then(|| seen.closed.clone())
    })
    .await;
    assert_eq!(closed, vec!["session-1".to_string()]);
}

#[tokio::test]
async fn a_token_is_redeemed_once_and_a_crowded_session_is_told_so() {
    let board = Board::ready().await;
    let certificate = board.server.hello(0).await.certificate;
    let token = board.server.token("0b");
    let _first = board.welcomed(&token).await;

    let hello = PageControl::Hello {
        protocol: PROTOCOL,
        token: token.clone(),
    };
    let mut second = board.page(&certificate, &hello).await;
    assert_eq!(
        second.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Token
        })
    );
    second.gone().await;

    let crowded = "0c".repeat(32);
    board
        .server
        .seen
        .lock()
        .unwrap()
        .crowded
        .insert(crowded.clone());
    let hello = PageControl::Hello {
        protocol: PROTOCOL,
        token: crowded,
    };
    let mut third = board.page(&certificate, &hello).await;
    assert_eq!(
        third.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Sessions
        })
    );
    third.gone().await;
}

#[tokio::test]
async fn what_is_not_a_proper_hello_is_refused_without_asking_the_server() {
    let board = Board::ready().await;
    let certificate = board.server.hello(0).await.certificate;

    let wrong_protocol = PageControl::Hello {
        protocol: PROTOCOL + 1,
        token: "0d".repeat(32),
    };
    let mut page = board.page(&certificate, &wrong_protocol).await;
    assert_eq!(
        page.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Protocol
        })
    );
    page.gone().await;

    let malformed = PageControl::Hello {
        protocol: PROTOCOL,
        token: "not a token".into(),
    };
    let mut page = board.page(&certificate, &malformed).await;
    assert_eq!(
        page.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Token
        })
    );
    page.gone().await;

    let mut page = board
        .page(
            &certificate,
            &PageControl::Ping {
                id: 1,
                page_time: 0.0,
            },
        )
        .await;
    assert_eq!(
        page.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Hello
        })
    );
    page.gone().await;

    // Bytes that are not a message at all.
    let connection = board
        .request(&certificate, Some(&board.origin()), "/kvm")
        .await
        .unwrap();
    let (mut send, receive) = connection.open_bi().await.unwrap().await.unwrap();
    send.write_all(&[0, 0, 1, 2, 3]).await.unwrap();
    let mut page = Page {
        connection,
        send,
        receive,
        reader: MessageReader::new(),
    };
    assert_eq!(
        page.hear().await,
        Some(GatewayControl::Refused {
            reason: Refusal::Hello
        })
    );
    page.gone().await;

    assert!(board.server.seen.lock().unwrap().redeemed.is_empty());
}

#[tokio::test]
async fn a_page_that_does_not_say_hello_in_time_is_dropped() {
    let board = Board::ready().await;
    let certificate = board.server.hello(0).await.certificate;
    let started = Instant::now();
    let connection = board
        .request(&certificate, Some(&board.origin()), "/kvm")
        .await
        .unwrap();
    // The stream is opened but nothing is said on it.
    let _stream = connection.open_bi().await.unwrap().await.unwrap();
    timeout(PATIENCE, connection.closed())
        .await
        .expect("the connection stayed open");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(600),
        "closed after {waited:?}"
    );
    assert!(waited < Duration::from_secs(3), "closed after {waited:?}");
    assert!(board.server.seen.lock().unwrap().redeemed.is_empty());
}

#[tokio::test]
async fn a_foreign_page_and_another_path_are_not_admitted() {
    let board = Board::ready().await;
    let certificate = board.server.hello(0).await.certificate;
    let own = board.origin();
    assert!(
        board
            .request(&certificate, Some("https://evil.example"), "/kvm")
            .await
            .is_err()
    );
    assert!(board.request(&certificate, None, "/kvm").await.is_err());
    assert!(board.request(&certificate, Some(&own), "/").await.is_err());
    assert!(
        board
            .request(&certificate, Some(&own), "/kvm/more")
            .await
            .is_err()
    );
    // A certificate with another hash is not the board's.
    let other = "00".repeat(32);
    assert!(board.request(&other, Some(&own), "/kvm").await.is_err());
    assert!(
        board
            .request(&certificate, Some(&own), "/kvm")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn the_server_ends_a_session() {
    let board = Board::ready().await;
    let mut page = board.welcomed(&board.server.token("0e")).await;
    board
        .server
        .commands
        .send(Command::End("session-1".into(), EndReason::Session))
        .unwrap();
    assert_eq!(
        page.hear_but_counters().await,
        Some(GatewayControl::Closing {
            reason: Closing::Session
        })
    );
    page.gone().await;
    // The server ended it; it is not told again.
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(board.server.seen.lock().unwrap().closed.is_empty());
}

#[tokio::test]
async fn a_lost_link_ends_every_session_and_nobody_is_served_until_it_is_back() {
    let board = Board::ready().await;
    let certificate = board.server.hello(0).await.certificate;
    let mut first = board.welcomed(&board.server.token("1a")).await;
    let mut second = board.welcomed(&board.server.token("1b")).await;

    board.server.commands.send(Command::Drop).unwrap();
    for page in [&mut first, &mut second] {
        assert_eq!(
            page.hear_but_counters().await,
            Some(GatewayControl::Closing {
                reason: Closing::Shutdown
            })
        );
        page.gone().await;
    }

    // The gateway comes back to the server by itself and says hello again.
    let again = board.server.hello(1).await;
    assert_eq!(again.certificate, certificate);
    let token = board.server.token("1c");
    let page = loop {
        // A connection attempt made before the link is up again is refused.
        match board
            .request(&certificate, Some(&board.origin()), "/kvm")
            .await
        {
            Ok(connection) => break connection,
            Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    };
    let (mut send, receive) = page.open_bi().await.unwrap().await.unwrap();
    send.write_all(
        &page::encode(&PageControl::Hello {
            protocol: PROTOCOL,
            token,
        })
        .unwrap(),
    )
    .await
    .unwrap();
    let mut page = Page {
        connection: page,
        send,
        receive,
        reader: MessageReader::new(),
    };
    assert!(matches!(
        page.hear().await,
        Some(GatewayControl::Welcome { .. })
    ));
}

/// A gateway that stops leaves the link before its connections have drained:
/// the server must not issue tokens for an endpoint that no longer answers.
#[tokio::test]
async fn a_gateway_that_stops_leaves_the_link_first() {
    let mut board = Board::ready().await;
    let token = board.server.token("5a");
    // A session that does not close by itself keeps the endpoint busy.
    let page = board.welcomed(&token).await;
    assert_eq!(board.server.seen.lock().unwrap().left, 0);
    let stopping = Instant::now();
    board.stop.take().unwrap().send(()).unwrap();
    eventually(|| (board.server.seen.lock().unwrap().left == 1).then_some(())).await;
    assert!(
        stopping.elapsed() < Duration::from_millis(500),
        "the link stayed up for {:?}",
        stopping.elapsed()
    );
    page.gone().await;
    // It does not come back to the link.
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert_eq!(board.server.seen.lock().unwrap().hellos.len(), 1);
}

#[tokio::test]
async fn nobody_is_served_while_the_link_is_down() {
    // No server listens on the link's path.
    let directory = tempfile::tempdir().unwrap();
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        link_socket: directory.path().join("absent.sock"),
        ..Config::default()
    };
    let gateway = Gateway::bind(config).unwrap();
    let port = gateway.local_addr().unwrap().port();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(gateway.serve(async {
        let _ = stopped.await;
    }));
    let client = Endpoint::client(
        ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([Sha256Digest::new([0u8; 32])])
            .build(),
    )
    .unwrap();
    let attempt = timeout(
        PATIENCE,
        client.connect(format!("https://127.0.0.1:{port}/kvm")),
    )
    .await;
    assert!(
        matches!(attempt, Ok(Err(_))),
        "the attempt must be refused, not left waiting"
    );
    let _ = stop.send(());
}

#[tokio::test]
async fn a_server_with_another_link_protocol_is_not_served() {
    let board = Board::start(|_| {}, PROTOCOL + 1).await;
    let certificate = board.server.hello(0).await.certificate;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        board
            .request(&certificate, Some(&board.origin()), "/kvm")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn connections_are_bounded() {
    let board = Board::start(|config| config.max_connections = 2, PROTOCOL).await;
    let certificate = board.server.hello(0).await.certificate;
    let first = board.welcomed(&board.server.token("2a")).await;
    let _second = board.welcomed(&board.server.token("2b")).await;
    assert!(
        board
            .request(&certificate, Some(&board.origin()), "/kvm")
            .await
            .is_err()
    );
    // A place that is given up can be taken again.
    drop(first);
    let token = board.server.token("2c");
    let deadline = Instant::now() + PATIENCE;
    loop {
        if board
            .request(&certificate, Some(&board.origin()), "/kvm")
            .await
            .is_ok()
        {
            break;
        }
        assert!(Instant::now() < deadline, "the place was not given back");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = token;
}

#[tokio::test]
async fn the_certificate_is_replaced_and_open_sessions_go_on() {
    let board = Board::start(
        |config| config.certificate_use = Duration::from_millis(900),
        PROTOCOL,
    )
    .await;
    let old = board.server.hello(0).await;
    let mut page = board.welcomed(&board.server.token("3a")).await;

    let new = board.server.hello(1).await;
    assert_ne!(new.certificate, old.certificate);
    assert_eq!(new.port, old.port);

    // The open session still answers.
    page.say(&PageControl::Ping {
        id: 7,
        page_time: 1.0,
    })
    .await;
    assert!(matches!(
        page.hear_but_counters().await,
        Some(GatewayControl::Pong { id: 7, .. })
    ));
    // A new page needs the new hash.
    let own = board.origin();
    assert!(
        board
            .request(&old.certificate, Some(&own), "/kvm")
            .await
            .is_err()
    );
    assert!(
        board
            .request(&new.certificate, Some(&own), "/kvm")
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn the_ipv6_wildcard_also_serves_ipv4_pages() {
    // Bound dual-stack, or to IPv4 alone where the host has no IPv6: a page
    // that reaches the board over IPv4 is served either way.
    let board = Board::start(|config| config.bind = "[::]:0".parse().unwrap(), PROTOCOL).await;
    let _page = board.welcomed(&board.server.token("4a")).await;
}

/// A page's input stream: the stream it opens after the control stream.
struct Input {
    send: SendStream,
    receive: RecvStream,
    reader: MessageReader,
}

impl Input {
    async fn open(page: &Page) -> Self {
        let (send, receive) = page.connection.open_bi().await.unwrap().await.unwrap();
        Self {
            send,
            receive,
            reader: MessageReader::new(),
        }
    }

    /// Writes the requests in one piece, the way a page that does not wait
    /// for answers would.
    async fn say(&mut self, requests: &[(u32, Value)]) {
        let mut bytes = Vec::new();
        for (seq, request) in requests {
            bytes.extend(
                page::encode(&InputRequest {
                    seq: *seq,
                    request: request.clone(),
                })
                .unwrap(),
            );
        }
        self.send.write_all(&bytes).await.unwrap();
    }

    async fn hear(&mut self) -> Option<InputResponse> {
        let mut buffer = [0u8; 2048];
        loop {
            if let Some(message) = self.reader.take::<InputResponse>().unwrap() {
                return Some(message);
            }
            let count = timeout(PATIENCE, self.receive.read(&mut buffer))
                .await
                .expect("the gateway did not answer")
                .ok()??;
            self.reader.push(&buffer[..count]);
        }
    }
}

fn operation(number: u32) -> String {
    format!("{number:032x}")
}

fn key(number: u32, code: u8) -> Value {
    json!({
        "action": "report",
        "operationId": operation(number),
        "leaseId": "5a".repeat(16),
        "report": { "mode": "keyboard", "data": [0, 0, code, 0, 0, 0, 0, 0] },
    })
}

fn position(number: u32, x: u16) -> Value {
    json!({
        "action": "report",
        "operationId": operation(number),
        "leaseId": "5a".repeat(16),
        "report": { "mode": "absolute_mouse", "data": [0, x & 0xff, x >> 8, 7, 0, 0] },
    })
}

/// What the test server answers to a request.
fn answer_to(number: u32) -> Value {
    json!({ "status": 200, "body": { "answerTo": operation(number) } })
}

/// Requests of the lease API go to the server in the order the page sent
/// them, for the page's session, and each comes back with the server's own
/// answer under the page's number. No keyboard report is lost or merged.
#[tokio::test]
async fn input_requests_reach_the_server_in_order_and_are_answered() {
    let board = Board::ready().await;
    let page = board.welcomed(&board.server.token("1b")).await;
    let mut input = Input::open(&page).await;
    // Sixty key reports at once, more than wait at a time.
    let requests: Vec<(u32, Value)> = (0..60)
        .map(|index| (1000 + index, key(index, 4 + (index % 3) as u8)))
        .collect();
    input.say(&requests).await;
    for (seq, _) in &requests {
        assert_eq!(
            input.hear().await,
            Some(InputResponse {
                seq: *seq,
                response: answer_to(seq - 1000),
            })
        );
    }
    let seen = board.server.seen.lock().unwrap();
    assert_eq!(seen.leases.len(), 60);
    for (index, (session, request)) in seen.leases.iter().enumerate() {
        assert_eq!(session, "session-1");
        assert_eq!(request, &requests[index].1);
    }
}

/// While the server is busy, positions of the pointer that a newer one
/// supersedes are not sent: the newest goes, and its answer answers them
/// all. Keys around them keep their place.
#[tokio::test]
async fn a_busy_server_gets_the_newest_position_only() {
    let board = Board::ready().await;
    board.server.seen.lock().unwrap().lease_delay = Duration::from_millis(150);
    let page = board.welcomed(&board.server.token("2c")).await;
    let mut input = Input::open(&page).await;
    let mut requests = vec![(1, key(1, 4))];
    requests.extend((0..20).map(|index| (10 + index, position(10 + index, 100 + index as u16))));
    requests.push((50, key(50, 5)));
    input.say(&requests).await;

    assert_eq!(
        input.hear().await.unwrap(),
        InputResponse {
            seq: 1,
            response: answer_to(1)
        }
    );
    // Twenty answers, all the newest position's.
    for index in 0..20 {
        assert_eq!(
            input.hear().await.unwrap(),
            InputResponse {
                seq: 10 + index,
                response: answer_to(29)
            }
        );
    }
    assert_eq!(
        input.hear().await.unwrap(),
        InputResponse {
            seq: 50,
            response: answer_to(50)
        }
    );
    let seen = board.server.seen.lock().unwrap();
    assert_eq!(
        seen.leases
            .iter()
            .map(|(_, request)| request.clone())
            .collect::<Vec<_>>(),
        vec![key(1, 4), position(29, 119), key(50, 5)]
    );
}

/// What a page wrote before it closed its input stream is still passed on,
/// and a page may open the stream again.
#[tokio::test]
async fn a_closed_input_stream_is_served_to_its_end_and_can_be_opened_again() {
    let board = Board::ready().await;
    board.server.seen.lock().unwrap().lease_delay = Duration::from_millis(50);
    let page = board.welcomed(&board.server.token("3d")).await;
    let mut input = Input::open(&page).await;
    let release =
        json!({ "action": "release", "operationId": operation(3), "leaseId": "5a".repeat(16) });
    input
        .say(&[(1, key(1, 4)), (2, key(2, 5)), (3, release.clone())])
        .await;
    input.send.finish().await.unwrap();
    eventually(|| (board.server.seen.lock().unwrap().leases.len() == 3).then_some(())).await;
    assert_eq!(board.server.seen.lock().unwrap().leases[2].1, release);
    for seq in 1..=3 {
        assert_eq!(input.hear().await.unwrap().seq, seq);
    }

    let mut again = Input::open(&page).await;
    again.say(&[(7, key(7, 6))]).await;
    assert_eq!(
        again.hear().await.unwrap(),
        InputResponse {
            seq: 7,
            response: answer_to(7)
        }
    );
    assert_eq!(board.server.seen.lock().unwrap().leases.len(), 4);
}

/// What is not a request ends the session, with the reason; nothing of it
/// reaches the server.
#[tokio::test]
async fn what_is_not_an_input_request_ends_the_session() {
    for garbage in [
        // A message of no bytes, one that is too long, and one that is no request.
        vec![0u8, 0],
        vec![0xff, 0xff, 1, 2, 3],
        page::encode(&json!({ "seq": 1, "request": {}, "more": true })).unwrap(),
        page::encode(&json!({ "seq": "one", "request": {} })).unwrap(),
    ] {
        let board = Board::ready().await;
        let mut page = board.welcomed(&board.server.token("4e")).await;
        let mut input = Input::open(&page).await;
        input.send.write_all(&garbage).await.unwrap();
        assert_eq!(
            page.hear_but_counters().await,
            Some(GatewayControl::Closing {
                reason: Closing::Protocol
            }),
            "{garbage:?}"
        );
        page.gone().await;
        assert!(board.server.seen.lock().unwrap().leases.is_empty());
    }
}

/// A server that does not answer costs the page one request's patience: it
/// is told the service is unavailable and the stream goes on.
#[tokio::test]
async fn a_request_the_server_does_not_answer_is_answered_as_unavailable() {
    let board = Board::ready().await;
    board.server.seen.lock().unwrap().lease_silent = true;
    let page = board.welcomed(&board.server.token("5f")).await;
    let mut input = Input::open(&page).await;
    input.say(&[(1, key(1, 4))]).await;
    let started = Instant::now();
    assert_eq!(
        input.hear().await.unwrap(),
        InputResponse {
            seq: 1,
            response: json!({ "status": 503, "body": { "code": -503, "msg": "service unavailable" } }),
        }
    );
    assert!(
        started.elapsed() >= Duration::from_millis(2900),
        "{:?}",
        started.elapsed()
    );
    board.server.seen.lock().unwrap().lease_silent = false;
    input.say(&[(2, key(2, 5))]).await;
    assert_eq!(
        input.hear().await.unwrap(),
        InputResponse {
            seq: 2,
            response: answer_to(2)
        }
    );
}
