//! The picture for one session: frames from the encoder, sent as the
//! transport's design says. A group of pictures, a key frame and the delta
//! frames that refer to it, travels in order on a unidirectional stream of
//! its own, written by a task of its own so that the encoder's frames are
//! never held up by the path. The transport retransmits what the path loses:
//! a lost packet delays the group by a round trip instead of breaking the
//! chain of delta frames until the next key frame, as a lost datagram did. A
//! session that lost a frame gets no more delta frames until the next key
//! frame, which is asked for.
//!
//! The page says which frames it received whole, each as it arrives. How long
//! the oldest frame it has not received has been on its way is its lag, and
//! the lag sets the share of the chosen bitrate that the session asks for
//! (`share.rs`). The transport takes what it is given into a buffer of its
//! own long before the path carries it, so what is on its way is judged by
//! the page's word:
//!
//! - When the next key frame is ready, the group before is finished, and its
//!   frames still on their way go on; but if a queue has built in front of
//!   the page, it is reset with every earlier group still on its way, and the
//!   page goes on from the new key frame.
//! - A key frame that the page has not received half a second after it was
//!   sent shows a path that takes long over key frames: the next key frame
//!   would only wait behind it, and is left out with the frames that refer to
//!   it, until the one after. Four seconds without it, its group is given up.
//! - A page that falls more than 1.5 s behind, with more on its way than a
//!   key frame, has every group reset, and the picture starts again from a
//!   key frame that is asked for: the page then shows the present instead of
//!   a picture that grows older on a path that cannot carry it.
//! - A page that has said nothing for a second is silent: frozen in the
//!   background, as a phone's browser does with a page whose app is not in
//!   front, or gone with its session still open until the transport gives
//!   it up. What it has not received is no evidence of its path: no group is
//!   given up or left out for it, and it holds nobody's bitrate down. Once
//!   it speaks again its path is judged afresh.

