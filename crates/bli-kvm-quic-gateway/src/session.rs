//! One page's session: from its first packet to its end.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use tokio::sync::mpsc;

use bli_kvm_quic_protocol::{
    PROTOCOL,
    frame::MAX_DATAGRAM_BYTES,
    link::EndReason,
    page::{
        self, Closing, GatewayControl, MessageReader, PageControl, Refusal, SESSION_PATH,
        VideoState, token_is_well_formed,
    },
};
use tokio::time::timeout;
use wtransport::{Connection, RecvStream, SendStream, VarInt, endpoint::IncomingSession};

use crate::{
    clock::monotonic_micros,
    config::Config,
    frames::FrameSource,
    input::{self, Ended},
    link::Link,
    video::{self, VideoControl, VideoCounters},
};

/// How long a refused or ended page gets to read why.
const FAREWELL: Duration = Duration::from_millis(500);
const READ_BUFFER: usize = 4096;

/// Counts a connection from its first packet until its task ends.
pub struct Slot(Arc<AtomicUsize>);

impl Slot {
    /// A place among `limit` connections, or `None` when all are taken.
    pub fn take(counter: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        let mut current = counter.load(Ordering::Acquire);
        loop {
            if current >= limit {
                return None;
            }
            match counter.compare_exchange(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(Self(Arc::clone(counter))),
                Err(seen) => current = seen,
            }
        }
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The page that opened the session must be the board's own: its origin's
/// host and port are the ones the session was requested for. The scheme is
/// not compared: a browser offers WebTransport to secure pages only.
pub fn origin_is_the_boards(origin: Option<&str>, authority: &str) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    let Some((scheme, rest)) = origin.split_once("://") else {
        return false;
    };
    let default_port = match scheme {
        "https" => 443,
        "http" => 80,
        _ => return false,
    };
    if rest.contains(['/', '?', '#', '@']) {
        return false;
    }
    match (
        host_and_port(rest, default_port),
        host_and_port(authority, 443),
    ) {
        (Some(page), Some(board)) => page == board,
        _ => false,
    }
}

/// `host`, `host:port` or `[v6]:port`, with the host in lower case.
fn host_and_port(value: &str, default_port: u16) -> Option<(String, u16)> {
    let (host, port) = if let Some(rest) = value.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        match after.strip_prefix(':') {
            Some(port) => (host, port.parse().ok()?),
            None if after.is_empty() => (host, default_port),
            None => return None,
        }
    } else {
        match value.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, port.parse().ok()?),
            Some(_) => return None,
            None => (value, default_port),
        }
    };
    if host.is_empty() {
        return None;
    }
    Some((host.to_ascii_lowercase(), port))
}

/// The session's messages on its control stream.
struct Control {
    send: SendStream,
    receive: RecvStream,
    reader: MessageReader,
    buffer: Box<[u8]>,
}

impl Control {
    /// The next message, `None` when the page closed the stream or sent what
    /// is not a message.
    async fn read(&mut self) -> Option<PageControl> {
        loop {
            match self.reader.take::<PageControl>() {
                Ok(Some(message)) => return Some(message),
                Ok(None) => {}
                Err(_) => return None,
            }
            let count = self.receive.read(&mut self.buffer).await.ok()??;
            self.reader.push(&self.buffer[..count]);
        }
    }

    async fn write(&mut self, message: &GatewayControl) -> bool {
        match page::encode(message) {
            Ok(bytes) => self.send.write_all(&bytes).await.is_ok(),
            Err(_) => false,
        }
    }
}

