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
    page::{self, Closing, GatewayControl, MessageReader, PageControl, Refusal},
};
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
                                GatewayToServer::Lease { .. } | GatewayToServer::Video { .. } => None,
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