use std::{
    collections::VecDeque,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use bli_kvm_quic_protocol::{
    frame::{FrameHeader, Kind},
    page::{GatewayControl, VideoState},
};
use tokio::sync::{Notify, mpsc, watch};
use wtransport::{Connection, VarInt};

use crate::{
    clock::monotonic_micros,
    frames::Subscription,
    share::{FULL, PathShare},
};

/// What a session's picture has cost and lost, for its counters.
#[derive(Debug, Default)]
pub struct VideoCounters {
    pub frames_sent: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub key_frames: AtomicU64,
    /// Groups reset because the page fell too far behind.
    pub groups_abandoned: AtomicU64,
}

/// What the session's control side and its picture tell each other.
#[derive(Debug)]
pub struct VideoControl {
    /// The page asked for a key frame: it can decode nothing until one comes.
    pub page_needs_key: AtomicBool,
    /// The capture time of the newest frame the page has said it received
    /// whole; zero until it says.
    pub page_received: AtomicU64,
    /// When the page last said anything, on the board's monotonic clock in
    /// microseconds.
    pub page_heard: AtomicU64,
    /// The share of the chosen bitrate, in percent, that the session's path
    /// carries: the picture sets it, the session tells the server.
    pub share: watch::Sender<u8>,
}

impl Default for VideoControl {
    fn default() -> Self {
        Self {
            page_needs_key: AtomicBool::new(false),
            page_received: AtomicU64::new(0),
            page_heard: AtomicU64::new(monotonic_micros()),
            share: watch::Sender::new(FULL),
        }
    }
}

/// A page whose oldest frame not received has been on its way this long,
/// with more on its way than a key frame, starts again from a key frame.
const MAX_PAGE_LAG: Duration = Duration::from_millis(1500);
/// A key frame not received this long after it was sent is slow: the next
/// is left out until it arrives...
const SLOW_KEY: Duration = Duration::from_millis(500);
/// ... and waited for this long before its group is given up.
const MAX_KEY_WAIT: Duration = Duration::from_secs(4);
/// A page that has said nothing this long is silent.
const SILENT_MICROS: u64 = 1_000_000;
/// The frames sent that are remembered until the page says it received them.
const MAX_UNRECEIVED: usize = 1024;

/// A group's stream that is reset because the next group overtook it.
const OVERTAKEN: u32 = 1;
/// Groups give way to the control and the input stream.
const GROUP_PRIORITY: i32 = -1;

/// The frames sent that the page has not said it received, oldest first:
/// each one's capture time, when it was sent and its size.
#[derive(Debug, Default)]
struct Unreceived {
    frames: VecDeque<(u64, Instant, usize)>,
    bytes: usize,
}

impl Unreceived {
    fn sent(&mut self, capture_micros: u64, now: Instant, bytes: usize) {
        if self.frames.len() == MAX_UNRECEIVED {
            self.pop();
        }
        self.frames.push_back((capture_micros, now, bytes));
        self.bytes += bytes;
    }

    fn pop(&mut self) {
        if let Some((_, _, bytes)) = self.frames.pop_front() {
            self.bytes -= bytes;
        }
    }

    /// The page's lag: how long the oldest frame it has not received, by the
    /// newest it received, has been on its way; nothing if it has them all.
    fn lag(&mut self, received: u64, now: Instant) -> Duration {
        while self
            .frames
            .front()
            .is_some_and(|(capture_micros, _, _)| *capture_micros <= received)
        {
            self.pop();
        }
        self.frames.front().map_or(Duration::ZERO, |(_, sent, _)| {
            now.saturating_duration_since(*sent)
        })
    }

    /// The frames sent so far will not arrive: their group was reset.
    fn forget(&mut self) {
        self.frames.clear();
        self.bytes = 0;
    }
}

/// The stream of the group under way, as the frame loop sees it.
struct Group {
    frames: mpsc::UnboundedSender<(Vec<u8>, u64)>,
    /// The capture time of the last frame the transport took, and of the
    /// last frame handed to the writer.
    taken: Arc<AtomicU64>,
    handed: u64,
    /// The size of the group's key frame.
    key_bytes: usize,
    stream: StreamEnd,
}

/// How the frame loop ends a group's stream once it has let it go: reset,
/// unless the writer has closed it already.
struct StreamEnd {
    reset: Arc<Notify>,
    /// The writer is done: the stream finished and acknowledged, or reset.
    closed: Arc<AtomicBool>,
}

impl StreamEnd {
    fn reset(&self) {
        if !self.closed.load(Ordering::Acquire) {
            self.reset.notify_one();
        }
    }
}

impl Group {
    /// Opens the group's stream with its key frame.
    fn open(connection: &Connection, key: Vec<u8>, capture_micros: u64) -> Self {
        let key_bytes = key.len();
        let (frames, receiver) = mpsc::unbounded_channel();
        let taken = Arc::new(AtomicU64::new(0));
        let stream = StreamEnd {
            reset: Arc::new(Notify::new()),
            closed: Arc::new(AtomicBool::new(false)),
        };
        tokio::spawn(write_group(
            connection.clone(),
            (key, capture_micros),
            receiver,
            Arc::clone(&taken),
            Arc::clone(&stream.reset),
            Arc::clone(&stream.closed),
        ));
        Self {
            frames,
            taken,
            handed: capture_micros,
            key_bytes,
            stream,
        }
    }

    /// Hands a delta frame of the group to its writer; false where the
    /// writer has ended (its stream failed).
    fn push(&mut self, bytes: Vec<u8>, capture_micros: u64) -> bool {
        self.handed = capture_micros;
        self.frames.send((bytes, capture_micros)).is_ok()
    }
}

/// The groups of a picture: the one under way, and those finished whose
/// streams may still be on their way. The transport takes what it is given
/// into its own buffer long before the path carries it, so a finished group
/// is only through once the page has it.
#[derive(Default)]
struct Groups {
    current: Option<Group>,
    finishing: Vec<StreamEnd>,
    /// The newest group's key frame: its capture time and when it was sent.
    key: Option<(u64, Instant)>,
    /// The picture ended with the encoder's socket: what is on its way may
    /// still arrive. Otherwise the page gave the picture up, and every
    /// stream still under way is reset.
    ended: bool,
}

impl Groups {
    /// The next group starts with its key frame, sent now. The one before is
    /// finished, unless the page is behind it, or the transport has not
    /// taken all of it: then it is reset, with every earlier one still on
    /// its way. True if it was.
    fn start(&mut self, next: Group, behind: bool, now: Instant) -> bool {
        self.finishing
            .retain(|stream| !stream.closed.load(Ordering::Acquire));
        let reset = match self.current.take() {
            Some(earlier) if behind || earlier.taken.load(Ordering::Acquire) < earlier.handed => {
                earlier.stream.reset();
                self.give_up();
                true
            }
            Some(earlier) => {
                self.finish(earlier);
                false
            }
            None => false,
        };
        self.key = Some((next.handed, now));
        self.current = Some(next);
        reset
    }

    /// The group under way gets no more frames: its writer finishes its
    /// stream once the frames it has are written.
    fn finish_current(&mut self) {
        if let Some(current) = self.current.take() {
            self.finish(current);
        }
    }

    fn finish(&mut self, group: Group) {
        // Dropping its sender lets the writer finish the stream.
        self.finishing.push(group.stream);
    }

    /// How long the newest key frame has been on its way, if the page has
    /// not said it received it.
    fn key_waited(&self, received: u64, now: Instant) -> Option<Duration> {
        self.key
            .filter(|(capture_micros, _)| received < *capture_micros)
            .map(|(_, sent)| now.saturating_duration_since(sent))
    }

    /// Every stream still under way is reset.
    fn give_up(&mut self) {
        if let Some(current) = self.current.take() {
            current.stream.reset();
        }
        for stream in self.finishing.drain(..) {
            stream.reset();
        }
        self.key = None;
    }
}

impl Drop for Groups {
    fn drop(&mut self) {
        if !self.ended {
            self.give_up();
        }
    }
}

/// Writes a group on a stream of its own: its key frame, then its delta
/// frames as they come, until the frame loop lets it go (the stream is
/// finished) or resets it.
async fn write_group(
    connection: Connection,
    key: (Vec<u8>, u64),
    frames: mpsc::UnboundedReceiver<(Vec<u8>, u64)>,
    taken: Arc<AtomicU64>,
    reset: Arc<Notify>,
    closed: Arc<AtomicBool>,
) {
    write_frames(connection, key, frames, taken, reset).await;
    closed.store(true, Ordering::Release);
}

async fn write_frames(
    connection: Connection,
    key: (Vec<u8>, u64),
    mut frames: mpsc::UnboundedReceiver<(Vec<u8>, u64)>,
    taken: Arc<AtomicU64>,
    reset: Arc<Notify>,
) {
    let opening = tokio::select! {
        opening = connection.open_uni() => opening,
        () = reset.notified() => return,
    };
    let Ok(opening) = opening else {
        return;
    };
    let mut stream = tokio::select! {
        stream = opening => match stream {
            Ok(stream) => stream,
            Err(_) => return,
        },
        () = reset.notified() => return,
    };
    stream.set_priority(GROUP_PRIORITY);
    tokio::select! {
        written = async {
            stream.write_all(&key.0).await?;
            taken.store(key.1, Ordering::Release);
            while let Some((bytes, capture_micros)) = frames.recv().await {
                stream.write_all(&bytes).await?;
                taken.store(capture_micros, Ordering::Release);
            }
            stream.finish().await
        } => {
            let _ = written;
        }
        () = reset.notified() => {
            let _ = stream.reset(VarInt::from_u32(OVERTAKEN));
        }
    }
}

/// Whether a key frame is left out because the page still waits for the
/// one before it. `new_picture` is a key frame of another stream or size.
/// The wait of an earlier picture does not count: its streams are given up
/// as the new one starts (code audit of 2026-10-20, AUD-18).
fn skips_key(new_picture: bool, key_waited: Option<Duration>) -> bool {
    !new_picture && key_waited.is_some_and(|waited| waited > SLOW_KEY && waited <= MAX_KEY_WAIT)
}

/// Sends the frames of `subscription` to the session until the encoder's
/// socket ends or the session does. `events` carries what the page is told
/// on its control stream.
pub async fn serve(
    connection: Connection,
    mut subscription: Subscription,
    control: Arc<VideoControl>,
    counters: Arc<VideoCounters>,
    events: mpsc::Sender<GatewayControl>,
) {
    // Nothing can be decoded before a key frame.
    let mut needs_key = true;
    let mut announced: Option<(u32, u16, u16)> = None;
    let mut groups = Groups::default();
    let mut unreceived = Unreceived::default();
    let mut share = PathShare::new(*control.share.borrow(), Instant::now());
    let mut silent = false;
    let tell = |percent: Option<u8>| {
        if let Some(percent) = percent {
            control.share.send_replace(percent);
        }
    };
    while let Some(record) = subscription.frames.recv().await {
        if subscription.take_lagged() {
            // Frames were lost to a full queue: the chain is broken.
            counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
            needs_key = true;
            subscription.request_key();
        }
        if control.page_needs_key.swap(false, Ordering::AcqRel) {
            needs_key = true;
            subscription.request_key();
        }
        let header = &record.header;
        let now = Instant::now();
        let received = control.page_received.load(Ordering::Acquire);
        let lag = unreceived.lag(received, now);
        let heard = control.page_heard.load(Ordering::Acquire);
        match (
            silent,
            monotonic_micros().saturating_sub(heard) > SILENT_MICROS,
        ) {
            (false, true) => {
                // A silent page holds nobody's bitrate down.
                silent = true;
                control
                    .share
                    .send_if_modified(|percent| std::mem::replace(percent, FULL) != FULL);
            }
            (true, false) => {
                // It speaks again: its path is judged afresh.
                silent = false;
                share = PathShare::new(FULL, now);
            }
            _ => {}
        }
        if !silent {
            tell(share.lag(now, lag));
        }
        // What a silent page has not received is no evidence of its path.
        let key_waited = groups.key_waited(received, now).filter(|_| !silent);
        match header.kind {
            Kind::Key => {
                let Ok(head) = FrameHeader::key(
                    header.stream,
                    header.frame,
                    header.size,
                    header.capture_micros,
                )
                .encode() else {
                    continue;
                };
                let picture = (header.stream, header.width, header.height);
                let new_picture = announced != Some(picture);
                if new_picture {
                    // What is on its way of the picture before is of no use
                    // to a page that starts this one.
                    groups.give_up();
                    announced = Some(picture);
                    let _ = events
                        .send(GatewayControl::Video {
                            state: VideoState::Started,
                            stream: Some(header.stream),
                            width: Some(header.width),
                            height: Some(header.height),
                            reason: None,
                        })
                        .await;
                }
                if skips_key(new_picture, key_waited) {
                    // The page still waits for the key frame before: this one
                    // would only wait behind it, and is left out with the
                    // frames that refer to it, the group before finished.
                    groups.finish_current();
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                let mut bytes = Vec::with_capacity(head.len() + record.data.len());
                bytes.extend_from_slice(&head);
                bytes.extend_from_slice(&record.data);
                let size = bytes.len();
                let next = Group::open(&connection, bytes, header.capture_micros);
                // The group before is finished, or reset with every earlier
                // one if a queue has built in front of the page.
                let behind = !silent
                    && (share.queued()
                        || lag > MAX_PAGE_LAG
                        || key_waited.is_some_and(|waited| waited > MAX_KEY_WAIT));
                if groups.start(next, behind, now) {
                    unreceived.forget();
                }
                unreceived.sent(header.capture_micros, now, size);
                needs_key = false;
                counters.key_frames.fetch_add(1, Ordering::Relaxed);
                counters.frames_sent.fetch_add(1, Ordering::Relaxed);
            }
            Kind::Delta => {
                let Some(current) = groups.current.as_mut().filter(|_| !needs_key) else {
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    continue;
                };
                let given_up = !silent
                    && match key_waited {
                        // A key frame on its way too long.
                        Some(waited) => waited > MAX_KEY_WAIT,
                        // More on its way than a key frame, for too long.
                        None => lag > MAX_PAGE_LAG && unreceived.bytes > current.key_bytes,
                    };
                if given_up {
                    // The path does not carry the picture: what is queued is
                    // dropped and the picture starts again from a key frame.
                    groups.give_up();
                    unreceived.forget();
                    tell(share.gave_up(now));
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    counters.groups_abandoned.fetch_add(1, Ordering::Relaxed);
                    needs_key = true;
                    subscription.request_key();
                    continue;
                }
                let Ok(head) = FrameHeader::whole_delta(
                    header.stream,
                    header.frame,
                    header.reference,
                    header.size,
                    header.capture_micros,
                )
                .encode() else {
                    continue;
                };
                let mut bytes = Vec::with_capacity(head.len() + record.data.len());
                bytes.extend_from_slice(&head);
                bytes.extend_from_slice(&record.data);
                let size = bytes.len();
                if current.push(bytes, header.capture_micros) {
                    unreceived.sent(header.capture_micros, now, size);
                    counters.frames_sent.fetch_add(1, Ordering::Relaxed);
                } else {
                    // The group's stream failed: the chain is broken.
                    groups.give_up();
                    counters.frames_dropped.fetch_add(1, Ordering::Relaxed);
                    needs_key = true;
                    subscription.request_key();
                }
            }
        }
    }
    // The encoder's socket ended: what is on its way is finished, the page
    // is told, and may ask again.
    groups.ended = true;
    drop(groups);
    let _ = events
        .send(GatewayControl::Video {
            state: VideoState::Stopped,
            stream: None,
            width: None,
            height: None,
            reason: Some("encoder".into()),
        })
        .await;
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn a_new_pictures_key_frame_is_not_left_out_for_the_old_ones() {
        // The wait belonged to the old stream's key frame, which the page
        // gives up with its stream; the new stream's first key frame was
        // left out, and its picture waited for the next one (code audit of
        // 2026-10-20, AUD-18).
        let slow = Some(SLOW_KEY + Duration::from_millis(100));
        assert!(skips_key(false, slow));
        assert!(!skips_key(true, slow));
        assert!(!skips_key(false, None));
    }
}