/// Serves one incoming connection. `slot` keeps its place among the
/// connections until this returns.
pub async fn serve(
    incoming: IncomingSession,
    link: Link,
    frames: FrameSource,
    config: Arc<Config>,
    slot: Slot,
) {
    let _slot = slot;
    let peer = incoming.remote_address().to_string();
    let deadline = Instant::now() + config.hello_timeout;
    // Everything up to a redeemed token happens within the hello timeout.
    let opened = timeout(config.hello_timeout, open(incoming, &link, &peer)).await;
    let (connection, mut control, session, mut ended) = match opened {
        Ok(Some(open)) => open,
        Ok(None) => return,
        Err(_) => {
            eprintln!("quic gateway: {peer} did not present a token in time");
            return;
        }
    };
    debug_assert!(Instant::now() <= deadline + FAREWELL);
    eprintln!("quic gateway: session from {peer} opened");

    let welcome = GatewayControl::Welcome {
        protocol: PROTOCOL,
        max_datagram: largest_datagram(&connection),
    };
    let reason = if control.write(&welcome).await {
        // The page's other stream, served beside the control stream.
        let (violation, violations) = tokio::sync::mpsc::channel(1);
        let _input = AbortOnDrop(tokio::spawn(serve_input(
            connection.clone(),
            link.clone(),
            session.clone(),
            violation,
        )));
        let picture = Picture {
            link: &link,
            session: &session,
            frames: &frames,
        };
        run(
            &connection,
            &mut control,
            &mut ended,
            violations,
            picture,
            &config,
        )
        .await
    } else {
        None
    };
    if let Some(reason) = reason {
        farewell(
            &connection,
            &mut control,
            &GatewayControl::Closing { reason },
        )
        .await;
    }
    connection.close(VarInt::from_u32(0), b"");
    link.closed(&session);
    eprintln!("quic gateway: session from {peer} ended");
}

/// Stops a task when the session that started it ends.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Serves the session's input stream: the next stream the page opens after
/// its control stream. One is served at a time; a page that closed its
/// input stream may open another.
async fn serve_input(
    connection: Connection,
    link: Link,
    session: String,
    violation: tokio::sync::mpsc::Sender<()>,
) {
    while let Ok((send, receive)) = connection.accept_bi().await {
        if input::serve(send, receive, &link, &session).await == Ended::Protocol {
            let _ = violation.send(()).await;
            return;
        }
    }
}

type Opened = (
    Connection,
    Control,
    String,
    tokio::sync::mpsc::Receiver<EndReason>,
);

/// Accepts the session request, reads the hello and redeems its token.
async fn open(incoming: IncomingSession, link: &Link, peer: &str) -> Option<Opened> {
    let request = incoming.await.ok()?;
    if request.path() != SESSION_PATH {
        request.not_found().await;
        return None;
    }
    if !origin_is_the_boards(request.origin(), request.authority()) {
        eprintln!("quic gateway: {peer} refused: the page is not the board's");
        request.forbidden().await;
        return None;
    }
    let connection = request.accept().await.ok()?;
    let (send, receive) = connection.accept_bi().await.ok()?;
    let mut control = Control {
        send,
        receive,
        reader: MessageReader::new(),
        buffer: vec![0u8; READ_BUFFER].into_boxed_slice(),
    };
    let refusal = match control.read().await {
        Some(PageControl::Hello { protocol, .. }) if protocol != PROTOCOL => Refusal::Protocol,
        Some(PageControl::Hello { token, .. }) if token_is_well_formed(&token) => {
            match link.redeem(token, peer.to_string()).await {
                Ok((session, ended)) => return Some((connection, control, session, ended)),
                Err(refusal) => refusal,
            }
        }
        Some(PageControl::Hello { .. }) => Refusal::Token,
        _ => Refusal::Hello,
    };
    eprintln!("quic gateway: {peer} refused: {refusal:?}");
    farewell(
        &connection,
        &mut control,
        &GatewayControl::Refused { reason: refusal },
    )
    .await;
    connection.close(VarInt::from_u32(1), b"");
    None
}

/// The session's life after its welcome. The answer is what the page is told
/// when the gateway ends the session, or `None` when the page or its
/// connection ended it.
/// What a session needs to be given the picture.
struct Picture<'a> {
    link: &'a Link,
    session: &'a str,
    frames: &'a FrameSource,
}

fn video_state(state: VideoState, reason: Option<&str>) -> GatewayControl {
    GatewayControl::Video {
        state,
        stream: None,
        width: None,
        height: None,
        reason: reason.map(str::to_owned),
    }
}

async fn run(
    connection: &Connection,
    control: &mut Control,
    ended: &mut tokio::sync::mpsc::Receiver<EndReason>,
    mut violations: tokio::sync::mpsc::Receiver<()>,
    picture: Picture<'_>,
    config: &Config,
) -> Option<Closing> {
    let mut counters = tokio::time::interval(config.counters_interval);
    counters.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The picture: the task that sends it while the page watches, what it
    // has to tell the page, and what it counted.
    let mut watching: Option<AbortOnDrop> = None;
    let video_control = Arc::new(VideoControl::default());
    let video_counters = Arc::new(VideoCounters::default());
    let (events, mut video_events) = mpsc::channel::<GatewayControl>(4);
    // The share of the bitrate the picture finds the path carries.
    let mut shares = video_control.share.subscribe();
    loop {
        tokio::select! {
            message = control.read() => {
                let reply = match message {
                    // The board's time is the system's monotonic clock, the
                    // one a frame's capture time is given in.
                    Some(PageControl::Ping { id, page_time }) => GatewayControl::Pong {
                        id,
                        page_time,
                        board_time: monotonic_micros(),
                    },
                    // What the page has of the picture: the newest frame it
                    // received whole.
                    Some(PageControl::Received { capture }) => {
                        video_control.page_received.fetch_max(capture, Ordering::AcqRel);
                        continue;
                    }
                    Some(PageControl::Video { subscribe: true }) => {
                        if watching.is_some() {
                            // It watches already: it starts again from a key frame.
                            video_control.page_needs_key.store(true, Ordering::Release);
                            picture.frames.request_key();
                            continue;
                        }
                        // The server holds the encoder's demand first.
                        if let Err(reason) = picture.link.video(picture.session, true).await {
                            video_state(VideoState::Unavailable, Some(&reason))
                        } else if let Some(subscription) = picture.frames.subscribe().await {
                            watching = Some(AbortOnDrop(tokio::spawn(video::serve(
                                connection.clone(),
                                subscription,
                                Arc::clone(&video_control),
                                Arc::clone(&video_counters),
                                events.clone(),
                            ))));
                            // "started" follows with the first key frame.
                            continue;
                        } else {
                            let _ = picture.link.video(picture.session, false).await;
                            video_state(VideoState::Unavailable, Some("encoder"))
                        }
                    }
                    Some(PageControl::Video { subscribe: false }) => {
                        if watching.take().is_some() {
                            let _ = picture.link.video(picture.session, false).await;
                        }
                        video_state(VideoState::Stopped, None)
                    }
                    // The page can decode nothing until a key frame comes:
                    // one is asked for, and no delta frame is sent before it.
                    Some(PageControl::Keyframe { .. }) => {
                        if watching.is_some() {
                            video_control.page_needs_key.store(true, Ordering::Release);
                            picture.frames.request_key();
                        }
                        continue;
                    }
                    Some(PageControl::Bye {}) => return None,
                    // A second hello, or what is not a message.
                    Some(PageControl::Hello { .. }) | None => {
                        return connection_is_open(connection).then_some(Closing::Protocol);
                    }
                };
                if !control.write(&reply).await {
                    return None;
                }
            }
            reason = ended.recv() => {
                return Some(match reason {
                    Some(EndReason::Session) => Closing::Session,
                    Some(EndReason::Shutdown) | None => Closing::Shutdown,
                });
            }
            // What came on the input stream was not a request.
            Some(()) = violations.recv() => return Some(Closing::Protocol),
            Some(event) = video_events.recv() => {
                // The encoder's socket ended: the page may ask again.
                if matches!(event, GatewayControl::Video { state: VideoState::Stopped, .. })
                    && watching.take().is_some()
                {
                    let _ = picture.link.video(picture.session, false).await;
                }
                if !control.write(&event).await {
                    return None;
                }
            }
            Ok(()) = shares.changed() => {
                let percent = *shares.borrow_and_update();
                picture.link.video_share(picture.session, percent);
            }
            _ = counters.tick() => {
                let stats = connection.quic_connection().stats();
                let message = GatewayControl::Counters {
                    round_trip_micros: u32::try_from(connection.rtt().as_micros()).unwrap_or(u32::MAX),
                    packets_lost: stats.path.lost_packets,
                    frames_sent: video_counters.frames_sent.load(Ordering::Relaxed),
                    frames_dropped: video_counters.frames_dropped.load(Ordering::Relaxed),
                    key_frames: video_counters.key_frames.load(Ordering::Relaxed),
                    bitrate_share: *video_control.share.borrow(),
                };
                if !control.write(&message).await {
                    return None;
                }
            }
            _ = connection.closed() => return None,
        }
    }
}

fn connection_is_open(connection: &Connection) -> bool {
    connection.quic_connection().close_reason().is_none()
}

/// The largest datagram for this path: the transport's live limit, and never
/// more than the ceiling that survives a VPN.
fn largest_datagram(connection: &Connection) -> u16 {
    connection
        .max_datagram_size()
        .unwrap_or(0)
        .min(MAX_DATAGRAM_BYTES) as u16
}

/// Tells the page why it is refused or ended, and gives it a moment to read
/// that before the connection goes.
async fn farewell(connection: &Connection, control: &mut Control, message: &GatewayControl) {
    if control.write(message).await && control.send.finish().await.is_ok() {
        let _ = timeout(FAREWELL, connection.closed()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_boards_own_page_may_open_a_session() {
        for (origin, authority) in [
            ("https://kvm.example", "kvm.example"),
            ("https://kvm.example", "kvm.example:443"),
            ("https://KVM.example:443", "kvm.example"),
            ("https://192.0.2.7", "192.0.2.7:443"),
            ("https://[2001:db8::7]", "[2001:db8::7]:443"),
            ("https://kvm.example:8443", "kvm.example:8443"),
            // A page served without TLS on a port has that port's origin.
            ("http://127.0.0.1:18080", "127.0.0.1:18080"),
        ] {
            assert!(
                origin_is_the_boards(Some(origin), authority),
                "{origin} for {authority}"
            );
        }
        for (origin, authority) in [
            ("https://evil.example", "kvm.example"),
            ("https://kvm.example:8443", "kvm.example"),
            ("https://kvm.example", "kvm.example:8443"),
            ("http://kvm.example", "kvm.example"),
            ("https://kvm.example/path", "kvm.example"),
            ("https://user@kvm.example", "kvm.example"),
            ("ftp://kvm.example", "kvm.example"),
            ("kvm.example", "kvm.example"),
            ("https://", "kvm.example"),
            ("https://[2001:db8::7", "[2001:db8::7]"),
            ("null", "kvm.example"),
        ] {
            assert!(
                !origin_is_the_boards(Some(origin), authority),
                "{origin} for {authority}"
            );
        }
        assert!(!origin_is_the_boards(None, "kvm.example"));
    }

    #[test]
    fn a_slot_is_one_of_a_bounded_number() {
        let counter = Arc::new(AtomicUsize::new(0));
        let first = Slot::take(&counter, 2).unwrap();
        let second = Slot::take(&counter, 2).unwrap();
        assert!(Slot::take(&counter, 2).is_none());
        drop(first);
        let third = Slot::take(&counter, 2).unwrap();
        assert!(Slot::take(&counter, 2).is_none());
        drop((second, third));
        assert_eq!(counter.load(Ordering::Acquire), 0);
    }
}
